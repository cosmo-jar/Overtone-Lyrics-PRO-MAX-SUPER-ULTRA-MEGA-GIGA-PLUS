//! Music-reactive cinematic camera ("Auto-showroom") controller.

use std::time::Instant;

use crate::config::{CineCuts, CineShots, HudConfig};
use crate::viz::VisualizerState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Off,
    Enter,
    On,
    Exit,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CineView {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

#[derive(Debug, Clone, Copy)]
struct Shot {
    kind: CineShots,
    a0: f32,
    dir: f32,
    t: f32,
    age: f32,
}

const ENTER_S: f32 = 1.6;
const EXIT_S: f32 = 1.2;
const HUD_S: f32 = 0.6;
const BLEND_S: f32 = 1.1;

const ALL_SHOTS: [CineShots; 6] = [
    CineShots::Orbit,
    CineShots::Low,
    CineShots::High,
    CineShots::Dolly,
    CineShots::Crane,
    CineShots::Track,
];

fn smooth_quintic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn pseudo_rand(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 33) as u32) as f32 / (u32::MAX as f32)
}

impl Shot {
    fn eval(
        &self,
        target: [f64; 3],
        cfg: &HudConfig,
        feet: f64,
        player_yaw_rad: f64,
    ) -> ([f64; 3], [f64; 3]) {
        let dist = cfg.cine_distance as f64;
        let h = cfg.cine_height as f64;
        let a0 = self.a0 as f64;
        let dir = self.dir as f64;
        let t = self.t as f64;
        let mut look = target;
        let pos = match self.kind {
            CineShots::Low => {
                let ang = a0 + dir * t * 0.22;
                look = [target[0], target[1] + 0.35, target[2]];
                [
                    target[0] + ang.sin() * dist * 0.75,
                    target[1] + feet + 0.35,
                    target[2] + ang.cos() * dist * 0.75,
                ]
            }
            CineShots::High => {
                let ang = a0 + dir * t * 0.15;
                [
                    target[0] + ang.sin() * dist * 0.55,
                    target[1] + dist * 0.9 + h,
                    target[2] + ang.cos() * dist * 0.55,
                ]
            }
            CineShots::Dolly => {
                let ang = player_yaw_rad + dir * 0.35;
                let d = dist * (1.5 - 0.11 * t).max(0.45);
                [
                    target[0] - ang.sin() * d,
                    target[1] + h * 0.25,
                    target[2] + ang.cos() * d,
                ]
            }
            CineShots::Crane => {
                let ang = a0 + dir * t * 0.08;
                let r = (t / 8.0).min(1.0);
                let rise = r * r * (3.0 - 2.0 * r);
                [
                    target[0] + ang.sin() * dist,
                    target[1] + feet + 0.3 + (dist * 0.8 + h - feet) * rise,
                    target[2] + ang.cos() * dist,
                ]
            }
            CineShots::Track => {
                let fx = a0.sin();
                let fz = a0.cos();
                let slide = dist * 0.9 * (-1.0 + (t * 0.12).min(2.0)) * dir;
                look = [
                    target[0] + fz * slide * 0.25,
                    target[1],
                    target[2] - fx * slide * 0.25,
                ];
                [
                    target[0] + fx * dist * 0.85 + fz * slide,
                    target[1] + h * 0.3,
                    target[2] + fz * dist * 0.85 - fx * slide,
                ]
            }
            CineShots::Orbit | CineShots::Mix => {
                let ang = a0 + dir * t * 0.35;
                [
                    target[0] + ang.sin() * dist,
                    target[1] + h * 0.5,
                    target[2] + ang.cos() * dist,
                ]
            }
        };
        (pos, look)
    }
}

pub struct CinematicController {
    phase: Phase,
    phase_t: f32,
    last_frame: Option<Instant>,
    shot: Option<Shot>,
    prev_shot: Option<Shot>,
    blend: f32,
    hud_hidden: f32,
    fov_offset: f32,
    energy: f32,
    bass_avg: f32,
    pulse: f32,
    punch: f32,
    clock: f32,
    since_beat: f32,
    rng: u64,
    viz: VisualizerState,
}

impl Default for CinematicController {
    fn default() -> Self {
        Self::new()
    }
}

