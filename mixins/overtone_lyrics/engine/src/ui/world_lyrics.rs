use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{
    FollowStyle, HudConfig, WorldAnim, WorldColor, WorldExit, WorldGlow, WorldLayout, WorldMode,
    WorldUnit,
};
use crate::controller::PlayerController;
use crate::i18n;
use crate::lyrics::{self, LyricLine};
use crate::ui::TextMetrics;

const MAX_ROW_W: f32 = 230.0;
const ROW_H: f32 = 12.0;
const BASE_SCALE: f32 = 0.026;
const FLY_MS: f32 = 380.0;
const HOLD_MS: i64 = 1500;
const SCATTER_HOLD_MS: i64 = 3200;
const FADE_MS: i64 = 800;
const MAX_DEBRIS: usize = 900;
const GRAVITY: f32 = 11.0;
const TELEPORT_SQ: f64 = 32.0 * 32.0;
const OCCLUSION_CHECK_NS: u128 = 150_000_000;

/// Probe implemented by Horizon's world / mock tests for block ground detection & occlusion.
pub trait WorldBlockProbe {
    /// Returns the highest solid collision top Y at or below `y + 0.05` around `(x, z)`, or `y - 64.0`.
    fn ground_below(&self, x: f64, y: f64, z: f64) -> f64;
    /// Returns `true` if a non-occluding (translucent) block sits between `cam` and `target`,
    /// matching Overtone's `occluded(...)` (glass/leaves let text punch through).
    fn is_translucent_occluded(&self, cam: [f64; 3], target: [f64; 3]) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NullWorldProbe;

impl WorldBlockProbe for NullWorldProbe {
    fn ground_below(&self, _x: f64, y: f64, _z: f64) -> f64 {
        y - 64.0
    }
    fn is_translucent_occluded(&self, _cam: [f64; 3], _target: [f64; 3]) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlowSpec {
    pub rgb: u32,
    pub inner_alpha: u8,
    pub outer_alpha: u8,
    pub radius: f32,
    pub enhanced: bool,
}

/// One 3D glyph / word draw command emitted for the active world frame.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldTextCmd {
    pub origin: [f64; 3],
    pub yaw: f32,
    pub flat_pitch: f32,
    pub roll: f32,
    pub base_scale: f32,
    pub local_pos: [f32; 3],
    pub piece_rot_y: f32,
    pub piece_rot_z: f32,
    pub piece_scale: f32,
    pub text: String,
    pub width: f32,
    pub argb: u32,
    pub shadow: bool,
    pub see_through: bool,
    pub glow: Option<GlowSpec>,
}

#[derive(Clone, Debug)]
struct Glyph {
    ch: String,
    w: f32,
    x: f32,
}

#[derive(Clone, Debug)]
struct Word {
    text: String,
    w: f32,
    x: f32,
    y: f32,
    sx: f32,
    sy: f32,
    sz: f32,
    start_rel: i64,
    dur_rel: i64,
    glyphs: Vec<Glyph>,
}

#[derive(Clone, Debug)]
struct Group {
    index: i32,
    follow: bool,
    fixed_yaw: bool,
    timed: bool,
    line_ms: i64,
    hold: i64,
    end_rel: i64,
    rows: usize,
    min_x: f32,
    max_x: f32,
    lateral: f32,
    vert: f32,
    yaw_off: f64,
    dist_k: f64,
    size_k: f32,
    tilt: f32,
    scatter: bool,
    placed: bool,
    shattered: bool,
    px: f64,
    py: f64,
    pz: f64,
    spawn_yaw: f64,
    lift_init: bool,
    lift_cur: f32,
    occ_checked_at: u128,
    occluded: bool,
    wall_start_ns: u128,
    words: Vec<Word>,
}

impl Group {
    fn new(index: i32, follow: bool, line_ms: i64, timed: bool) -> Self {
        Self {
            index,
            follow,
            fixed_yaw: false,
            timed,
            line_ms,
            hold: HOLD_MS,
            end_rel: 0,
            rows: 1,
            min_x: 0.0,
            max_x: 0.0,
            lateral: 0.0,
            vert: 0.0,
            yaw_off: 0.0,
            dist_k: 1.0,
            size_k: 1.0,
            tilt: 0.0,
            scatter: false,
            placed: false,
            shattered: false,
            px: 0.0,
            py: 0.0,
            pz: 0.0,
            spawn_yaw: 0.0,
            lift_init: false,
            lift_cur: 0.0,
            occ_checked_at: 0,
            occluded: false,
            wall_start_ns: now_ns(),
            words: Vec::new(),
        }
    }

    fn total(&self) -> i64 {
        self.end_rel + self.hold + FADE_MS
    }
}

#[derive(Clone, Debug)]
struct Debris {
    ch: String,
    w: f32,
    s: f32,
    mode: WorldExit,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    rest_yaw: f32,
    ground: f64,
    vx: f32,
    vy: f32,
    vz: f32,
    roll: f32,
    roll_v: f32,
    phase: f32,
    delay: f32,
    age: f32,
    landed: bool,
    bounced: bool,
    landed_at: f32,
    flat: f32,
    rgb: u32,
}

#[derive(Default)]
struct SimpleRng(u64);

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x5DEECE66D)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / (u32::MAX as f64)
    }
    fn next_f32(&mut self) -> f32 {
        self.next_f64() as f32
    }
    fn next_bool(&mut self) -> bool {
        (self.next_u32() & 1) == 1
    }
}

pub struct WorldLyricsState {
    groups: HashMap<i32, Group>,
    scatter_lines: HashMap<i32, Vec<Group>>,
    extras: Vec<Group>,
    debris: Vec<Debris>,
    lines_sig: u64,
    last_render_ns: u128,
    est_pos: f64,
    est_init: bool,
    last_dim: String,
    last_cam: Option<[f64; 3]>,
    pause_k: f32,
}

