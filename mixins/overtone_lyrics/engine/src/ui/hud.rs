use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{
    Anchor, HudConfig, Layout, LyricsEnter, LyricsMode, ShowMode, VizColor, VizDirection, VizStyle,
    VolumeStyle, WorldExit,
};
use crate::controller::PlayerController;
use crate::i18n;
use crate::lyrics::{self, LyricLine, WordTiming};
use crate::model::Track;
use crate::ui::cover::CoverArtManager;
use crate::ui::{DrawList, PlayerTheme, TexSlot, TextMetrics};

const GRAVITY_2D: f32 = 650.0;
const FADE_IN_MS: i64 = 180;
const FADE_OUT_MS: i64 = 450;
const BIT_MAX: usize = 600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HudBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl HudBox {
    pub fn contains(self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.w && my >= self.y && my < self.y + self.h
    }
}

#[derive(Clone, Debug, Default)]
pub struct HudBounds {
    pub widget: Option<HudBox>,
    pub lyrics: Option<HudBox>,
    pub volume: Option<HudBox>,
    pub viz: Option<HudBox>,
}

pub fn anchor_hv(a: Anchor) -> (i32, i32) {
    match a {
        Anchor::TopLeft => (-1, -1),
        Anchor::TopCenter => (0, -1),
        Anchor::TopRight => (1, -1),
        Anchor::MidLeft => (-1, 0),
        Anchor::Center => (0, 0),
        Anchor::MidRight => (1, 0),
        Anchor::BottomLeft => (-1, 1),
        Anchor::BottomCenter => (0, 1),
        Anchor::BottomRight => (1, 1),
    }
}

pub fn compute_anchored_pos(
    anchor: Anchor,
    w: i32,
    h: i32,
    ox: i32,
    oy: i32,
    sw: i32,
    sh: i32,
) -> (i32, i32) {
    let (ah, av) = anchor_hv(anchor);
    let x = match ah {
        -1 => ox,
        0 => (sw - w) / 2 + ox,
        _ => sw - w - ox,
    };
    let y = match av {
        -1 => oy,
        0 => (sh - h) / 2 + oy,
        _ => sh - h - oy,
    };
    (x, y)
}

impl HudBounds {
    /// Converts a dragged top-left `(nx, ny)` into anchor-relative `(ox, oy)`.
    pub fn place_offset(
        anchor: Anchor,
        w: i32,
        h: i32,
        sw: i32,
        sh: i32,
        nx: i32,
        ny: i32,
    ) -> (i32, i32) {
        let (ah, av) = anchor_hv(anchor);
        let ox = match ah {
            -1 => nx,
            0 => nx + w / 2 - sw / 2,
            _ => sw - w - nx,
        };
        let oy = match av {
            -1 => ny,
            0 => ny + h / 2 - sh / 2,
            _ => sh - h - ny,
        };
        (ox, oy)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn panel_bg(g: &mut DrawList, x: f32, y: f32, w: f32, h: f32, opacity_pct: i32, alpha_k: f32) {
    let a = ((opacity_pct.clamp(0, 100) as f32 / 100.0) * 210.0 * alpha_k).round() as i32;
    if a > 2 {
        g.rect(x, y, w, h, PlayerTheme::argb(0x0D_0D11, a));
    }
}

#[allow(clippy::too_many_arguments)]
fn panel_progress(
    g: &mut DrawList,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    frac: f32,
    accent_rgb: u32,
    alpha_k: f32,
) {
    let f = frac.clamp(0.0, 1.0);
    g.rect(
        x,
        y,
        w,
        h,
        PlayerTheme::argb(0xFF_FFFF, (36.0 * alpha_k) as i32),
    );
    let fw = w * f;
    if fw > 0.5 {
        g.rect(
            x,
            y,
            fw,
            h,
            PlayerTheme::argb(accent_rgb, (240.0 * alpha_k) as i32),
        );
    }
}

fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Clone, Debug)]
struct CrumbleBit {
    ch: String,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    color: u32,
    age: f32,
    life: f32,
}

#[derive(Default)]
pub struct HudState {
    // Widget fade tracking
    last_track_id: String,
    track_changed_ms: i64,
    last_near_end: bool,
    near_end_ms: i64,
    // 2D lyrics crumble + transition state
    prev_idx: i32,
    prev_track_id: String,
    prev_line_text: String,
    prev_line_x: f32,
    prev_line_y: f32,
    prev_line_color: u32,
    entered_ms: i64,
    crumble: Vec<CrumbleBit>,
    crumble_rng: u64,
    // Volume popup timer
    last_vol: f32,
    vol_shown_ms: i64,
    vol_init: bool,
    // Cinema bars smoothing
    cine_bars_anim: f32,
    // Peak hold cache per bar
    viz_peaks: Vec<f32>,
    viz_peak_vel: Vec<f32>,
    // Resolved bounds for HudEditScreen
    pub bounds: HudBounds,
}

impl HudState {
    pub fn new() -> Self {
        Self {
            prev_idx: -2,
            crumble_rng: 0x5EED_CAFE_1234_5678,
            last_vol: -1.0,
            ..Self::default()
        }
    }

    fn next_rand(&mut self) -> f32 {
        let mut x = self.crumble_rng;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.crumble_rng = x;
        ((x >> 40) as f32) / ((1u64 << 24) as f32)
    }