impl CinematicController {
    pub fn new() -> Self {
        Self {
            phase: Phase::Off,
            phase_t: 0.0,
            last_frame: None,
            shot: None,
            prev_shot: None,
            blend: 1.0,
            hud_hidden: 0.0,
            fov_offset: 0.0,
            energy: 0.0,
            bass_avg: 0.0,
            pulse: 0.0,
            punch: 0.0,
            clock: 0.0,
            since_beat: 9.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            viz: VisualizerState::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.phase != Phase::Off
    }

    pub fn hud_hidden(&self) -> f32 {
        self.hud_hidden
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn toggle(&mut self, player_yaw_deg: f32, cfg: &HudConfig) {
        if self.phase != Phase::Off && self.phase != Phase::Exit {
            self.phase = Phase::Exit;
            self.phase_t = 0.0;
        } else {
            if self.phase == Phase::Off {
                let yaw = player_yaw_deg.to_radians();
                self.shot = Some(self.new_shot(cfg, None, std::f32::consts::PI - yaw + 0.6));
                self.prev_shot = None;
                self.blend = 1.0;
            }
            self.phase = Phase::Enter;
            self.phase_t = 0.0;
            self.last_frame = None;
        }
    }

    pub fn stop(&mut self) {
        self.phase = Phase::Off;
        self.hud_hidden = 0.0;
        self.fov_offset = 0.0;
        self.last_frame = None;
    }

    pub fn tick(&mut self, cfg: &HudConfig, player_alive: bool, no_screen: bool, moving: bool) {
        if self.phase == Phase::Off {
            return;
        }
        if !player_alive {
            self.stop();
        } else if cfg.cine_exit_on_move && self.phase != Phase::Exit && no_screen && moving {
            self.phase = Phase::Exit;
            self.phase_t = 0.0;
        }
    }

    pub fn fov(&self, vanilla_fov: f32) -> f32 {
        (vanilla_fov + self.fov_offset).max(10.0)
    }

    fn new_shot(&mut self, cfg: &HudConfig, prev: Option<Shot>, angle: f32) -> Shot {
        let mut kind = cfg.cine_shots;
        if kind == CineShots::Mix {
            loop {
                let idx = (pseudo_rand(&mut self.rng) * ALL_SHOTS.len() as f32) as usize
                    % ALL_SHOTS.len();
                kind = ALL_SHOTS[idx];
                if prev.is_none_or(|p| p.kind != kind) {
                    break;
                }
            }
        }
        Shot {
            kind,
            a0: angle,
            dir: if pseudo_rand(&mut self.rng) >= 0.5 {
                1.0
            } else {
                -1.0
            },
            t: 0.0,
            age: 0.0,
        }
    }

    fn listen(&mut self, dt: f32) -> bool {
        self.viz.update(8, 100, 20, true, false, false);
        self.since_beat += dt;
        self.pulse *= (-dt * 4.5).exp();
        self.punch += (self.pulse - self.punch) * (1.0 - (-dt * 10.0).exp());
        let lv = &self.viz.levels;
        if !self.viz.live || lv.len() < 2 {
            self.energy += (0.35 - self.energy) * (dt * 2.0).min(1.0);
            return false;
        }
        let sum: f32 = lv.iter().sum();
        self.energy += (sum / lv.len() as f32 - self.energy) * (dt * 3.0).min(1.0);
        let bass = lv[0].max(lv[1]);
        let kick = bass > 0.45 && bass - self.bass_avg > 0.16 && self.since_beat > 0.28;
        self.bass_avg += (bass - self.bass_avg) * (dt * 2.5).min(1.0);
        if kick {
            self.since_beat = 0.0;
            self.pulse = 1.0;
        }
        kick
    }

    /// Evaluates camera placement. `clip_ray(target, candidate_pos)` returns unobstructed position.
    #[allow(clippy::too_many_arguments)]
    pub fn frame<F>(
        &mut self,
        cfg: &HudConfig,
        player_pos: [f64; 3],
        eye_height: f64,
        player_yaw_deg: f32,
        eye_pos: [f64; 3],
        view_dir: [f64; 3],
        base_fov: f32,
        clip_ray: F,
    ) -> Option<CineView>
    where
        F: FnOnce([f64; 3], [f64; 3]) -> [f64; 3],
    {
        if self.phase == Phase::Off {
            return None;
        }
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|prev| (now - prev).as_secs_f32().min(0.1))
            .unwrap_or(0.016);
        self.last_frame = Some(now);
        self.phase_t += dt;

        let k = if cfg.cine_music {
            cfg.cine_music_strength as f32 / 100.0
        } else {
            0.0
        };
        let beat = self.listen(dt);
        let speed = (cfg.cine_speed as f32 / 100.0)
            * (1.0 + (self.energy - 0.35) * 1.3 * k).clamp(0.35, 2.5);

        let mut shot = self.shot.unwrap_or(Shot {
            kind: cfg.cine_shots,
            a0: 0.0,
            dir: 1.0,
            t: 0.0,
            age: 0.0,
        });
        shot.t += dt * speed;
        shot.age += dt;
        if let Some(prev) = &mut self.prev_shot {
            prev.t += dt * speed;
            self.blend = (self.blend + dt / BLEND_S).min(1.0);
        }

        let min_s = cfg.cine_shot_sec as f32 * if k > 0.0 { 0.6 } else { 1.0 };
        let max_s = cfg.cine_shot_sec as f32 * if k > 0.0 { 1.7 } else { 1.0 };
        if self.phase != Phase::Exit
            && (shot.age >= max_s || (k > 0.0 && beat && shot.age >= min_s))
        {
            let smooth = cfg.cine_cuts == CineCuts::Smooth;
            self.prev_shot = if smooth { Some(shot) } else { None };
            let next_a0 =
                shot.a0 + std::f32::consts::PI * (0.55 + pseudo_rand(&mut self.rng) * 0.9);
            shot = self.new_shot(cfg, Some(shot), next_a0);
            self.blend = if smooth { 0.0 } else { 1.0 };
        }
        self.shot = Some(shot);

        let target = [
            player_pos[0],
            player_pos[1] + eye_height * 0.72,
            player_pos[2],
        ];
        let feet = -eye_height * 0.72;
        let player_yaw_rad = (player_yaw_deg as f64).to_radians();
        let (mut pos, mut look) = shot.eval(target, cfg, feet, player_yaw_rad);
        if let Some(prev) = self.prev_shot
            && self.blend < 1.0
        {
            let (old_pos, old_look) = prev.eval(target, cfg, feet, player_yaw_rad);
            let e = smooth_quintic(self.blend) as f64;
            pos = lerp3(old_pos, pos, e);
            look = lerp3(old_look, look, e);
        }

        self.clock += dt;
        let sway = 0.035 * (self.punch * k) as f64;
        let zoom = 1.0 - 0.04 * (self.punch * k) as f64;
        pos = [
            target[0]
                + (pos[0] - target[0]) * zoom
                + (((self.clock * 11.0).sin() + (self.clock * 6.3).sin()) * 0.5) as f64 * sway,
            target[1]
                + (pos[1] - target[1]) * zoom
                + (((self.clock * 9.1 + 1.3).sin() + (self.clock * 4.7).sin()) * 0.5) as f64 * sway,
            target[2] + (pos[2] - target[2]) * zoom,
        ];
        pos = clip_ray(target, pos);

        let eye_look = [
            eye_pos[0] + view_dir[0] * 8.0,
            eye_pos[1] + view_dir[1] * 8.0,
            eye_pos[2] + view_dir[2] * 8.0,
        ];

        let into = match self.phase {
            Phase::Enter => {
                let i = smooth_quintic((self.phase_t / ENTER_S).min(1.0));
                self.hud_hidden = smooth_quintic((self.phase_t / HUD_S).min(1.0));
                if self.phase_t >= ENTER_S {
                    self.phase = Phase::On;
                }
                i
            }
            Phase::Exit => {
                let i = 1.0 - smooth_quintic((self.phase_t / EXIT_S).min(1.0));
                self.hud_hidden = 1.0
                    - smooth_quintic(((self.phase_t - (EXIT_S - HUD_S)) / HUD_S).clamp(0.0, 1.0));
                if self.phase_t >= EXIT_S {
                    self.stop();
                    return None;
                }
                i
            }
            Phase::On => {
                self.hud_hidden = 1.0;
                1.0
            }
            Phase::Off => return None,
        };

        pos = lerp3(eye_pos, pos, into as f64);
        look = lerp3(eye_look, look, into as f64);
        self.fov_offset = (cfg.cine_fov as f32 - 3.5 * self.punch * k) * into;

        let dx = look[0] - pos[0];
        let dy = look[1] - pos[1];
        let dz = look[2] - pos[2];
        let flat = (dx * dx + dz * dz).sqrt();
        let yaw = (-dx).atan2(dz).to_degrees() as f32;
        let pitch = (-dy.atan2(flat)).to_degrees() as f32;
        Some(CineView {
            pos,
            yaw,
            pitch,
            fov: self.fov(base_fov),
        })
    }

    pub fn bar_height(&self, cfg: &HudConfig, screen_h: i32) -> i32 {
        if self.hud_hidden <= 0.0 || !cfg.cine_bars {
            return 0;
        }
        let h = self.hud_hidden;
        let e = h * h * (3.0 - 2.0 * h);
        (screen_h as f32 * 0.11 * e).round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cinema_toggle_and_frame() {
        let mut c = CinematicController::new();
        let cfg = HudConfig::default();
        assert!(!c.active());
        c.toggle(45.0, &cfg);
        assert!(c.active());
        let view = c.frame(
            &cfg,
            [0.0, 64.0, 0.0],
            1.62,
            45.0,
            [0.0, 65.62, 0.0],
            [0.0, 0.0, 1.0],
            70.0,
            |_, p| p,
        );
        assert!(view.is_some());
    }
}