impl Default for WorldLyricsState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldLyricsState {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
            scatter_lines: HashMap::new(),
            extras: Vec::new(),
            debris: Vec::new(),
            lines_sig: 0,
            last_render_ns: 0,
            est_pos: 0.0,
            est_init: false,
            last_dim: String::new(),
            last_cam: None,
            pause_k: 1.0,
        }
    }

    pub fn hides_hud(ctl: &PlayerController, in_world: bool) -> bool {
        let cfg = ctl.hud_config();
        Self::takes_over_with(ctl, &cfg, in_world) && !cfg.lyrics_with_world
    }

    fn takes_over_with(ctl: &PlayerController, cfg: &HudConfig, in_world: bool) -> bool {
        if !cfg.world_lyrics || !in_world {
            return false;
        }
        let Some(cand) = ctl.active_lyrics() else {
            return false;
        };
        ctl.current_track().is_some()
            && !ctl.is_idle()
            && !cand.lines.is_empty()
            && cand.lines.iter().any(|l| l.is_synced())
    }

    pub fn toast(&mut self, metrics: &dyn TextMetrics, text: &str) {
        self.extras.retain(|g| g.index != -1);
        let mut g = Group::new(-1, true, 0, false);
        g.vert = -0.35;
        add_words(&mut g, metrics, text, 90);
        g.end_rel = g.end_rel.max(400);
        self.extras.push(g);
    }

    pub fn spawn_demo(&mut self, cfg: &HudConfig, metrics: &dyn TextMetrics) {
        self.extras.retain(|g| g.index != -2 && g.index > -1000);
        let text = i18n::raw("world.demo");
        if cfg.world_layout != WorldLayout::Scatter {
            let mut g = Group::new(-2, true, 0, false);
            add_words(&mut g, metrics, &text, 330);
            self.extras.push(g);
        } else {
            let now = now_ns();
            let mut placed: Vec<Group> = Vec::new();
            for (k, part) in text.split_whitespace().enumerate() {
                let mut g = Group::new(-1000 - k as i32, true, 0, false);
                g.words.push(make_word(part, 0, 330));
                finish_group(&mut g, metrics);
                let mut refs: Vec<&Group> = placed.iter().collect();
                make_scatter(&mut g, &mut refs, cfg);
                g.wall_start_ns = now + (k as u128) * 520_000_000;
                placed.push(g.clone());
                self.extras.push(g);
            }
        }
    }

    pub fn active_group_count(&self) -> usize {
        self.groups.len() + self.scatter_lines.len() + self.extras.len()
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.scatter_lines.clear();
        self.extras.clear();
        self.debris.clear();
        self.est_init = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_emit(
        &mut self,
        ctl: &PlayerController,
        metrics: &dyn TextMetrics,
        probe: &dyn WorldBlockProbe,
        cam: [f64; 3],
        cam_yaw_deg: f32,
        dim: &str,
    ) -> Vec<WorldTextCmd> {
        self.update_and_emit_view(ctl, metrics, probe, cam, cam_yaw_deg, None, dim)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_emit_view(
        &mut self,
        ctl: &PlayerController,
        metrics: &dyn TextMetrics,
        probe: &dyn WorldBlockProbe,
        cam: [f64; 3],
        cam_yaw_deg: f32,
        cam_pitch_deg: Option<f32>,
        dim: &str,
    ) -> Vec<WorldTextCmd> {
        let cinema_pitch = cam_pitch_deg.map(|p| f64::from(p).to_radians());
        let now = now_ns();
        let dt = if self.last_render_ns == 0 {
            0.016f32
        } else {
            ((now.saturating_sub(self.last_render_ns) as f64) / 1e9).clamp(0.0, 0.1) as f32
        };
        self.last_render_ns = now;
        let wall = (now as f64) / 1e9;
        let cfg = &ctl.hud_config();

        if self.last_dim.is_empty() {
            self.last_dim = dim.to_owned();
        } else if self.last_dim != dim {
            self.last_dim = dim.to_owned();
            self.last_cam = Some(cam);
            self.clear();
            return Vec::new();
        }
        if let Some(prev) = self.last_cam {
            let dx = cam[0] - prev[0];
            let dy = cam[1] - prev[1];
            let dz = cam[2] - prev[2];
            if dx * dx + dy * dy + dz * dz > TELEPORT_SQ {
                self.clear();
            }
        }
        self.last_cam = Some(cam);

        let yaw_rad = f64::from(cam_yaw_deg).to_radians();
        let mut out = Vec::new();

        // Extras (demo & toasts)
        let extra_len = self.extras.len();
        let mut i = self.extras.len();
        while i > 0 {
            i -= 1;
            let rel = ((now as i128 - self.extras[i].wall_start_ns as i128) / 1_000_000) as i64;
            let total = self.extras[i].total();
            if rel > total + if extra_len > 1 { 200 } else { 0 } {
                self.extras.remove(i);
                continue;
            }
            if rel >= 0 {
                let mut g = self.extras[i].clone();
                let shattered = emit_group(
                    &mut g,
                    rel,
                    0.0,
                    cam,
                    yaw_rad,
                    cinema_pitch,
                    cfg,
                    dt,
                    wall,
                    self.pause_k,
                    probe,
                    &mut self.debris,
                    &mut out,
                );
                self.extras[i] = g;
                if shattered {
                    self.extras.remove(i);
                }
            }
        }

        self.step_debris(cam, cfg, dt, &mut out);

        let current = ctl.current_track();
        let idle = ctl.is_idle();
        if !cfg.world_lyrics || current.is_none() || idle {
            self.groups.clear();
            self.scatter_lines.clear();
            return out;
        }
        let Some(cand) = ctl.active_lyrics() else {
            self.groups.clear();
            self.scatter_lines.clear();
            return out;
        };
        let lines = &cand.lines;
        if lines.is_empty() || !lines.iter().any(|l| l.is_synced()) {
            self.groups.clear();
            self.scatter_lines.clear();
            return out;
        }

        let track_id = current.as_ref().map(|t| t.id.as_str()).unwrap_or("");
        let sig = lyrics_signature(track_id, cand.source, lines);
        if sig != self.lines_sig {
            self.lines_sig = sig;
            self.groups.clear();
            self.scatter_lines.clear();
            self.est_init = false;
        }

        let polled = ctl.position_ms();
        if self.est_init && (polled as f64 - self.est_pos).abs() <= 150.0 {
            if ctl.is_playing() {
                self.est_pos += f64::from(dt) * 1000.0;
            }
            self.est_pos += (polled as f64 - self.est_pos) * f64::from(dt * 18.0).min(1.0);
        } else {
            self.est_pos = polled as f64;
            self.est_init = true;
        }

        let pos = self.est_pos.round().max(0.0) as u64;
        self.pause_k = if ctl.is_playing() {
            (self.pause_k + dt / 0.35).min(1.0)
        } else {
            (self.pause_k - dt / 0.6).max(0.0)
        };

        let spos_u64 = ctl.adjusted_lyrics_pos_ms(pos, track_id, cfg.lyrics_shift_ms);
        let Some(cur) = lyrics::current_index(lines, spos_u64) else {
            self.groups.clear();
            self.scatter_lines.clear();
            return out;
        };

        let spos = spos_u64 as i64;

        let scatter = cfg.world_layout == WorldLayout::Scatter;
        let follow = cfg.world_mode == WorldMode::Follow;
        let fixed_yaw = follow && (scatter || cfg.world_follow_style == FollowStyle::World);

        if scatter {
            let cur_i = cur as i32;
            self.scatter_lines
                .retain(|&k, _| k >= cur_i - 8 && k <= cur_i);
        } else {
            self.scatter_lines.clear();
        }

        let min_i = cur.saturating_sub(if scatter { 7 } else { 4 });
        let mut keep = HashSet::new();
        let mut active_indices: Vec<(i32, usize)> = Vec::new();

        for idx in min_i..=cur {
            let line = &lines[idx];
            if line.text.trim().is_empty() || !line.is_synced() {
                continue;
            }
            let next = lines.get(idx + 1);
            let key = idx as i32;
            if scatter {
                let needs_rebuild = match self.scatter_lines.get(&key) {
                    Some(v) => v
                        .first()
                        .is_none_or(|g| g.follow != follow || g.fixed_yaw != fixed_yaw),
                    None => true,
                };
                if needs_rebuild {
                    let existing: Vec<Group> = self
                        .scatter_lines
                        .values()
                        .flatten()
                        .filter(|o| spos - o.line_ms <= o.total())
                        .cloned()
                        .collect();
                    let built = scatter_from_line(
                        key, line, next, follow, fixed_yaw, metrics, cfg, &existing,
                    );
                    self.scatter_lines.insert(key, built);
                }
                if let Some(list) = self.scatter_lines.get(&key) {
                    for (sub_i, g) in list.iter().enumerate() {
                        if spos - g.line_ms <= g.total() {
                            active_indices.push((key, sub_i));
                        }
                    }
                }
            } else {
                let needs_rebuild = match self.groups.get(&key) {
                    Some(g) => g.follow != follow || g.fixed_yaw != fixed_yaw,
                    None => true,
                };
                if needs_rebuild {
                    let built = from_line(key, line, next, follow, fixed_yaw, metrics);
                    self.groups.insert(key, built);
                }
                if let Some(g) = self.groups.get(&key)
                    && spos - g.line_ms <= g.total()
                {
                    keep.insert(key);
                    active_indices.push((key, 0));
                }
            }
        }

        if !scatter {
            self.groups.retain(|k, _| keep.contains(k));
            let mut acc = 0.0f32;
            for k in (0..active_indices.len()).rev() {
                let key = active_indices[k].0;
                if k + 1 < active_indices.len() {
                    let newer_key = active_indices[k + 1].0;
                    let newer_rows =
                        self.groups.get(&newer_key).map(|g| g.rows).unwrap_or(1) as f32;
                    let cur_rows = self.groups.get(&key).map(|g| g.rows).unwrap_or(1) as f32;
                    acc += (newer_rows + cur_rows) / 2.0 * ROW_H + 8.0;
                }
                if let Some(g) = self.groups.get_mut(&key) {
                    let rel = spos - g.line_ms;
                    emit_group(
                        g,
                        rel,
                        -acc,
                        cam,
                        yaw_rad,
                        cinema_pitch,
                        cfg,
                        dt,
                        wall,
                        self.pause_k,
                        probe,
                        &mut self.debris,
                        &mut out,
                    );
                }
            }
        } else {
            for (key, sub_i) in active_indices {
                if let Some(list) = self.scatter_lines.get_mut(&key)
                    && let Some(g) = list.get_mut(sub_i)
                {
                    let rel = spos - g.line_ms;
                    emit_group(
                        g,
                        rel,
                        0.0,
                        cam,
                        yaw_rad,
                        cinema_pitch,
                        cfg,
                        dt,
                        wall,
                        self.pause_k,
                        probe,
                        &mut self.debris,
                        &mut out,
                    );
                }
            }
        }

        out
    }

    fn step_debris(
        &mut self,
        cam: [f64; 3],
        cfg: &HudConfig,
        dt: f32,
        out: &mut Vec<WorldTextCmd>,
    ) {
        let mut i = self.debris.len();
        while i > 0 {
            i -= 1;
            let d = &mut self.debris[i];
            d.age += dt;
            let t = d.age - d.delay;
            let mut alpha = 1.0f32;
            let mut k = 1.0f32;
            let mut dead = false;
            if t > 0.0 {
                match d.mode {
                    WorldExit::Fall | WorldExit::Shatter => {
                        if !d.landed {
                            d.vy -= GRAVITY * dt;
                            d.x += f64::from(d.vx * dt);
                            d.y += f64::from(d.vy * dt);
                            d.z += f64::from(d.vz * dt);
                            d.roll += d.roll_v * dt;
                            let half = 4.5 * f64::from(d.s);
                            if d.y - half <= d.ground {
                                if d.vy < -3.0 && !d.bounced {
                                    d.bounced = true;
                                    d.vy = -d.vy * 0.22;
                                    d.vx *= 0.4;
                                    d.vz *= 0.4;
                                    d.y = d.ground + half;
                                } else {
                                    d.landed = true;
                                    d.landed_at = t;
                                    d.y = d.ground + 0.015;
                                }
                            }
                            if d.y < d.ground - 40.0 {
                                dead = true;
                            }
                        } else {
                            d.flat = (d.flat + dt / 0.14).min(1.0);
                            let left = t - d.landed_at - cfg.world_linger as f32;
                            alpha = 1.0 - clamp01(left / 1.2);
                            dead = left > 1.2;
                        }
                    }
                    WorldExit::Float => {
                        d.y += f64::from(d.vy * dt);
                        d.x += f64::from((t * 2.3 + d.phase).sin() * 0.25 * dt);
                        d.z += f64::from((t * 1.9 + d.phase).cos() * 0.25 * dt);
                        k = 1.0 + t * 0.18;
                        alpha = 1.0 - clamp01(t / 2.6);
                        dead = t > 2.6;
                    }
                    WorldExit::Wind => {
                        d.vy -= 0.9 * dt;
                        d.x += f64::from(d.vx * dt);
                        d.y += f64::from((d.vy + (t * 7.0 + d.phase).sin() * 0.5) * dt);
                        d.z += f64::from(d.vz * dt);
                        d.roll += d.roll_v * dt;
                        alpha = 1.0 - clamp01(t / 2.1);
                        dead = t > 2.1;
                    }
                    WorldExit::Shrink => {
                        k = 1.0 - ease_out_cubic(clamp01(t / 0.35));
                        dead = t > 0.35;
                    }
                    WorldExit::Sink => {
                        d.y -= f64::from(0.85 * dt);
                        alpha = 1.0 - clamp01(t / 0.9);
                        dead = t > 0.9;
                    }
                    WorldExit::Fade => dead = true,
                }
            }
            if dead {
                self.debris.remove(i);
                continue;
            }
            if alpha < 0.03 || k <= 0.01 {
                continue;
            }
            let _ = cam;
            let glow = if cfg.world_glow == WorldGlow::All && cfg.world_glow_strength > 0 {
                let glow_rgb = lerp_rgb(d.rgb, 0xFF_FFFF, 0.2);
                let ga = (alpha * cfg.world_glow_strength as f32 * 0.8).round() as u8;
                let oa = (alpha * cfg.world_glow_strength as f32 * 0.35).round() as u8;
                Some(GlowSpec {
                    rgb: glow_rgb,
                    inner_alpha: ga,
                    outer_alpha: oa,
                    radius: (cfg.world_glow_size as f32 / 100.0) * 0.7,
                    enhanced: cfg.world_enhanced_glow,
                })
            } else {
                None
            };
            let a255 = (alpha * 255.0).round() as u32;
            out.push(WorldTextCmd {
                origin: [d.x, d.y, d.z],
                yaw: d.yaw + (d.rest_yaw - d.yaw) * d.flat,
                flat_pitch: if d.flat > 0.0 {
                    -std::f32::consts::FRAC_PI_2 * ease_out_cubic(d.flat)
                } else {
                    0.0
                },
                roll: d.roll * (1.0 - d.flat),
                base_scale: d.s * k,
                local_pos: [-d.w / 2.0, -4.5, 0.0],
                piece_rot_y: 0.0,
                piece_rot_z: 0.0,
                piece_scale: 1.0,
                text: d.ch.clone(),
                width: d.w,
                argb: (a255 << 24) | (d.rgb & 0x00FF_FFFF),
                shadow: cfg.world_shadow,
                see_through: cfg.world_see_through,
                glow,
            });
        }
    }
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn lyrics_signature(
    track_id: &str,
    source: crate::model::LyricSourceKind,
    lines: &[LyricLine],
) -> u64 {
    let mut h = 14695981039346656037u64;
    for b in track_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h = (h ^ (source as u64)).wrapping_mul(1099511628211);
    h = (h ^ (lines.len() as u64)).wrapping_mul(1099511628211);
    for line in lines {
        h = (h ^ line.time_ms).wrapping_mul(1099511628211);
        h = (h ^ (line.words.len() as u64)).wrapping_mul(1099511628211);
    }
    h
}

fn make_word(text: &str, start_rel: i64, dur_rel: i64) -> Word {
    Word {
        text: text.to_owned(),
        w: 1.0,
        x: 0.0,
        y: 0.0,
        sx: 0.0,
        sy: 0.0,
        sz: 0.0,
        start_rel,
        dur_rel,
        glyphs: Vec::new(),
    }
}

fn add_words(g: &mut Group, metrics: &dyn TextMetrics, text: &str, step_ms: i64) {
    let mut t = 0;
    for part in text.split_whitespace() {
        g.words.push(make_word(part, t, step_ms));
        t += step_ms;
    }
    finish_group(g, metrics);
}

fn from_line(
    index: i32,
    line: &LyricLine,
    next: Option<&LyricLine>,
    follow: bool,
    fixed_yaw: bool,
    metrics: &dyn TextMetrics,
) -> Group {
    let mut g = Group::new(index, follow, line.time_ms as i64, true);
    g.fixed_yaw = fixed_yaw;
    let line_dur = next
        .filter(|n| n.is_synced())
        .map(|n| (n.time_ms as i64 - line.time_ms as i64).max(400))
        .unwrap_or(4000);

    if !line.words.is_empty() {
        for (i, w) in line.words.iter().enumerate() {
            let start_rel = (w.time_ms as i64 - line.time_ms as i64).max(0);
            let end = w
                .end_ms
                .map(|e| e as i64 - line.time_ms as i64)
                .or_else(|| {
                    line.words
                        .get(i + 1)
                        .map(|nw| nw.time_ms as i64 - line.time_ms as i64)
                })
                .unwrap_or((line_dur - 100).max(start_rel + 450));
            let dur_rel = (end - start_rel).max(140);
            g.words.push(make_word(&w.text, start_rel, dur_rel));
        }
    } else {
        // Match HUD Subtitles character sweep 1:1 over the full line duration [0..line_dur].
        let parts: Vec<&str> = line.text.split_whitespace().collect();
        let count = parts.len();
        let total_chars: usize = parts
            .iter()
            .enumerate()
            .map(|(i, p)| p.chars().count().max(1) + usize::from(i + 1 < count))
            .sum();
        if total_chars > 0 {
            let mut before = 0i64;
            for (i, p) in parts.iter().enumerate() {
                let len = p.chars().count().max(1) as i64 + i64::from(i + 1 < count);
                let start_rel = line_dur * before / total_chars as i64;
                let end_rel = line_dur * (before + len) / total_chars as i64;
                let dur_rel = (end_rel - start_rel).max(140);
                g.words.push(make_word(p, start_rel, dur_rel));
                before += len;
            }
        }
    }
    finish_group(&mut g, metrics);
    g
}

#[allow(clippy::too_many_arguments)]
fn scatter_from_line(
    line_idx: i32,
    line: &LyricLine,
    next: Option<&LyricLine>,
    follow: bool,
    fixed_yaw: bool,
    metrics: &dyn TextMetrics,
    cfg: &HudConfig,
    existing: &[Group],
) -> Vec<Group> {
    let base = from_line(line_idx, line, next, follow, fixed_yaw, metrics);
    let mut out = Vec::with_capacity(base.words.len());
    let mut placed_refs: Vec<Group> = existing.to_vec();
    for (i, w) in base.words.iter().enumerate() {
        let mut g = Group::new(
            line_idx.saturating_mul(100) + i as i32,
            follow,
            line.time_ms as i64 + w.start_rel,
            true,
        );
        g.fixed_yaw = fixed_yaw;
        let mut single = w.clone();
        single.start_rel = 0;
        g.words.push(single);
        finish_group(&mut g, metrics);
        let mut refs: Vec<&Group> = placed_refs.iter().collect();
        make_scatter(&mut g, &mut refs, cfg);
        placed_refs.push(g.clone());
        out.push(g);
    }
    out
}

fn make_scatter(g: &mut Group, others: &mut Vec<&Group>, cfg: &HudConfig) {
    g.scatter = true;
    g.hold = SCATTER_HOLD_MS;
    g.lateral = 0.0;
    let mut rnd = SimpleRng::new((g.index as i64 * 7919 + 12345) as u64);
    let base = f64::from(cfg.world_distance);
    let span = f64::from(cfg.world_spread).to_radians().max(0.2);
    let mut best_yaw = 0.0;
    let mut best_vert = 0.0;
    let mut best_k = 0.6;
    let mut best_score = -1.0;

    for _ in 0..16 {
        let sign = if rnd.next_bool() { 1.0 } else { -1.0 };
        let yo = if rnd.next_f64() < 0.85 {
            sign * (0.12 + rnd.next_f64() * (span - 0.12))
        } else {
            (rnd.next_f64() * 2.0 - 1.0) * 0.12
        };
        let vt = -1.35 + rnd.next_f64().powf(1.3) * 1.6;
        let k = 0.45 + rnd.next_f64() * 0.5;
        let d = base * k;
        let x = -yo.sin() * d;
        let z = yo.cos() * d;
        let mut score = 10.0f64;
        for o in others.iter().filter(|o| o.scatter) {
            let od = base * o.dist_k;
            let ox = -o.yaw_off.sin() * od;
            let oz = o.yaw_off.cos() * od;
            let dx = x - ox;
            let dz = z - oz;
            let dy = (vt - f64::from(o.vert)) * 1.3;
            score = score.min((dx * dx + dz * dz + dy * dy).sqrt());
        }
        if score > best_score {
            best_score = score;
            best_yaw = yo;
            best_vert = vt;
            best_k = k;
        }
    }
    g.yaw_off = best_yaw;
    g.vert = best_vert as f32;
    g.dist_k = best_k;
    g.size_k = 0.85 + rnd.next_f32() * 0.35;
    g.tilt = (rnd.next_f32() - 0.5) * 0.22;
}

fn finish_group(g: &mut Group, metrics: &dyn TextMetrics) {
    let space = metrics.width(" ").max(2.0);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row: Vec<usize> = Vec::new();
    let mut cur = 0.0f32;

    for (idx, w) in g.words.iter_mut().enumerate() {
        w.w = metrics.width(&w.text).max(1.0);
        w.glyphs = build_glyphs(&w.text, metrics);
        if !row.is_empty() && cur + space + w.w > MAX_ROW_W {
            rows.push(row);
            row = Vec::new();
            cur = 0.0;
        }
        w.x = cur;
        cur += w.w + space;
        row.push(idx);
    }
    if !row.is_empty() {
        rows.push(row);
    }

    let num_rows = rows.len().max(1);
    for (r, r_indices) in rows.iter().enumerate() {
        if let Some(&last_i) = r_indices.last() {
            let width = g.words[last_i].x + g.words[last_i].w;
            for &wi in r_indices {
                g.words[wi].x -= width / 2.0;
                g.words[wi].y = (r as f32 - (num_rows as f32 - 1.0) / 2.0) * ROW_H;
            }
        }
    }
    g.rows = num_rows;
    g.min_x = f32::MAX;
    g.max_x = -f32::MAX;
    for w in &g.words {
        g.min_x = g.min_x.min(w.x);
        g.max_x = g.max_x.max(w.x + w.w);
    }
    if g.words.is_empty() {
        g.min_x = 0.0;
        g.max_x = 0.0;
    }

    let seed = g.index.wrapping_mul(131) as u32;
    for (i, w) in g.words.iter_mut().enumerate() {
        let h = seed
            .wrapping_add((i as u32).wrapping_mul(7919))
            .wrapping_add(17)
            .wrapping_mul(0x9E37_79B1);
        w.sx = (((h >> 3) & 63) as f32) - 32.0;
        w.sy = 14.0 + (((h >> 9) & 31) as f32);
        w.sz = -(40.0 + (((h >> 14) & 31) as f32));
        g.end_rel = g.end_rel.max(w.start_rel + w.dur_rel);
    }
    let h = (g.index as u32).wrapping_mul(2654435).wrapping_add(12345);
    g.lateral = ((((h >> 4) & 0xFF) % 5) as i32 - 2) as f32 * 0.16;
    if g.vert == 0.0 {
        g.vert = ((((h >> 12) & 0xFF) % 3) as i32 - 1) as f32 * 0.18;
    }
}

fn build_glyphs(text: &str, metrics: &dyn TextMetrics) -> Vec<Glyph> {
    let mut out = Vec::new();
    let mut x = 0.0f32;
    for ch in text.chars() {
        let s = ch.to_string();
        let cw = metrics.width(&s).max(1.0);
        out.push(Glyph {
            ch: s,
            w: cw,
            x: x + cw / 2.0,
        });
        x += cw;
    }
    out
}

fn target_pos(
    g: &Group,
    cam: [f64; 3],
    yaw: f64,
    cfg: &HudConfig,
    cinema_pitch: Option<f64>,
) -> [f64; 3] {
    let dist = f64::from(cfg.world_distance) * g.dist_k;
    let ang = yaw + g.yaw_off;
    let fx = -ang.sin();
    let fz = ang.cos();
    let rx = -ang.cos();
    let rz = -ang.sin();
    let lat = f64::from(g.lateral) * dist * (f64::from(cfg.world_spread) / 57.0);
    if let Some(pitch_rad) = cinema_pitch {
        let cos_p = pitch_rad.cos();
        let sin_p = pitch_rad.sin();
        let hdist = if g.fixed_yaw { dist } else { dist * cos_p };
        [
            cam[0] + fx * hdist + rx * lat,
            cam[1] - dist * sin_p + f64::from(cfg.world_height + g.vert),
            cam[2] + fz * hdist + rz * lat,
        ]
    } else {
        [
            cam[0] + fx * dist + rx * lat,
            cam[1] + f64::from(cfg.world_height + g.vert),
            cam[2] + fz * dist + rz * lat,
        ]
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_group(
    g: &mut Group,
    rel: i64,
    stack_lift: f32,
    cam: [f64; 3],
    yaw: f64,
    cinema_pitch: Option<f64>,
    cfg: &HudConfig,
    dt: f32,
    wall: f64,
    pause_k: f32,
    probe: &dyn WorldBlockProbe,
    debris: &mut Vec<Debris>,
    out: &mut Vec<WorldTextCmd>,
) -> bool {
    if rel < 0 {
        return false;
    }
    if !g.placed {
        g.spawn_yaw = yaw;
    }
    let t = target_pos(
        g,
        cam,
        if g.fixed_yaw { g.spawn_yaw } else { yaw },
        cfg,
        cinema_pitch,
    );
    if !g.placed {
        g.px = t[0];
        g.py = t[1];
        g.pz = t[2];
        g.placed = true;
    } else if g.follow {
        let a = 1.0 - (-f64::from(dt) * if g.scatter { 12.0 } else { 7.0 }).exp();
        let ay = 1.0 - (-f64::from(dt) * 2.6).exp();
        g.px += (t[0] - g.px) * a;
        g.py += (t[1] - g.py) * ay;
        g.pz += (t[2] - g.pz) * a;
    }

    if cfg.world_exit != WorldExit::Fade && rel >= g.end_rel + g.hold {
        if !g.shattered {
            g.shattered = true;
            spawn_debris(g, cam, cfg, wall, probe, debris);
        }
        return true;
    }

    let now = now_ns();
    if now.saturating_sub(g.occ_checked_at) >= OCCLUSION_CHECK_NS {
        g.occ_checked_at = now;
        g.occluded = probe.is_translucent_occluded(cam, [g.px, g.py, g.pz]);
    }
    let see_now = cfg.world_see_through || g.occluded;

    let exit = clamp01((rel - g.end_rel - g.hold) as f32 / FADE_MS as f32);
    let mut fade = 1.0 - exit * exit;
    if g.timed {
        fade *= pause_k * pause_k * (3.0 - 2.0 * pause_k);
    }
    if fade <= 0.02 {
        return false;
    }

    if !g.lift_init {
        g.lift_cur = stack_lift;
        g.lift_init = true;
    } else {
        g.lift_cur += (stack_lift - g.lift_cur) * (1.0 - (-dt * 6.0).exp());
    }

    let lift = g.lift_cur - 14.0 * ease_out_cubic(exit);
    let scale = BASE_SCALE * cfg.world_scale * g.size_k;
    let dx = cam[0] - g.px;
    let dy = cam[1] - g.py;
    let dz = cam[2] - g.pz;
    let horiz = (dx * dx + dz * dz).sqrt();
    let face = if horiz > 1e-3 {
        dx.atan2(dz) as f32
    } else {
        (std::f64::consts::PI - yaw) as f32
    };
    let flat_pitch = if cinema_pitch.is_some() {
        (-(dy as f32)).atan2(horiz as f32)
    } else {
        0.0
    };
    let roll = if g.scatter {
        g.tilt + 0.05 * ((wall * 1.1 + f64::from(g.index) * 0.9).sin() as f32)
    } else {
        0.0
    };
    let letters = cfg.world_unit == WorldUnit::Letters;
    let speed = cfg.world_anim_speed as f32 / 100.0;
    let bounce = cfg.world_bounce as f32 / 100.0;
    let mut glyph_idx = 0usize;

    for (i, w) in g.words.iter().enumerate() {
        let tw = rel - w.start_rel;
        if tw < 0 {
            continue;
        }
        let sung_now = tw < w.dur_rel;
        let sweep = if sung_now {
            clamp01(tw as f32 / w.dur_rel.max(1) as f32)
        } else {
            1.0
        };
        let pulse = if sung_now {
            1.0 + 0.09 * (std::f32::consts::PI * tw as f32 / w.dur_rel.max(1) as f32).sin()
        } else {
            1.0
        };
        let base_x = w.x;
        let base_y = w.y + lift;

        if !letters {
            let p = clamp01(tw as f32 * speed / FLY_MS);
            let bob = (wall * 2.2 + f64::from(g.index) * 1.3 + i as f64 * 0.7).sin() as f32
                * 1.3
                * bounce;
            let cx = base_x + w.w / 2.0;
            let rgb = word_color(cfg, g, cx, glyph_idx, sung_now, wall);
            emit_piece(
                out,
                cfg,
                [g.px, g.py, g.pz],
                face,
                flat_pitch,
                roll,
                scale,
                &w.text,
                w.w,
                cx,
                base_y + bob * ease_out_cubic(p),
                0.0,
                p,
                w,
                i,
                g.index,
                wall,
                pulse,
                rgb,
                fade,
                sung_now,
                see_now,
            );
            glyph_idx += w.glyphs.len();
        } else {
            let n = w.glyphs.len();
            let step_ms = if n > 1 {
                (cfg.world_letter_ms as i64).min((w.dur_rel / n as i64).max(15))
            } else {
                0
            };
            for (j, gl) in w.glyphs.iter().enumerate() {
                let tl = tw - (j as i64) * step_ms;
                if tl >= 0 {
                    let p = clamp01(tl as f32 * speed / FLY_MS);
                    let lit = sung_now && (j as f32) < (sweep * n as f32).ceil();
                    let cx = base_x + gl.x;
                    let lbob =
                        (wall * 2.2 + f64::from(g.index) * 1.3 + i as f64 * 0.7 + j as f64 * 0.25)
                            .sin() as f32
                            * 1.3
                            * bounce;
                    let rgb = word_color(cfg, g, cx, glyph_idx + j, lit, wall);
                    emit_piece(
                        out,
                        cfg,
                        [g.px, g.py, g.pz],
                        face,
                        flat_pitch,
                        roll,
                        scale,
                        &gl.ch,
                        gl.w,
                        cx,
                        base_y + lbob * ease_out_cubic(p),
                        0.0,
                        p,
                        w,
                        j,
                        g.index.wrapping_mul(31) + i as i32,
                        wall,
                        pulse,
                        rgb,
                        fade,
                        lit,
                        see_now,
                    );
                }
            }
            glyph_idx += n;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn emit_piece(
    out: &mut Vec<WorldTextCmd>,
    cfg: &HudConfig,
    origin: [f64; 3],
    yaw: f32,
    flat_pitch: f32,
    roll: f32,
    base_scale: f32,
    text: &str,
    width: f32,
    mut cx: f32,
    mut cy: f32,
    mut cz: f32,
    p: f32,
    w: &Word,
    j: usize,
    seed: i32,
    wall: f64,
    pulse: f32,
    mut rgb: u32,
    fade: f32,
    lit: bool,
    see_now: bool,
) {
    let e = ease_out_cubic(p);
    let mut alpha = (p * 1.8).min(1.0) * fade;
    let mut k = pulse;
    let mut rot_y = 0.0f32;
    let mut rot_z = 0.0f32;

    match cfg.world_anim {
        WorldAnim::Fly => {
            cx += w.sx * (1.0 - e) * 0.5;
            cy += w.sy * (1.0 - e);
            cz += w.sz * (1.0 - e);
            k *= 0.55 + 0.45 * ease_out_back(p);
        }
        WorldAnim::Rise => cy += 11.0 * (1.0 - ease_out_back(p)),
        WorldAnim::Drop => {
            cy -= 14.0 * (1.0 - ease_out_bounce(p));
            alpha = (p * 4.0).min(1.0) * fade;
        }
        WorldAnim::Pop => k *= ease_out_back(p).max(0.0),
        WorldAnim::Wave => {
            cy += 9.0 * (1.0 - e)
                + (wall as f32 * 5.0 - j as f32 * 0.55 - seed as f32 * 0.3).sin()
                    * 1.8
                    * (cfg.world_bounce as f32 / 100.0)
                    * e;
        }
        WorldAnim::Type => {
            alpha = if p > 0.05 { 1.0 } else { 0.0 } * fade;
            k *= if p < 0.25 { 1.25 - p } else { 1.0 };
        }
        WorldAnim::Spin => {
            rot_y = std::f32::consts::FRAC_PI_2 * (1.0 - ease_out_back(p));
            k *= 0.7 + 0.3 * e;
        }
        WorldAnim::Glitch => {
            let jitter = (1.0 - e) * 4.0;
            let h = (seed as u32)
                .wrapping_mul(73)
                .wrapping_add((j as u32).wrapping_mul(151))
                .wrapping_add((wall * 30.0) as u32)
                .wrapping_mul(0x9E37_79B1);
            cx += ((((h >> 3) & 15) as f32 - 7.5) / 7.5) * jitter;
            cy += ((((h >> 9) & 15) as f32 - 7.5) / 7.5) * jitter;
            if p < 0.6 && ((h >> 20) & 3) == 0 {
                rgb = (!rgb) & 0x00FF_FFFF;
            }
            rot_z = ((h & 7) as f32 - 3.5) * 0.04 * (1.0 - e);
        }
    }

    if alpha < 0.04 || k <= 0.01 {
        return;
    }

    let do_glow = cfg.world_glow == WorldGlow::All || (cfg.world_glow == WorldGlow::Sung && lit);
    let glow = if do_glow && cfg.world_glow_strength > 0 {
        let glow_rgb = lerp_rgb(rgb, 0xFF_FFFF, 0.2);
        let inner = (alpha * cfg.world_glow_strength as f32 * 0.9)
            .round()
            .clamp(0.0, 255.0) as u8;
        let outer = (alpha * cfg.world_glow_strength as f32 * 0.4)
            .round()
            .clamp(0.0, 255.0) as u8;
        Some(GlowSpec {
            rgb: glow_rgb,
            inner_alpha: inner,
            outer_alpha: outer,
            radius: cfg.world_glow_size as f32 / 100.0,
            enhanced: cfg.world_enhanced_glow,
        })
    } else {
        None
    };

    let a255 = (alpha * 255.0).round().clamp(0.0, 255.0) as u32;
    out.push(WorldTextCmd {
        origin,
        yaw,
        flat_pitch,
        roll,
        base_scale,
        local_pos: [cx, cy, cz],
        piece_rot_y: rot_y,
        piece_rot_z: rot_z,
        piece_scale: k,
        text: text.to_owned(),
        width,
        argb: (a255 << 24) | (rgb & 0x00FF_FFFF),
        shadow: cfg.world_shadow && !do_glow,
        see_through: see_now,
        glow,
    });
}

fn spawn_debris(
    g: &Group,
    cam: [f64; 3],
    cfg: &HudConfig,
    wall: f64,
    probe: &dyn WorldBlockProbe,
    debris: &mut Vec<Debris>,
) {
    if !g.placed {
        return;
    }
    let s = BASE_SCALE * cfg.world_scale * g.size_k;
    let face = (cam[0] - g.px).atan2(cam[2] - g.pz) as f32;
    let cos = f64::from(face).cos();
    let sin = f64::from(face).sin();
    let mut rnd = SimpleRng::new((g.index as i64 * 4447 + g.line_ms) as u64);
    let stagger = cfg.world_letter_ms as f32 / 1000.0;
    let span_x = (g.max_x - g.min_x).max(1.0);
    let mut k = 0usize;
    let mut glyph_idx = 0usize;

    for w in &g.words {
        for gl in &w.glyphs {
            let lx = w.x + gl.x;
            let ly = w.y + g.lift_cur;
            let dx = g.px + cos * f64::from(lx * s);
            let dy = g.py - f64::from(ly * s);
            let dz = g.pz - sin * f64::from(lx * s);
            let ground = probe.ground_below(dx, dy, dz);
            let rgb = word_color(cfg, g, lx, glyph_idx, false, wall);
            glyph_idx += 1;

            let mut d = Debris {
                ch: gl.ch.clone(),
                w: gl.w,
                s,
                mode: cfg.world_exit,
                x: dx,
                y: dy,
                z: dz,
                yaw: face,
                rest_yaw: face + (rnd.next_f32() - 0.5) * 0.9,
                ground,
                vx: 0.0,
                vy: 0.0,
                vz: 0.0,
                roll: 0.0,
                roll_v: 0.0,
                phase: 0.0,
                delay: k as f32 * stagger,
                age: 0.0,
                landed: false,
                bounced: false,
                landed_at: 0.0,
                flat: 0.0,
                rgb,
            };
            match d.mode {
                WorldExit::Fall => {
                    d.vx = (rnd.next_f32() - 0.5) * 0.5;
                    d.vy = 0.4 + rnd.next_f32() * 0.8;
                    d.vz = (rnd.next_f32() - 0.5) * 0.5;
                    d.roll_v = (rnd.next_f32() - 0.5) * 9.0;
                }
                WorldExit::Shatter => {
                    let ox = d.x - (g.px + cos * f64::from((g.min_x + span_x / 2.0) * s));
                    let oz = d.z - (g.pz - sin * f64::from((g.min_x + span_x / 2.0) * s));
                    let len = (ox * ox + oz * oz).sqrt().max(0.05);
                    let power = 1.6 + rnd.next_f32() * 2.2;
                    d.vx = (ox / len * f64::from(power)) as f32
                        + ((cam[0] - g.px) * 0.25 * rnd.next_f64()) as f32;
                    d.vz = (oz / len * f64::from(power)) as f32
                        + ((cam[2] - g.pz) * 0.25 * rnd.next_f64()) as f32;
                    d.vy = 2.0 + rnd.next_f32() * 2.5;
                    d.roll_v = (rnd.next_f32() - 0.5) * 22.0;
                }
                WorldExit::Float => {
                    d.delay = k as f32 * 0.02;
                    d.vy = 0.45 + rnd.next_f32() * 0.5;
                    d.phase = rnd.next_f32() * std::f32::consts::TAU;
                }
                WorldExit::Wind => {
                    d.delay = ((lx - g.min_x) / span_x).clamp(0.0, 1.0) * 0.45;
                    let push = 2.8 + rnd.next_f32() * 2.4;
                    d.vx = cos as f32 * push;
                    d.vz = -sin as f32 * push;
                    d.vy = 0.4 + rnd.next_f32() * 0.7;
                    d.roll_v = (rnd.next_f32() - 0.3) * 14.0;
                    d.phase = rnd.next_f32() * std::f32::consts::TAU;
                }
                _ => {}
            }
            debris.push(d);
            k += 1;
        }
    }
    while debris.len() > MAX_DEBRIS {
        debris.remove(0);
    }
}

fn word_color(cfg: &HudConfig, g: &Group, x: f32, index: usize, lit: bool, wall: f64) -> u32 {
    let a = cfg.world_accent.rgb();
    match cfg.world_color {
        WorldColor::Accent => {
            if lit {
                lerp_rgb(a, 0xFF_FFFF, 0.5)
            } else {
                a
            }
        }
        WorldColor::Gradient => {
            let t = if g.max_x > g.min_x {
                clamp01((x - g.min_x) / (g.max_x - g.min_x))
            } else {
                0.5
            };
            let c = lerp_rgb(a, cfg.world_accent2.rgb(), t);
            if lit { lerp_rgb(c, 0xFF_FFFF, 0.45) } else { c }
        }
        WorldColor::Rainbow => {
            let hue = ((index as f32 * 0.035 + wall as f32 * 0.12) % 1.0 + 1.0) % 1.0;
            crate::ui::cover::hsv_to_rgb(hue, if lit { 0.35 } else { 0.65 }, 1.0)
        }
        WorldColor::White => {
            if lit {
                lerp_rgb(0xFF_FFFF, a, 0.25)
            } else {
                0xFF_FFFF
            }
        }
        WorldColor::Classic => {
            if lit {
                cfg.accent_rgb()
            } else {
                0xFF_FFFF
            }
        }
    }
}

fn lerp_rgb(from: u32, to: u32, t: f32) -> u32 {
    crate::ui::PlayerTheme::mix(from, to, t)
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - clamp01(t);
    1.0 - u * u * u
}

fn ease_out_back(t: f32) -> f32 {
    let u = clamp01(t) - 1.0;
    1.0 + 2.70158 * u * u * u + 1.70158 * u * u
}

fn ease_out_bounce(t: f32) -> f32 {
    let mut t = clamp01(t);
    let n = 7.5625;
    let d = 2.75;
    if t < 1.0 / d {
        n * t * t
    } else if t < 2.0 / d {
        t -= 1.5 / d;
        n * t * t + 0.75
    } else if t < 2.5 / d {
        t -= 2.25 / d;
        n * t * t + 0.9375
    } else {
        t -= 2.625 / d;
        n * t * t + 0.984375
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MonoMetrics;

    struct FlatFloorProbe;
    impl WorldBlockProbe for FlatFloorProbe {
        fn ground_below(&self, _x: f64, _y: f64, _z: f64) -> f64 {
            63.0
        }
        fn is_translucent_occluded(&self, _cam: [f64; 3], _target: [f64; 3]) -> bool {
            false
        }
    }

    #[test]
    fn world_lyrics_emits_3d_commands_and_enhanced_halo_increases_glow_layers() {
        let ctl = PlayerController::new(false);
        let metrics = MonoMetrics;
        let probe = FlatFloorProbe;

        let mut world = WorldLyricsState::new();
        ctl.update_hud_config(|h| {
            h.world_lyrics = true;
            h.world_glow = WorldGlow::All;
            h.world_enhanced_glow = false;
        });
        world.spawn_demo(&ctl.hud_config(), &metrics);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let base_cmds = world.update_and_emit(
            &ctl,
            &metrics,
            &probe,
            [0.0, 66.0, 0.0],
            0.0,
            "minecraft:overworld",
        );
        assert!(!base_cmds.is_empty());
        assert!(
            base_cmds
                .iter()
                .any(|c| c.glow.as_ref().is_some_and(|g| !g.enhanced))
        );

        ctl.update_hud_config(|h| {
            h.world_enhanced_glow = true;
        });
        let enh_cmds = world.update_and_emit(
            &ctl,
            &metrics,
            &probe,
            [0.0, 66.0, 0.0],
            0.0,
            "minecraft:overworld",
        );
        assert!(!enh_cmds.is_empty());
        assert!(enh_cmds.iter().any(|c| {
            c.glow
                .as_ref()
                .is_some_and(|g| g.enhanced && g.outer_alpha > 0)
        }));
    }

    #[test]
    fn world_lyrics_spawns_in_front_and_faces_camera_ccw_for_all_yaws() {
        let ctl = PlayerController::new(false);
        let metrics = MonoMetrics;
        let probe = FlatFloorProbe;
        ctl.update_hud_config(|h| {
            h.world_lyrics = true;
            h.world_mode = WorldMode::Follow;
            h.world_follow_style = FollowStyle::Screen;
            h.world_layout = WorldLayout::Line;
        });

        let cam = [10.0, 66.0, -20.0];
        for yaw_deg in [0.0f32, 45.0, 90.0, 180.0, -90.0, -135.0] {
            let mut world = WorldLyricsState::new();
            world.spawn_demo(&ctl.hud_config(), &metrics);
            std::thread::sleep(std::time::Duration::from_millis(30));
            let cmds =
                world.update_and_emit(&ctl, &metrics, &probe, cam, yaw_deg, "minecraft:overworld");
            assert!(!cmds.is_empty(), "expected world cmds at yaw {yaw_deg}");
            let cmd = &cmds[0];

            // Camera-space rotation matching horizon_client_render::camera::view_matrix(yaw, 0.0)
            let phi = f64::from(yaw_deg).to_radians() - std::f64::consts::PI;
            let to_cam_space = |v: [f64; 3]| -> [f64; 3] {
                [
                    v[0] * phi.cos() + v[2] * phi.sin(),
                    v[1],
                    -v[0] * phi.sin() + v[2] * phi.cos(),
                ]
            };

            let rel_origin = [
                cmd.origin[0] - cam[0],
                cmd.origin[1] - cam[1],
                cmd.origin[2] - cam[2],
            ];
            let cam_origin = to_cam_space(rel_origin);
            assert!(
                cam_origin[2] < -1.0 && cam_origin[0].abs() < 1.5,
                "origin at yaw {yaw_deg} must be directly ahead of camera, got {cam_origin:?}"
            );

            // Transform local font corners TL=(0,0,0), BL=(0,8,0), BR=(w,8,0) through runtime.rs pose
            let transform_font_pt = |fx: f64, fy: f64| -> [f64; 3] {
                let lx = (fx - f64::from(cmd.width) * 0.5) * f64::from(cmd.piece_scale);
                let ly = (fy - 4.0) * f64::from(cmd.piece_scale);
                let px = (lx + f64::from(cmd.local_pos[0])) * f64::from(cmd.base_scale);
                let py = (ly + f64::from(cmd.local_pos[1])) * (-f64::from(cmd.base_scale));
                let pz = f64::from(cmd.local_pos[2]) * f64::from(cmd.base_scale);
                let sy = f64::from(cmd.yaw).sin();
                let cy = f64::from(cmd.yaw).cos();
                let wx = rel_origin[0] + px * cy + pz * sy;
                let wy = rel_origin[1] + py;
                let wz = rel_origin[2] - px * sy + pz * cy;
                to_cam_space([wx, wy, wz])
            };

            let tl = transform_font_pt(0.0, 0.0);
            let bl = transform_font_pt(0.0, 8.0);
            let br = transform_font_pt(f64::from(cmd.width.max(6.0)), 8.0);
            assert!(
                br[0] > bl[0],
                "text must read left-to-right at yaw {yaw_deg}: bl={bl:?}, br={br:?}"
            );
            let e1 = [bl[0] - tl[0], bl[1] - tl[1]];
            let e2 = [br[0] - bl[0], br[1] - bl[1]];
            let cross_z = e1[0] * e2[1] - e1[1] * e2[0];
            assert!(
                cross_z > 0.0,
                "quad must wind CCW (not back-face culled) at yaw {yaw_deg}: cross_z={cross_z}"
            );
        }
    }

    #[test]
    fn cinema_mode_pitches_world_lyrics_toward_overhead_camera_without_changing_normal_mode() {
        let ctl = PlayerController::new(false);
        let metrics = MonoMetrics;
        let probe = FlatFloorProbe;
        ctl.update_hud_config(|h| {
            h.world_lyrics = true;
            h.world_mode = WorldMode::Follow;
            h.world_follow_style = FollowStyle::Screen;
            h.world_layout = WorldLayout::Line;
        });

        let cam = [0.0, 72.0, 0.0];
        let yaw_deg = 30.0f32;
        let pitch_deg = 60.0f32;

        // Normal playback (cam_pitch_deg = None): stays horizontal, flat_pitch == 0.0.
        let mut normal_world = WorldLyricsState::new();
        normal_world.spawn_demo(&ctl.hud_config(), &metrics);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let normal_cmds = normal_world.update_and_emit_view(
            &ctl,
            &metrics,
            &probe,
            cam,
            yaw_deg,
            None,
            "minecraft:overworld",
        );
        assert!(!normal_cmds.is_empty());
        assert_eq!(normal_cmds[0].flat_pitch, 0.0);

        // Cinema mode (cam_pitch_deg = Some(60.0) looking down): places lyrics ahead in 3D view and pitches upward toward camera.
        let mut cine_world = WorldLyricsState::new();
        cine_world.spawn_demo(&ctl.hud_config(), &metrics);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let cine_cmds = cine_world.update_and_emit_view(
            &ctl,
            &metrics,
            &probe,
            cam,
            yaw_deg,
            Some(pitch_deg),
            "minecraft:overworld",
        );
        assert!(!cine_cmds.is_empty());
        let cmd = &cine_cmds[0];
        assert!(
            cmd.origin[1] < cam[1] - 2.0,
            "cinema lyrics must drop into overhead view cone, origin={:?} cam={cam:?}",
            cmd.origin
        );
        assert!(
            cmd.flat_pitch < -0.5,
            "cinema lyrics must tilt up toward overhead camera, flat_pitch={}",
            cmd.flat_pitch
        );

        // Verify full 3D view alignment (`yaw` then `flat_pitch` around +X): local +Z normal points directly at `cam`.
        let cy = f64::from(cmd.yaw).cos();
        let sy = f64::from(cmd.yaw).sin();
        let sp = f64::from(cmd.flat_pitch).sin();
        let cp = f64::from(cmd.flat_pitch).cos();
        // Rotate (0, 0, 1) by Rx(flat_pitch) -> (0, -sp, cp), then by Ry(yaw) -> (cp*sy, -sp, cp*cy)
        let nx = cp * sy;
        let ny = -sp;
        let nz = cp * cy;
        let vx = cam[0] - cmd.origin[0];
        let vy = cam[1] - cmd.origin[1];
        let vz = cam[2] - cmd.origin[2];
        let vlen = (vx * vx + vy * vy + vz * vz).sqrt();
        let dot = (nx * vx + ny * vy + nz * vz) / vlen;
        assert!(
            dot > 0.99,
            "3D lyric normal must point straight at overhead camera (dot={dot})"
        );
    }

    #[test]
    fn world_words_match_full_line_duration_and_shift_sign() {
        let metrics = MonoMetrics;
        let line0 = LyricLine {
            time_ms: 10_000,
            text: "First sample phrase here".to_string(),
            words: Vec::new(),
        };
        let line1 = LyricLine {
            time_ms: 16_000,
            text: "Second sample line".to_string(),
            words: Vec::new(),
        };
        let g = from_line(0, &line0, Some(&line1), true, false, &metrics);
        assert_eq!(g.words.len(), 4);
        // Word 0 starts at 0; final word must not start prematurely in the first 2 seconds
        // of a 6-second line (it must sweep proportionally with HUD Subtitles).
        assert_eq!(g.words[0].start_rel, 0);
        assert!(
            g.words[3].start_rel >= 3_000,
            "last word rushed ahead: start_rel={}",
            g.words[3].start_rel
        );
        let last = g.words.last().unwrap();
        assert!(
            (last.start_rel + last.dur_rel) >= 5_800,
            "words must span full 6000ms line duration, ended at {}",
            last.start_rel + last.dur_rel
        );

        let ctl = PlayerController::new(false);
        ctl.add_track_shift_ms("t1", -1500);
        assert_eq!(ctl.adjusted_lyrics_pos_ms(10_000, "t1", 0), 11_500);
        ctl.reset_track_shift_ms("t1");
        ctl.add_track_shift_ms("t1", 1500);
        assert_eq!(ctl.adjusted_lyrics_pos_ms(10_000, "t1", 500), 8_000);
    }
}