    pub fn notify_volume_changed(&mut self, vol: f32) {
        self.last_vol = vol;
        self.vol_shown_ms = now_ms();
        self.vol_init = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_all(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        sw: i32,
        sh: i32,
        dt: f32,
        preview: bool,
        tab_pressed: bool,
        hud_hidden: bool,
        cinema_active: bool,
        has_armor: bool,
    ) {
        self.bounds = HudBounds::default();
        let cfg = ctl.hud_config();

        // Cinema 16:9 black bars draw even if HUD is hidden (unless user disabled cine_bars)
        let want_bars = cinema_active && cfg.cine_bars && !preview;
        let target_bar = if want_bars { 1.0 } else { 0.0 };
        let step = 1.0 - (-dt * 8.0).exp();
        self.cine_bars_anim += (target_bar - self.cine_bars_anim) * step;
        if self.cine_bars_anim > 0.005 {
            let target_h = ((sh as f32 - sw as f32 * 9.0 / 16.0) / 2.0).max(0.0);
            let bar_h = (target_h * self.cine_bars_anim).round();
            if bar_h > 0.0 {
                g.rect(0.0, 0.0, sw as f32, bar_h, 0xFF00_0000);
                g.rect(0.0, sh as f32 - bar_h, sw as f32, bar_h, 0xFF00_0000);
            }
        }

        if !preview {
            if !cfg.master {
                return;
            }
            if cfg.hide_with_gui && hud_hidden {
                return;
            }
            if cfg.hide_on_tab && tab_pressed {
                return;
            }
            if cinema_active && !cfg.cine_keep_music_hud {
                return;
            }
        }

        self.render_widget(g, metrics, ctl, covers, &cfg, sw, sh, preview, has_armor);
        if !cfg.world_lyrics || cfg.lyrics_with_world || preview {
            self.render_lyrics(g, metrics, ctl, &cfg, sw, sh, dt, preview);
        }
        self.render_volume(g, metrics, ctl, &cfg, sw, sh, preview);
        self.render_visualizer(g, ctl, &cfg, sw, sh, dt, preview);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_widget(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        cfg: &HudConfig,
        sw: i32,
        sh: i32,
        preview: bool,
        has_armor: bool,
    ) {
        if !cfg.enabled && !preview {
            return;
        }
        let current = ctl.current_track();
        let demo_track;
        let track: &Track = if let Some(t) = current.as_ref() {
            t
        } else if preview {
            demo_track = Track {
                id: "demo".to_owned(),
                title: i18n::raw("demo.title"),
                artist: i18n::raw("demo.artist"),
                duration_ms: 210_000,
                ..Default::default()
            };
            &demo_track
        } else {
            return;
        };

        let now = now_ms();
        if track.id != self.last_track_id {
            self.last_track_id = track.id.clone();
            self.track_changed_ms = now;
            self.last_near_end = false;
        }
        let dur_ms = track.duration_ms.max(1);
        let pos_ms = if current.is_none() && preview {
            84_000
        } else {
            ctl.position_ms().min(dur_ms)
        };
        let playing = ctl.is_playing() || (current.is_none() && preview);
        let near_end = playing && dur_ms > 20_000 && dur_ms.saturating_sub(pos_ms) <= 10_000;
        if near_end && !self.last_near_end {
            self.near_end_ms = now;
        }
        self.last_near_end = near_end;

        let alpha = if preview {
            1.0
        } else {
            widget_visibility_alpha(cfg, playing, now, self.track_changed_ms, self.near_end_ms)
        };
        if alpha <= 0.02 {
            return;
        }

        let frac = pos_ms as f32 / dur_ms as f32;
        let accent = cfg.accent_rgb() & 0x00FF_FFFF;
        let secs = now as f64 / 1000.0;
        let has_cover = cfg.show_cover && {
            if let Some(url) = track.cover_url.as_deref() {
                covers.offer_hud(&track.id, Some(url));
            }
            covers.get_hud(&track.id).is_some()
        };

        let adj_pos_ms = if let Some(ref tr) = current {
            ctl.adjusted_lyrics_pos_ms(pos_ms, &tr.id, cfg.lyrics_shift_ms)
        } else {
            pos_ms
        };
        let line = if cfg.card_lyrics && !preview {
            active_synced_text(ctl, adj_pos_ms).unwrap_or_default()
        } else {
            String::new()
        };
        let extra_h = if !line.is_empty() { 13 } else { 0 };
        let lh = metrics.line_height().round() as i32;

        let (base_w, base_h) = match cfg.layout {
            Layout::Card => (208, (if has_cover { 48 } else { 42 }) + extra_h),
            Layout::Classic => (214, (if has_cover { 44 } else { 38 }) + extra_h),
            Layout::Bar => (340, 26 + extra_h),
            Layout::Compact => (190, 26 + extra_h),
            Layout::Hotbar => {
                let w = 182;
                let bar = sh - 22;
                let hx = (sw - w) / 2;
                let mut hy = bar - 26;
                if has_armor {
                    hy -= 10;
                }
                if cfg.layout == Layout::Hotbar {
                    g.rect(
                        hx as f32,
                        hy as f32,
                        w as f32,
                        18.0,
                        PlayerTheme::argb(0x0D_0D11, (195.0 * alpha) as i32),
                    );
                    let title = metrics.ellipsize(
                        &format!("{} — {}", track.artist, track.title),
                        (w - 16) as f32,
                    );
                    g.text(
                        title,
                        (hx + 6) as f32,
                        (hy + 4) as f32,
                        PlayerTheme::argb(0xFF_FFFF, (255.0 * alpha) as i32),
                        true,
                    );
                    panel_progress(
                        g,
                        (hx + 4) as f32,
                        (hy + 15) as f32,
                        (w - 8) as f32,
                        2.0,
                        frac,
                        accent,
                        alpha,
                    );
                    self.bounds.widget = Some(HudBox {
                        x: hx,
                        y: hy,
                        w,
                        h: 18,
                    });
                }
                return;
            }
            Layout::Vanilla => {
                let title = format!("♪ {} — {}", track.artist, track.title);
                let tw = (metrics.width(&title) + 12.0).max(120.0) as i32;
                (tw.min(260), 22 + extra_h)
            }
        };

        let scale = cfg.scale.clamp(0.5, 2.0);
        let sw_scaled = (base_w as f32 * scale).round() as i32;
        let sh_scaled = (base_h as f32 * scale).round() as i32;
        let (x, y) = compute_anchored_pos(
            cfg.anchor,
            sw_scaled,
            sh_scaled,
            cfg.offset_x,
            cfg.offset_y,
            sw,
            sh,
        );
        self.bounds.widget = Some(HudBox {
            x,
            y,
            w: sw_scaled,
            h: sh_scaled,
        });

        g.push_translate(x as f32, y as f32);
        g.push_scale(scale);

        let w = base_w as f32;
        let h = base_h as f32;
        let main_col = PlayerTheme::argb(0xFF_FFFF, (255.0 * alpha) as i32);
        let dim_col = PlayerTheme::argb(0xB4_B4C0, (255.0 * alpha) as i32);
        let acc_col = PlayerTheme::argb(accent, (255.0 * alpha) as i32);

        match cfg.layout {
            Layout::Card | Layout::Classic => {
                panel_bg(g, 0.0, 0.0, w, h, cfg.background_opacity, alpha);
                if cfg.layout == Layout::Card {
                    g.rect(0.0, 0.0, 2.5, h, acc_col);
                }
                let cs = if cfg.show_cover {
                    (base_h - extra_h - 10).max(24) as f32
                } else {
                    0.0
                };
                let tx = if cfg.show_cover { 8.0 + cs + 7.0 } else { 8.0 };
                if cfg.show_cover {
                    if has_cover {
                        g.texture(TexSlot::CoverHud, 7.0, 5.0, cs, cs, alpha);
                    } else {
                        g.rect(
                            7.0,
                            5.0,
                            cs,
                            cs,
                            PlayerTheme::argb(0x24_242A, (220.0 * alpha) as i32),
                        );
                        let nw = metrics.width("♪");
                        g.text(
                            "♪",
                            7.0 + (cs - nw) / 2.0,
                            5.0 + (cs - lh as f32) / 2.0,
                            dim_col,
                            true,
                        );
                    }
                }
                let eq_w = if cfg.show_waveform { 14.0 } else { 0.0 };
                let text_w = (w - tx - 8.0 - eq_w).max(40.0);
                g.text(
                    metrics.ellipsize(&track.title, text_w),
                    tx,
                    6.0,
                    main_col,
                    true,
                );
                g.text(
                    metrics.ellipsize(&track.artist, text_w),
                    tx,
                    17.0,
                    dim_col,
                    true,
                );
                if cfg.show_waveform {
                    draw_mini_eq(g, w - 18.0, 7.0, 14, playing, secs, acc_col);
                }
                let bar_y = (base_h - extra_h - 9) as f32;
                let bar_x = if cfg.show_timestamps {
                    let s = fmt_time(pos_ms);
                    g.text(&s, tx, bar_y - 1.0, dim_col, true);
                    tx + metrics.width(&s) + 5.0
                } else {
                    tx
                };
                let bar_end = if cfg.show_timestamps {
                    let d = fmt_time(dur_ms);
                    let dw = metrics.width(&d);
                    g.text(&d, w - 7.0 - dw, bar_y - 1.0, dim_col, true);
                    w - 11.0 - dw
                } else {
                    w - 8.0
                };
                if bar_end > bar_x + 12.0 {
                    panel_progress(
                        g,
                        bar_x,
                        bar_y + 2.0,
                        bar_end - bar_x,
                        2.5,
                        frac,
                        accent,
                        alpha,
                    );
                }
                if !line.is_empty() {
                    let clipped = metrics.ellipsize(&line, w - 16.0);
                    g.text(clipped, 8.0, (base_h - 13) as f32, acc_col, true);
                }
            }
            Layout::Bar => {
                panel_bg(g, 0.0, 0.0, w, h, cfg.background_opacity, alpha);
                let mut tx = 7.0;
                if cfg.show_cover {
                    if has_cover {
                        g.texture(TexSlot::CoverHud, 5.0, 4.0, 18.0, 18.0, alpha);
                    } else {
                        g.rect(
                            5.0,
                            4.0,
                            18.0,
                            18.0,
                            PlayerTheme::argb(0x24_242A, (220.0 * alpha) as i32),
                        );
                        g.text("♪", 11.0, 9.0, dim_col, true);
                    }
                    tx = 28.0;
                }
                let right_pad = if cfg.show_timestamps { 68.0 } else { 10.0 }
                    + if cfg.show_waveform { 16.0 } else { 0.0 };
                let label = format!("{} — {}", track.artist, track.title);
                g.text(
                    metrics.ellipsize(&label, (w - tx - right_pad).max(40.0)),
                    tx,
                    6.0,
                    main_col,
                    true,
                );
                if cfg.show_timestamps {
                    let tm = format!("{} / {}", fmt_time(pos_ms), fmt_time(dur_ms));
                    let tw = metrics.width(&tm);
                    let rx = w - (if cfg.show_waveform { 22.0 } else { 8.0 }) - tw;
                    g.text(tm, rx, 6.0, dim_col, true);
                }
                if cfg.show_waveform {
                    draw_mini_eq(g, w - 17.0, 5.0, 10, playing, secs, acc_col);
                }
                panel_progress(g, tx, 20.0, w - tx - 7.0, 2.0, frac, accent, alpha);
                if !line.is_empty() {
                    g.text(metrics.ellipsize(&line, w - 16.0), 8.0, 26.0, acc_col, true);
                }
            }
            Layout::Compact => {
                panel_bg(g, 0.0, 0.0, w, h, cfg.background_opacity, alpha);
                let mut tx = 7.0;
                if cfg.show_cover {
                    if has_cover {
                        g.texture(TexSlot::CoverHud, 5.0, 4.0, 18.0, 18.0, alpha);
                    } else {
                        g.rect(
                            5.0,
                            4.0,
                            18.0,
                            18.0,
                            PlayerTheme::argb(0x24_242A, (220.0 * alpha) as i32),
                        );
                    }
                    tx = 27.0;
                }
                let eq_w = if cfg.show_waveform { 16.0 } else { 0.0 };
                g.text(
                    metrics.ellipsize(&track.title, (w - tx - 8.0 - eq_w).max(30.0)),
                    tx,
                    6.0,
                    main_col,
                    true,
                );
                if cfg.show_waveform {
                    draw_mini_eq(g, w - 16.0, 5.0, 10, playing, secs, acc_col);
                }
                panel_progress(g, tx, 19.0, w - tx - 7.0, 2.0, frac, accent, alpha);
                if !line.is_empty() {
                    g.text(metrics.ellipsize(&line, w - 14.0), 7.0, 25.0, acc_col, true);
                }
            }
            Layout::Vanilla => {
                panel_bg(
                    g,
                    0.0,
                    0.0,
                    w,
                    h,
                    (cfg.background_opacity / 2).max(20),
                    alpha,
                );
                let title = format!("♪ {} — {}", track.artist, track.title);
                g.text(
                    metrics.ellipsize(&title, w - 12.0),
                    6.0,
                    6.0,
                    main_col,
                    true,
                );
                panel_progress(g, 4.0, 18.0, w - 8.0, 1.5, frac, accent, alpha);
                if !line.is_empty() {
                    g.text(metrics.ellipsize(&line, w - 12.0), 6.0, 22.0, acc_col, true);
                }
            }
            Layout::Hotbar => {}
        }

        g.pop_scale();
        g.pop_translate();
    }

    #[allow(clippy::too_many_arguments)]
    fn render_lyrics(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        cfg: &HudConfig,
        sw: i32,
        sh: i32,
        dt: f32,
        preview: bool,
    ) {
        if cfg.lyrics_mode == LyricsMode::Off && !preview {
            return;
        }
        let mode = if preview && cfg.lyrics_mode == LyricsMode::Off {
            LyricsMode::Subtitle
        } else {
            cfg.lyrics_mode
        };

        let active_cand = ctl.active_lyrics();
        let active_lines = active_cand
            .as_ref()
            .map(|c| c.lines.as_slice())
            .unwrap_or(&[]);
        let demo_lines;
        let lines: &[LyricLine] = if !active_lines.is_empty() {
            active_lines
        } else if preview {
            demo_lines = vec![
                LyricLine {
                    time_ms: 0,
                    text: i18n::raw("demo.lyric_1"),
                    words: Vec::new(),
                },
                LyricLine {
                    time_ms: 3000,
                    text: i18n::raw("demo.lyric_2"),
                    words: Vec::new(),
                },
                LyricLine {
                    time_ms: 6000,
                    text: i18n::raw("demo.lyric_3"),
                    words: Vec::new(),
                },
            ];
            &demo_lines
        } else {
            return;
        };

        if !lines[0].is_synced() {
            // PRO MAX guarantee: never display raw UNSYNCED lines in HUD
            return;
        }

        let current = ctl.current_track();
        let pos = if current.is_none() && preview {
            3_800
        } else {
            let track_id = current.as_ref().map(|t| t.id.as_str()).unwrap_or("");
            ctl.adjusted_lyrics_pos_ms(ctl.position_ms(), track_id, cfg.lyrics_shift_ms)
        };
        let cur = lyrics::current_index(lines, pos);
        let track_id = current.as_ref().map(|t| t.id.as_str()).unwrap_or("demo");
        let now = now_ms();

        // Trigger exit particles / line-enter timer when active line changes
        let cur_idx_i = cur.map(|i| i as i32).unwrap_or(-1);
        if track_id != self.prev_track_id {
            self.prev_track_id = track_id.to_owned();
            self.prev_idx = cur_idx_i;
            self.entered_ms = now;
            self.crumble.clear();
            self.prev_line_text.clear();
        } else if cur_idx_i != self.prev_idx {
            if !self.prev_line_text.is_empty() && cfg.lyrics_exit == WorldExit::Shatter {
                self.spawn_crumble(metrics, cfg.lyrics_scale);
            }
            self.prev_idx = cur_idx_i;
            self.entered_ms = now;
            self.prev_line_text = cur
                .and_then(|i| lines.get(i))
                .map(|l| l.text.clone())
                .unwrap_or_default();
        }

        self.step_crumble(g, dt);

        let scale = cfg.lyrics_scale.clamp(0.5, 2.0);
        let box_w_unscaled = cfg.lyrics_width.max(120) as f32;
        let lh = metrics.line_height() + 3.0;
        let max_lines = cfg.lyrics_lines.clamp(1, 9);
        let box_h_unscaled = match mode {
            LyricsMode::Subtitle => lh + 10.0,
            LyricsMode::Karaoke => lh * max_lines as f32 + 12.0,
            LyricsMode::Off => 20.0,
        };
        let sw_scaled = (box_w_unscaled * scale).round() as i32;
        let sh_scaled = (box_h_unscaled * scale).round() as i32;
        let (x, y) = compute_anchored_pos(
            cfg.lyrics_anchor,
            sw_scaled,
            sh_scaled,
            cfg.lyrics_offset_x,
            cfg.lyrics_offset_y,
            sw,
            sh,
        );
        self.bounds.lyrics = Some(HudBox {
            x,
            y,
            w: sw_scaled,
            h: sh_scaled,
        });

        let accent = cfg.accent_rgb() & 0x00FF_FFFF;
        g.push_translate(x as f32, y as f32);
        g.push_scale(scale);

        match mode {
            LyricsMode::Subtitle => {
                let Some(ci) = cur else {
                    g.pop_scale();
                    g.pop_translate();
                    return;
                };
                let line = &lines[ci];
                let linger_ms = (cfg.lyrics_linger.max(1) as i64) * 1000;
                let elapsed = (now - self.entered_ms).max(0);
                let next_time = lines.get(ci + 1).map(|l| l.time_ms);
                let fade = if !preview && next_time.is_none() && elapsed > linger_ms {
                    (1.0 - (elapsed - linger_ms) as f32 / 600.0).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if fade <= 0.02 {
                    g.pop_scale();
                    g.pop_translate();
                    return;
                }
                let wrapped = metrics.wrap(&line.text, box_w_unscaled - 16.0);
                let mut shown_line = wrapped.first().cloned().unwrap_or_default();
                let line_dur = next_time
                    .map(|nt| nt.saturating_sub(line.time_ms))
                    .unwrap_or(4000)
                    .max(400);
                let line_prog =
                    (pos.saturating_sub(line.time_ms) as f32 / line_dur as f32).clamp(0.0, 1.0);
                let enter_t = if cfg.lyrics_letter_ms <= 0 {
                    1.0
                } else {
                    (elapsed as f32
                        / (cfg.lyrics_letter_ms as f32 * line.text.chars().count().max(1) as f32))
                        .clamp(0.0, 1.0)
                };
                if cfg.lyrics_enter == LyricsEnter::Type && enter_t < 1.0 {
                    let take = ((shown_line.chars().count() as f32) * enter_t).ceil() as usize;
                    shown_line = shown_line.chars().take(take).collect();
                }

                let tw = metrics.width(&shown_line);
                let bg_w = (tw + 16.0).min(box_w_unscaled);
                let bg_x = (box_w_unscaled - bg_w) / 2.0;
                panel_bg(g, bg_x, 0.0, bg_w, lh + 8.0, cfg.lyrics_background, fade);

                let text_x = (box_w_unscaled - tw) / 2.0;
                let mut y_off = 0.0;
                let mut x_off = 0.0;
                let k = (elapsed as f32 / 220.0).clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - k).powi(3);
                match cfg.lyrics_enter {
                    LyricsEnter::Slide => x_off = (1.0 - ease) * 14.0,
                    LyricsEnter::Rise => y_off = (1.0 - ease) * 7.0,
                    LyricsEnter::Pop => {}
                    LyricsEnter::Type => {}
                }

                self.prev_line_x = x as f32 + (text_x + x_off) * scale;
                self.prev_line_y = y as f32 + (5.0 + y_off) * scale;
                self.prev_line_color = if cfg.lyrics_accent {
                    PlayerTheme::argb(accent, (255.0 * fade) as i32)
                } else {
                    PlayerTheme::argb(0xFF_FFFF, (255.0 * fade) as i32)
                };

                if cfg.lyrics_word_karaoke {
                    let word_spans: &[WordTiming] = if shown_line == line.text {
                        &line.words
                    } else {
                        &[]
                    };
                    draw_word_karaoke(
                        g,
                        metrics,
                        &shown_line,
                        word_spans,
                        pos,
                        line_prog,
                        text_x + x_off,
                        5.0 + y_off,
                        accent,
                        cfg.lyrics_accent,
                        fade,
                        cfg.lyrics_shadow,
                    );
                } else {
                    g.text(
                        &shown_line,
                        text_x + x_off,
                        5.0 + y_off,
                        self.prev_line_color,
                        cfg.lyrics_shadow,
                    );
                }
            }
            LyricsMode::Karaoke => {
                panel_bg(
                    g,
                    0.0,
                    0.0,
                    box_w_unscaled,
                    box_h_unscaled,
                    cfg.lyrics_background,
                    1.0,
                );
                let half = max_lines / 2;
                let ci = cur.unwrap_or(0);
                let start = ci.saturating_sub(half as usize);
                for slot in 0..max_lines as usize {
                    let idx = start + slot;
                    if idx >= lines.len() {
                        break;
                    }
                    let line = &lines[idx];
                    let ly = 6.0 + slot as f32 * lh;
                    let is_cur = Some(idx) == cur;
                    let text = metrics.ellipsize(&line.text, box_w_unscaled - 16.0);
                    let tw = metrics.width(&text);
                    let tx = (box_w_unscaled - tw) / 2.0;
                    if is_cur {
                        let next_t = lines.get(idx + 1).map(|l| l.time_ms);
                        let dur = next_t
                            .map(|n| n.saturating_sub(line.time_ms))
                            .unwrap_or(4000)
                            .max(400);
                        let prog =
                            (pos.saturating_sub(line.time_ms) as f32 / dur as f32).clamp(0.0, 1.0);
                        self.prev_line_x = x as f32 + tx * scale;
                        self.prev_line_y = y as f32 + ly * scale;
                        self.prev_line_color = PlayerTheme::argb(
                            if cfg.lyrics_accent { accent } else { 0xFF_FFFF },
                            255,
                        );
                        if cfg.lyrics_word_karaoke {
                            let word_spans: &[WordTiming] =
                                if text == line.text { &line.words } else { &[] };
                            draw_word_karaoke(
                                g,
                                metrics,
                                &text,
                                word_spans,
                                pos,
                                prog,
                                tx,
                                ly,
                                accent,
                                cfg.lyrics_accent,
                                1.0,
                                cfg.lyrics_shadow,
                            );
                        } else {
                            g.text(text, tx, ly, self.prev_line_color, cfg.lyrics_shadow);
                        }
                    } else {
                        let dist = (idx as isize - ci as isize).abs() as f32;
                        let a = (200.0 - dist * 35.0).clamp(65.0, 200.0) as i32;
                        let col = PlayerTheme::argb(0xB4_B4C0, a);
                        g.text(text, tx, ly, col, cfg.lyrics_shadow);
                    }
                }
            }
            LyricsMode::Off => {}
        }

        g.pop_scale();
        g.pop_translate();
    }

    fn spawn_crumble(&mut self, metrics: &dyn TextMetrics, scale: f32) {
        if self.crumble.len() >= BIT_MAX {
            self.crumble.drain(..(self.crumble.len() - BIT_MAX / 2));
        }
        let mut cursor_x = self.prev_line_x;
        let base_y = self.prev_line_y;
        let rgb = self.prev_line_color & 0x00FF_FFFF;
        let chars: Vec<char> = self.prev_line_text.chars().take(48).collect();
        for ch in chars {
            let s = ch.to_string();
            let cw = metrics.width(&s) * scale;
            if ch != ' ' {
                let r1 = self.next_rand() - 0.5;
                let r2 = self.next_rand();
                self.crumble.push(CrumbleBit {
                    ch: s,
                    x: cursor_x,
                    y: base_y,
                    vx: r1 * 85.0,
                    vy: -35.0 - r2 * 60.0,
                    color: 0xFF00_0000 | rgb,
                    age: 0.0,
                    life: 0.75 + r2 * 0.45,
                });
            }
            cursor_x += cw;
        }
    }

    fn step_crumble(&mut self, g: &mut DrawList, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        self.crumble.retain_mut(|b| {
            b.age += dt;
            if b.age >= b.life {
                return false;
            }
            b.vy += GRAVITY_2D * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            let k = 1.0 - (b.age / b.life).clamp(0.0, 1.0);
            g.text(&b.ch, b.x, b.y, PlayerTheme::fade(b.color, k), true);
            true
        });
    }

    pub fn render_volume_only(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        cfg: &HudConfig,
        sw: i32,
        sh: i32,
    ) {
        self.render_volume(g, metrics, ctl, cfg, sw, sh, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_volume(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        cfg: &HudConfig,
        sw: i32,
        sh: i32,
        preview: bool,
    ) {
        if !cfg.volume_enabled && !preview {
            return;
        }
        let vol = ctl.volume().clamp(0.0, 1.0);
        if !self.vol_init {
            self.last_vol = vol;
            self.vol_init = true;
        } else if (vol - self.last_vol).abs() > 1e-4 {
            self.notify_volume_changed(vol);
        }
        let now = now_ms();
        let show_ms = (cfg.volume_seconds.max(1) as i64) * 1000;
        let alpha = if preview {
            1.0
        } else {
            let elapsed = now - self.vol_shown_ms;
            if elapsed < 0 || elapsed > show_ms {
                return;
            }
            if elapsed > show_ms - 400 {
                ((show_ms - elapsed) as f32 / 400.0).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };

        let (bw, bh) = match cfg.volume_style {
            VolumeStyle::Bar => (134, 22),
            VolumeStyle::Compact => (96, 18),
            VolumeStyle::Vertical => (24, 96),
        };
        let scale = cfg.volume_scale.clamp(0.5, 2.0);
        let sw_s = (bw as f32 * scale).round() as i32;
        let sh_s = (bh as f32 * scale).round() as i32;
        let (x, y) = compute_anchored_pos(
            cfg.volume_anchor,
            sw_s,
            sh_s,
            cfg.volume_offset_x,
            cfg.volume_offset_y,
            sw,
            sh,
        );
        self.bounds.volume = Some(HudBox {
            x,
            y,
            w: sw_s,
            h: sh_s,
        });

        let accent = cfg.accent_rgb() & 0x00FF_FFFF;
        let vol = ctl.volume().clamp(0.0, 1.0);
        let pct_str = format!("{}%", (vol * 100.0).round() as i32);
        let icon_col = PlayerTheme::argb(0xFF_FFFF, (255.0 * alpha) as i32);

        g.push_translate(x as f32, y as f32);
        g.push_scale(scale);
        panel_bg(
            g,
            0.0,
            0.0,
            bw as f32,
            bh as f32,
            cfg.volume_background,
            alpha,
        );

        let waves = if vol <= 0.01 {
            0
        } else if vol < 0.5 {
            1
        } else {
            2
        };
        match cfg.volume_style {
            VolumeStyle::Bar | VolumeStyle::Compact => {
                PlayerTheme::icon_volume(g, 11, bh / 2, waves, icon_col);
                let pct_w = if cfg.volume_percent {
                    metrics.width(&pct_str) + 6.0
                } else {
                    0.0
                };
                let bar_x = 22.0;
                let bar_w = (bw as f32 - bar_x - 8.0 - pct_w).max(20.0);
                let bar_h = 4.0;
                let bar_y = (bh as f32 - bar_h) / 2.0;
                panel_progress(g, bar_x, bar_y, bar_w, bar_h, vol, accent, alpha);
                if cfg.volume_percent {
                    g.text(
                        pct_str,
                        bw as f32 - pct_w + 1.0,
                        (bh as f32 - metrics.line_height()) / 2.0,
                        icon_col,
                        true,
                    );
                }
            }
            VolumeStyle::Vertical => {
                PlayerTheme::icon_volume(g, bw / 2, bh - 10, waves, icon_col);
                let top_pad = if cfg.volume_percent {
                    let tw = metrics.width(&pct_str);
                    g.text(&pct_str, (bw as f32 - tw) / 2.0, 4.0, icon_col, true);
                    15.0
                } else {
                    7.0
                };
                let bar_h = (bh as f32 - top_pad - 20.0).max(20.0);
                let bar_x = (bw as f32 - 5.0) / 2.0;
                g.rect(
                    bar_x,
                    top_pad,
                    5.0,
                    bar_h,
                    PlayerTheme::argb(0xFF_FFFF, (38.0 * alpha) as i32),
                );
                let fill_h = bar_h * vol;
                if fill_h > 0.5 {
                    g.rect(
                        bar_x,
                        top_pad + bar_h - fill_h,
                        5.0,
                        fill_h,
                        PlayerTheme::argb(accent, (240.0 * alpha) as i32),
                    );
                }
            }
        }
        g.pop_scale();
        g.pop_translate();
    }

    #[allow(clippy::too_many_arguments)]
    fn render_visualizer(
        &mut self,
        g: &mut DrawList,
        ctl: &PlayerController,
        cfg: &HudConfig,
        sw: i32,
        sh: i32,
        dt: f32,
        preview: bool,
    ) {
        if !cfg.viz_enabled && !preview {
            return;
        }
        let playing = ctl.is_playing();
        if !preview && cfg.viz_hide_idle && !playing {
            return;
        }
        let bars = cfg.viz_bars.clamp(4, 96) as usize;
        let bw = cfg.viz_bar_width.clamp(1, 10);
        let gap = cfg.viz_gap.clamp(0, 6);
        let bh = cfg.viz_height.clamp(8, 160);
        let base_w = bars as i32 * bw + (bars as i32 - 1) * gap + 8;
        let base_h = bh + 6;
        let scale = cfg.viz_scale.clamp(0.5, 2.0);
        let sw_s = (base_w as f32 * scale).round() as i32;
        let sh_s = (base_h as f32 * scale).round() as i32;
        let (x, y) = compute_anchored_pos(
            cfg.viz_anchor,
            sw_s,
            sh_s,
            cfg.viz_offset_x,
            cfg.viz_offset_y,
            sw,
            sh,
        );
        self.bounds.viz = Some(HudBox {
            x,
            y,
            w: sw_s,
            h: sh_s,
        });

        let alpha = (cfg.viz_opacity.clamp(5, 100) as f32) / 100.0;
        let accent = cfg.accent_rgb() & 0x00FF_FFFF;
        let now_s = now_ms() as f32 / 1000.0;
        if self.viz_peaks.len() != bars {
            self.viz_peaks.resize(bars, 0.0);
            self.viz_peak_vel.resize(bars, 0.0);
        }

        let mut spectrum = [0.0f32; 96];
        {
            let mut vst = ctl.viz.lock().unwrap();
            vst.update(
                bars,
                cfg.viz_sensitivity,
                cfg.viz_smoothing,
                cfg.viz_auto_sens,
                preview && !playing,
                cfg.viz_stereo,
            );
            let bands = vst.bands();
            for (i, dst) in spectrum[..bars].iter_mut().enumerate() {
                *dst = bands.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }

        let dir_up = match cfg.viz_direction {
            VizDirection::Up => true,
            VizDirection::Down => false,
            VizDirection::Auto => anchor_hv(cfg.viz_anchor).1 >= 0,
        };

        g.push_translate(x as f32, y as f32);
        g.push_scale(scale);
        panel_bg(
            g,
            0.0,
            0.0,
            base_w as f32,
            base_h as f32,
            cfg.viz_background,
            alpha,
        );

        let base_top = 3.0;
        let h_f = bh as f32;
        for (i, &val) in spectrum[..bars].iter().enumerate() {
            let bx = 4.0 + i as f32 * (bw + gap) as f32;
            let bar_h = (val * h_f).max(1.5);
            let rgb = match cfg.viz_color {
                VizColor::Accent => accent,
                VizColor::White => 0xFF_FFFF,
                VizColor::Gradient => PlayerTheme::mix(accent, 0x0038_BDF8, val),
                VizColor::Rainbow => {
                    let hue = (i as f32 / bars.max(1) as f32 + now_s * 0.15) % 1.0;
                    crate::ui::cover::hsv_to_rgb(hue, 0.85, 1.0)
                }
            };
            let col = PlayerTheme::argb(rgb, (240.0 * alpha) as i32);

            if val >= self.viz_peaks[i] {
                self.viz_peaks[i] = val;
                self.viz_peak_vel[i] = 0.0;
            } else {
                self.viz_peak_vel[i] += 0.9 * dt;
                self.viz_peaks[i] = (self.viz_peaks[i] - self.viz_peak_vel[i] * dt).max(val);
            }

            match cfg.viz_style {
                VizStyle::Bars => {
                    let by = if dir_up {
                        base_top + h_f - bar_h
                    } else {
                        base_top
                    };
                    g.rect(bx, by, bw as f32, bar_h, col);
                }
                VizStyle::Blocks => {
                    let segs = ((bar_h / 4.0).floor() as i32).max(1);
                    for s in 0..segs {
                        let sy = if dir_up {
                            base_top + h_f - (s + 1) as f32 * 4.0 + 1.0
                        } else {
                            base_top + s as f32 * 4.0
                        };
                        g.rect(bx, sy, bw as f32, 3.0, col);
                    }
                }
                VizStyle::Mirror => {
                    let cy = base_top + h_f / 2.0;
                    g.rect(bx, cy - bar_h / 2.0, bw as f32, bar_h, col);
                }
                VizStyle::Wave => {
                    let wy = base_top + h_f / 2.0 + (val - 0.5) * h_f * 0.8;
                    g.rect(bx, wy - 1.5, (bw + gap.min(1)) as f32, 3.0, col);
                }
            }

            if cfg.viz_peaks && cfg.viz_style != VizStyle::Wave {
                let py = if dir_up {
                    base_top + h_f - (self.viz_peaks[i] * h_f).max(2.0)
                } else {
                    base_top + (self.viz_peaks[i] * h_f).max(2.0) - 2.0
                };
                g.rect(
                    bx,
                    py,
                    bw as f32,
                    1.5,
                    PlayerTheme::argb(0xFF_FFFF, (220.0 * alpha) as i32),
                );
            }
        }
        g.pop_scale();
        g.pop_translate();
    }
}

fn widget_visibility_alpha(
    cfg: &HudConfig,
    playing: bool,
    now: i64,
    track_changed_ms: i64,
    near_end_ms: i64,
) -> f32 {
    if cfg.show_mode == ShowMode::Always {
        return 1.0;
    }
    if !playing && now - track_changed_ms > 2_000 {
        return 0.0;
    }
    let dur = cfg.show_mode.duration_ms() as i64;
    let mut best = 0.0f32;
    if cfg.show_on_song_start {
        best = best.max(window_envelope(now - track_changed_ms, dur));
    }
    if cfg.show_on_song_end && near_end_ms > 0 {
        best = best.max(window_envelope(now - near_end_ms, dur));
    }
    best
}

fn window_envelope(elapsed: i64, dur: i64) -> f32 {
    if elapsed < 0 || elapsed > dur + FADE_OUT_MS {
        return 0.0;
    }
    if elapsed < FADE_IN_MS {
        elapsed as f32 / FADE_IN_MS as f32
    } else if elapsed <= dur {
        1.0
    } else {
        1.0 - (elapsed - dur) as f32 / FADE_OUT_MS as f32
    }
}

fn draw_mini_eq(g: &mut DrawList, x: f32, y: f32, h: i32, playing: bool, secs: f64, color: u32) {
    for i in 0..3 {
        let k = if playing {
            0.5 + 0.5 * (secs * (6.0 + i as f64 * 2.1) + i as f64 * 1.9).sin()
        } else {
            0.2
        };
        let bh = 2.0 + (k as f32 * (h - 2) as f32).round();
        g.rect(x + i as f32 * 3.0, y + h as f32 - bh, 2.0, bh, color);
    }
}

fn active_synced_text(ctl: &PlayerController, pos_ms: u64) -> Option<String> {
    let cand = ctl.active_lyrics()?;
    if cand.lines.is_empty() || !cand.lines[0].is_synced() {
        return None;
    }
    let idx = lyrics::current_index(&cand.lines, pos_ms)?;
    Some(cand.lines[idx].text.clone())
}

#[allow(clippy::too_many_arguments)]
fn draw_word_karaoke(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    text: &str,
    words: &[WordTiming],
    pos_ms: u64,
    line_prog: f32,
    start_x: f32,
    y: f32,
    accent: u32,
    use_accent: bool,
    alpha: f32,
    shadow: bool,
) {
    let active_rgb = if use_accent { accent } else { 0xFF_FFFF };
    let sung_col = PlayerTheme::argb(active_rgb, (255.0 * alpha) as i32);
    let dim_col = PlayerTheme::argb(0xC8_C8D4, (200.0 * alpha) as i32);

    if words.is_empty() {
        // Fallback character sweep proportional to line_prog (1:1 Overtone karaoke sweep)
        let total = text.chars().count().max(1);
        let cutoff = ((total as f32) * line_prog).round() as usize;
        let sung: String = text.chars().take(cutoff).collect();
        let rest: String = text.chars().skip(cutoff).collect();
        let sw = metrics.width(&sung);
        if !sung.is_empty() {
            g.text(sung, start_x, y, sung_col, shadow);
        }
        if !rest.is_empty() {
            g.text(rest, start_x + sw, y, dim_col, shadow);
        }
        return;
    }

    // Exact per-word spans from AMLL/YRC/TTML
    let mut cx = start_x;
    for (i, w) in words.iter().enumerate() {
        let token = if i + 1 < words.len() {
            format!("{} ", w.text)
        } else {
            w.text.clone()
        };
        let end_ms = w.end_ms.unwrap_or_else(|| {
            words
                .get(i + 1)
                .map(|nw| nw.time_ms)
                .unwrap_or(w.time_ms.saturating_add(450))
        });
        let dur = end_ms.saturating_sub(w.time_ms).max(1);
        let wp = if pos_ms <= w.time_ms {
            0.0
        } else if pos_ms >= end_ms {
            1.0
        } else {
            (pos_ms - w.time_ms) as f32 / dur as f32
        };
        let hop = if wp > 0.0 && wp < 1.0 {
            -((wp * std::f32::consts::PI).sin() * 2.0)
        } else {
            0.0
        };
        let rgb = PlayerTheme::mix(0xC8_C8D4, active_rgb, wp.sqrt());
        let col = PlayerTheme::argb(rgb, (255.0 * alpha) as i32);
        let adv = metrics.width(&token);
        g.text(token, cx, y + hop, col, shadow);
        cx += adv;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LyricCandidate, LyricSourceKind, Track};
    use crate::ui::MonoMetrics;

    #[test]
    fn hud_renders_all_six_layouts_and_suppresses_unsynced_lyrics() {
        let ctl = PlayerController::new(false);
        let covers = CoverArtManager::new();
        let metrics = MonoMetrics;

        {
            let mut st = ctl.state.lock().unwrap();
            st.queue.push(Track {
                id: "t1".into(),
                title: "Neon Track".into(),
                artist: "Artist".into(),
                duration_ms: 60_000,
                provider: "local".into(),
                ..Default::default()
            });
            st.index = Some(0);
            st.lyrics_track_id = "t1".into();
            // Inject UNSYNCED lines — HUD must refuse to draw them!
            st.lyrics = Some(crate::lyrics::LyricsLookupResult {
                candidates: vec![LyricCandidate {
                    source: LyricSourceKind::LrcLib,
                    lines: vec![LyricLine::new(
                        LyricLine::UNSYNCED,
                        "SHOULD NOT APPEAR ON HUD",
                    )],
                    score: 1.0,
                    has_word_timings: false,
                    is_auto_dsp: false,
                }],
                active_idx: 0,
                plain_available: false,
            });
        }

        let mut hud = HudState::new();
        for layout in Layout::ALL {
            ctl.update_hud_config(|h| h.layout = layout);
            let mut g = DrawList::new();
            hud.render_all(
                &mut g, &metrics, &ctl, &covers, 640, 360, 0.016, false, false, false, false, false,
            );
            assert!(hud.bounds.widget.is_some());
            assert!(hud.bounds.lyrics.is_none());
            for cmd in g.cmds {
                if let crate::ui::DrawCmd::Text { text, .. } = cmd {
                    assert!(
                        !text.contains("SHOULD NOT APPEAR"),
                        "unsynced text leaked onto HUD"
                    );
                }
            }
        }
    }
}
