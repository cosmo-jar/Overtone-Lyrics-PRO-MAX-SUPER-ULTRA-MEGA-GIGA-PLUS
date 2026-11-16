pub mod cover;
pub mod hud;
pub mod screens;
pub mod world_lyrics;

use std::sync::{OnceLock, RwLock};

use crate::config::{HudConfig, PlayerConfig};
use crate::model::Platform;

/// Font measurement abstraction shared by the 2D HUD, UI screens, 3D world lyrics,
/// and unit tests (where `MonoMetrics` gives deterministic pixel widths).
pub trait TextMetrics {
    fn width(&self, text: &str) -> f32;
    fn line_height(&self) -> f32;

    fn trim_to_width(&self, text: &str, max_w: f32) -> String {
        if max_w <= 0.0 || text.is_empty() {
            return String::new();
        }
        if self.width(text) <= max_w {
            return text.to_owned();
        }
        let mut end = 0usize;
        for (idx, ch) in text.char_indices() {
            let next = idx + ch.len_utf8();
            if self.width(&text[..next]) > max_w {
                break;
            }
            end = next;
        }
        text[..end].to_owned()
    }

    fn ellipsize(&self, text: &str, max_w: f32) -> String {
        if self.width(text) <= max_w {
            return text.to_owned();
        }
        let dots = "…";
        let room = (max_w - self.width(dots)).max(0.0);
        let prefix = self.trim_to_width(text, room);
        format!("{}{dots}", prefix.trim_end())
    }

    fn wrap(&self, text: &str, max_w: f32) -> Vec<String> {
        let limit = max_w.max(8.0);
        let mut out = Vec::new();
        for raw_line in text.lines() {
            if raw_line.is_empty() {
                out.push(String::new());
                continue;
            }
            let mut cur = String::new();
            for word in raw_line.split_inclusive(' ') {
                let cand = if cur.is_empty() {
                    word.to_owned()
                } else {
                    format!("{cur}{word}")
                };
                if self.width(cand.trim_end()) <= limit {
                    cur = cand;
                    continue;
                }
                if !cur.trim_end().is_empty() {
                    out.push(cur.trim_end().to_owned());
                    cur.clear();
                }
                let mut rest = word;
                while self.width(rest.trim_end()) > limit && rest.chars().count() > 1 {
                    let cut = self.trim_to_width(rest, limit);
                    let take = if cut.is_empty() {
                        rest.chars().next().map(char::len_utf8).unwrap_or(1)
                    } else {
                        cut.len()
                    };
                    out.push(rest[..take].trim_end().to_owned());
                    rest = &rest[take..];
                }
                cur = rest.to_owned();
            }
            if !cur.trim_end().is_empty() || out.is_empty() {
                out.push(cur.trim_end().to_owned());
            }
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }
}

/// Deterministic fallback metrics for tests and offscreen layout (6px per char, 9px line height).
#[derive(Clone, Copy, Debug, Default)]
pub struct MonoMetrics;

impl TextMetrics for MonoMetrics {
    fn width(&self, text: &str) -> f32 {
        let mut w = 0.0f32;
        for ch in text.chars() {
            w += match ch {
                'i' | 'l' | '!' | '.' | ',' | ':' | ';' | '|' | '\'' => 2.0,
                'I' | 't' | ' ' => 4.0,
                'f' | 'k' | 'r' | 's' => 5.0,
                'W' | 'M' | '№' | '♪' => 7.0,
                _ => 6.0,
            };
        }
        w
    }

    fn line_height(&self) -> f32 {
        9.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexSlot {
    CoverHud,
    CoverBig,
    PickerSquare,
    PickerHue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    FillRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        argb: u32,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        argb: u32,
        shadow: bool,
        scale: f32,
    },
    Tex {
        slot: TexSlot,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        alpha: f32,
    },
    PushScissor {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    PopScissor,
}

/// Command buffer mirror of `dev.musicplayer.compat.Canvas` with scale stack.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    pub cmds: Vec<DrawCmd>,
    scale_stack: Vec<f32>,
    tx_stack: Vec<[f32; 2]>,
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    fn cur_scale(&self) -> f32 {
        self.scale_stack.last().copied().unwrap_or(1.0)
    }

    fn cur_offset(&self) -> [f32; 2] {
        self.tx_stack.last().copied().unwrap_or([0.0, 0.0])
    }

    pub fn push_scale(&mut self, factor: f32) {
        self.scale_stack.push(self.cur_scale() * factor);
    }

    pub fn push_translate(&mut self, dx: f32, dy: f32) {
        let [ox, oy] = self.cur_offset();
        let s = self.cur_scale();
        self.tx_stack.push([ox + dx * s, oy + dy * s]);
    }

    pub fn pop_scale(&mut self) {
        self.scale_stack.pop();
    }

    pub fn pop_translate(&mut self) {
        self.tx_stack.pop();
    }

    pub fn push_scissor(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = self.cur_scale();
        let [ox, oy] = self.cur_offset();
        self.cmds.push(DrawCmd::PushScissor {
            x: ox + x * s,
            y: oy + y * s,
            w: w * s,
            h: h * s,
        });
    }

    pub fn pop_scissor(&mut self) {
        self.cmds.push(DrawCmd::PopScissor);
    }

    pub fn fill(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, argb: u32) {
        let w = x2 - x1;
        let h = y2 - y1;
        if w <= 0.0 || h <= 0.0 || (argb >> 24) == 0 {
            return;
        }
        let s = self.cur_scale();
        let [ox, oy] = self.cur_offset();
        self.cmds.push(DrawCmd::FillRect {
            x: ox + x1 * s,
            y: oy + y1 * s,
            w: w * s,
            h: h * s,
            argb,
        });
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, argb: u32) {
        self.fill(x, y, x + w, y + h, argb);
    }

    pub fn text(&mut self, text: impl Into<String>, x: f32, y: f32, argb: u32, shadow: bool) {
        let t = text.into();
        if t.is_empty() || (argb >> 24) == 0 {
            return;
        }
        let s = self.cur_scale();
        let [ox, oy] = self.cur_offset();
        self.cmds.push(DrawCmd::Text {
            text: t,
            x: ox + x * s,
            y: oy + y * s,
            argb,
            shadow,
            scale: s,
        });
    }

    pub fn texture(&mut self, slot: TexSlot, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        if w <= 0.0 || h <= 0.0 || alpha <= 0.0 {
            return;
        }
        let s = self.cur_scale();
        let [ox, oy] = self.cur_offset();
        self.cmds.push(DrawCmd::Tex {
            slot,
            x: ox + x * s,
            y: oy + y * s,
            w: w * s,
            h: h * s,
            alpha: alpha.clamp(0.0, 1.0),
        });
    }

    /// Replays `cmds` into `self` with backdrop/panel scale + translation around `(cx, cy)` and alpha scaling.
    #[allow(clippy::too_many_arguments)]
    pub fn append_transformed_cmds(
        &mut self,
        cmds: &[DrawCmd],
        cx: f32,
        cy: f32,
        tx: f32,
        ty: f32,
        sx: f32,
        sy: f32,
        alpha: f32,
    ) {
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.01 {
            return;
        }
        let sx = sx.max(0.001);
        let sy = sy.max(0.001);
        let uniform = sx.min(sy);
        for cmd in cmds {
            match cmd {
                DrawCmd::FillRect { x, y, w, h, argb } => {
                    let faded = scale_argb_alpha(*argb, a);
                    if (faded >> 24) == 0 {
                        continue;
                    }
                    self.cmds.push(DrawCmd::FillRect {
                        x: cx + (*x - cx) * sx + tx,
                        y: cy + (*y - cy) * sy + ty,
                        w: *w * sx,
                        h: *h * sy,
                        argb: faded,
                    });
                }
                DrawCmd::Text {
                    text,
                    x,
                    y,
                    argb,
                    shadow,
                    scale,
                } => {
                    if uniform < 0.30 {
                        continue;
                    }
                    let faded = scale_argb_alpha(*argb, a);
                    if (faded >> 24) < 4 {
                        continue;
                    }
                    self.cmds.push(DrawCmd::Text {
                        text: text.clone(),
                        x: cx + (*x - cx) * sx + tx,
                        y: cy + (*y - cy) * sy + ty,
                        argb: faded,
                        shadow: *shadow,
                        scale: (*scale * uniform).clamp(0.05, 4.0),
                    });
                }
                DrawCmd::Tex {
                    slot,
                    x,
                    y,
                    w,
                    h,
                    alpha: ta,
                } => {
                    let final_a = (*ta * a).clamp(0.0, 1.0);
                    if final_a <= 0.01 {
                        continue;
                    }
                    self.cmds.push(DrawCmd::Tex {
                        slot: *slot,
                        x: cx + (*x - cx) * sx + tx,
                        y: cy + (*y - cy) * sy + ty,
                        w: *w * sx,
                        h: *h * sy,
                        alpha: final_a,
                    });
                }
                DrawCmd::PushScissor { x, y, w, h } => {
                    self.cmds.push(DrawCmd::PushScissor {
                        x: cx + (*x - cx) * sx + tx,
                        y: cy + (*y - cy) * sy + ty,
                        w: (*w * sx).max(0.0),
                        h: (*h * sy).max(0.0),
                    });
                }
                DrawCmd::PopScissor => self.cmds.push(DrawCmd::PopScissor),
            }
        }
    }
}

pub fn scale_argb_alpha(argb: u32, factor: f32) -> u32 {
    let orig_a = ((argb >> 24) & 0xFF) as f32;
    let new_a = (orig_a * factor.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u32;
    (new_a << 24) | (argb & 0x00FF_FFFF)
}

/// Exact 1:1 rounded rectangle scanline geometry matching `Shapes.java`.
pub mod shapes {
    use super::DrawList;

    fn inset(r: i32, row: i32) -> i32 {
        let dy = r as f64 - (row as f64 + 0.5);
        r - ((r as f64 * r as f64 - dy * dy).max(0.0).sqrt()).round() as i32
    }

    pub fn fill(g: &mut DrawList, x: i32, y: i32, w: i32, h: i32, r: i32, argb: u32) {
        if w <= 0 || h <= 0 || (argb >> 24) == 0 {
            return;
        }
        let r = r.max(0).min(w / 2).min(h / 2);
        if r == 0 {
            g.rect(x as f32, y as f32, w as f32, h as f32, argb);
            return;
        }
        for i in 0..r {
            let off = inset(r, i);
            let rw = w - off * 2;
            if rw > 0 {
                g.rect((x + off) as f32, (y + i) as f32, rw as f32, 1.0, argb);
                g.rect(
                    (x + off) as f32,
                    (y + h - 1 - i) as f32,
                    rw as f32,
                    1.0,
                    argb,
                );
            }
        }
        if h > r * 2 {
            g.rect(x as f32, (y + r) as f32, w as f32, (h - r * 2) as f32, argb);
        }
    }

    pub fn ring(g: &mut DrawList, x: i32, y: i32, w: i32, h: i32, r: i32, argb: u32) {
        if w < 2 || h < 2 || (argb >> 24) == 0 {
            return;
        }
        let r = r.max(0).min(w / 2).min(h / 2);
        let top_off = if r > 0 { inset(r, 0) } else { 0 };
        g.rect(
            (x + top_off) as f32,
            y as f32,
            (w - top_off * 2) as f32,
            1.0,
            argb,
        );
        g.rect(
            (x + top_off) as f32,
            (y + h - 1) as f32,
            (w - top_off * 2) as f32,
            1.0,
            argb,
        );
        for i in 1..r {
            let off = inset(r, i);
            let prev = inset(r, i - 1);
            let sw = (prev - off + 1).max(1);
            g.rect((x + off) as f32, (y + i) as f32, sw as f32, 1.0, argb);
            g.rect(
                (x + w - off - sw) as f32,
                (y + i) as f32,
                sw as f32,
                1.0,
                argb,
            );
            g.rect(
                (x + off) as f32,
                (y + h - 1 - i) as f32,
                sw as f32,
                1.0,
                argb,
            );
            g.rect(
                (x + w - off - sw) as f32,
                (y + h - 1 - i) as f32,
                sw as f32,
                1.0,
                argb,
            );
        }
        if h > r * 2 {
            g.rect(x as f32, (y + r) as f32, 1.0, (h - r * 2) as f32, argb);
            g.rect(
                (x + w - 1) as f32,
                (y + r) as f32,
                1.0,
                (h - r * 2) as f32,
                argb,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn shadow(
        g: &mut DrawList,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        spread: i32,
        argb: u32,
    ) {
        if spread <= 0 || (argb >> 24) == 0 {
            return;
        }
        let a = ((argb >> 24) & 0xFF) as f32;
        let rgb = argb & 0x00FF_FFFF;
        for i in 1..=spread {
            let step_a = ((a * (spread - i + 1) as f32) / (spread as f32 * 2.0)).round() as u32;
            fill(
                g,
                x - i,
                y - i + 1,
                w + i * 2,
                h + i * 2,
                r + i,
                (step_a.min(255) << 24) | rgb,
            );
        }
    }
}

/// Theme colors and primitive UI icons derived from `PlayerConfig` (1:1 `PlayerTheme.java`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerTheme {
    pub accent: u32,
    pub accent_hover: u32,
    pub panel_top: u32,
    pub panel2: u32,
    pub field_bg: u32,
    pub track_bg: u32,
    pub btn_bg: u32,
    pub card: u32,
    pub card_hover: u32,
    pub hover: u32,
    pub row_current: u32,
    pub border: u32,
    pub dim: u32,
    pub radius: i32,
    pub accent_rgb: u32,
    key: u64,
}

impl PlayerTheme {
    pub const TEXT_MAIN: u32 = 0xFFFF_FFFF;
    pub const TEXT_MID: u32 = 0xFFB4_B4C0;
    pub const TEXT_DIM: u32 = 0xFF6E_6E7C;
    pub const WARN: u32 = 0xFFE0_AF68;
    pub const HEART_W: i32 = 7;
    pub const HEART_H: i32 = 6;

    pub fn from_config(pc: &PlayerConfig, hud: &HudConfig) -> Self {
        let rgb = pc.resolved_accent_rgb(hud) & 0x00FF_FFFF;
        let key = (rgb as u64)
            | ((pc.tint as u64) << 24)
            | ((pc.dim as u64) << 28)
            | ((pc.corner_radius as u64) << 36)
            | ((pc.window_opacity as u64) << 44);
        let bg = ((pc.window_opacity as f32) * 2.55).round() as i32;
        let k = (pc.tint as f32) * 0.05;
        Self {
            accent: 0xFF00_0000 | rgb,
            accent_hover: 0xFF00_0000 | Self::mix(rgb, 0x00FF_FFFF, 0.15),
            panel_top: Self::argb(
                Self::mix(0x11_1115, rgb, k * 0.6),
                (bg as f32 * 0.94).round() as i32,
            ),
            panel2: Self::argb(Self::mix(0x08_080A, rgb, k * 0.4), bg.min(255)),
            field_bg: 0xFF00_0000 | Self::mix(0x09_090C, rgb, k * 0.3),
            track_bg: 0xFF00_0000 | Self::mix(0x24_242A, rgb, k * 0.5),
            btn_bg: 0xFF00_0000 | Self::mix(0x24_242A, rgb, k * 0.5),
            card: Self::argb(Self::mix(0x0E_0E12, rgb, k * 0.45), 185),
            card_hover: Self::argb(Self::mix(0x1E_1E26, rgb, k * 0.55), 220),
            hover: 0xFF00_0000 | Self::mix(0x30_3038, rgb, k * 0.5 + 0.08),
            row_current: 0xFF00_0000 | Self::mix(0x1A_1A20, rgb, 0.3),
            border: Self::argb(0x00FF_FFFF, 30),
            dim: Self::argb(0x0000_0000, ((pc.dim as f32) * 2.55).round() as i32),
            radius: (pc.corner_radius + 1) / 2,
            accent_rgb: rgb,
            key,
        }
    }

    pub fn current(pc: &PlayerConfig, hud: &HudConfig) -> Self {
        static CACHE: OnceLock<RwLock<Option<PlayerTheme>>> = OnceLock::new();
        let lock = CACHE.get_or_init(|| RwLock::new(None));
        let built = Self::from_config(pc, hud);
        if let Ok(guard) = lock.read()
            && let Some(c) = *guard
            && c.key == built.key
        {
            return c;
        }
        if let Ok(mut guard) = lock.write() {
            *guard = Some(built);
        }
        built
    }

    pub fn small(&self, r: i32) -> i32 {
        r.min(self.radius.max(3))
    }

    pub fn window(&self, g: &mut DrawList, x: i32, y: i32, w: i32, h: i32) {
        let a = ((self.panel_top >> 24) & 0xFF) as i32;
        shapes::shadow(g, x + 2, y, w, h, self.radius, 3, Self::argb(0, a / 3));
        shapes::fill(g, x, y, w, h, self.radius, self.panel_top);
        shapes::ring(g, x, y, w, h, self.radius, self.border);
        g.fill(
            (x + self.radius + 1) as f32,
            (y + 1) as f32,
            (x + w - self.radius - 1) as f32,
            (y + 2) as f32,
            Self::argb(0x00FF_FFFF, 14),
        );
    }

    pub fn argb(rgb: u32, alpha: i32) -> u32 {
        let a = alpha.clamp(0, 255) as u32;
        (a << 24) | (rgb & 0x00FF_FFFF)
    }

    pub fn fade(argb: u32, k: f32) -> u32 {
        let a = ((argb >> 24) & 0xFF) as f32;
        ((a * k.clamp(0.0, 1.0)).round() as u32) << 24 | (argb & 0x00FF_FFFF)
    }

    pub fn mix(from: u32, to: u32, t: f32) -> u32 {
        let t = t.clamp(0.0, 1.0);
        let r0 = ((from >> 16) & 0xFF) as f32;
        let g0 = ((from >> 8) & 0xFF) as f32;
        let b0 = (from & 0xFF) as f32;
        let r1 = ((to >> 16) & 0xFF) as f32;
        let g1 = ((to >> 8) & 0xFF) as f32;
        let b1 = (to & 0xFF) as f32;
        let r = (r0 + (r1 - r0) * t).round() as u32;
        let g = (g0 + (g1 - g0) * t).round() as u32;
        let b = (b0 + (b1 - b0) * t).round() as u32;
        (r << 16) | (g << 8) | b
    }

    pub fn blend(from: u32, to: u32, t: f32) -> u32 {
        0xFF00_0000 | Self::mix(from, to, t)
    }

    pub fn lighten(argb: u32, t: f32) -> u32 {
        let a = argb & 0xFF00_0000;
        let a = if a == 0 { 0xFF00_0000 } else { a };
        a | Self::mix(argb & 0x00FF_FFFF, 0x00FF_FFFF, t)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rrect(&self, g: &mut DrawList, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        shapes::fill(g, x, y, w, h, self.small(r), color);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ring(&self, g: &mut DrawList, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        shapes::ring(g, x, y, w, h, self.small(r), color);
    }

    pub fn circle(g: &mut DrawList, cx: i32, cy: i32, r: i32, color: u32) {
        if r <= 0 {
            return;
        }
        shapes::fill(g, cx - r, cy - r, r * 2, r * 2, (r / 3).clamp(1, 3), color);
    }

    fn tri_width(w: i32, h: i32, row: i32) -> i32 {
        let t = 1.0 - (((row as f32 + 0.5) / h as f32) * 2.0 - 1.0).abs();
        (w as f32 * t).round().max(1.0) as i32
    }

    pub fn tri(g: &mut DrawList, x: i32, cy: i32, w: i32, h: i32, right: bool, color: u32) {
        if h <= 0 {
            return;
        }
        let top = cy - h / 2;
        let mut start = 0;
        let mut run_w = Self::tri_width(w, h, 0);
        for r in 1..=h {
            let cw = if r < h { Self::tri_width(w, h, r) } else { -1 };
            if cw == run_w {
                continue;
            }
            if right {
                g.fill(
                    x as f32,
                    (top + start) as f32,
                    (x + run_w) as f32,
                    (top + r) as f32,
                    color,
                );
            } else {
                g.fill(
                    (x + w - run_w) as f32,
                    (top + start) as f32,
                    (x + w) as f32,
                    (top + r) as f32,
                    color,
                );
            }
            start = r;
            run_w = cw;
        }
    }

    pub fn icon_play(g: &mut DrawList, cx: i32, cy: i32, h: i32, color: u32) {
        let w = (h as f32 * 0.8).round() as i32;
        Self::tri(g, cx - w / 2 + 1, cy, w, h, true, color);
    }

    pub fn icon_pause(g: &mut DrawList, cx: i32, cy: i32, h: i32, color: u32) {
        let bw = (h / 4).max(2);
        let gap = (h / 4).max(2);
        let x0 = cx - (bw * 2 + gap) / 2;
        g.fill(
            x0 as f32,
            (cy - h / 2) as f32,
            (x0 + bw) as f32,
            (cy - h / 2 + h) as f32,
            color,
        );
        g.fill(
            (x0 + bw + gap) as f32,
            (cy - h / 2) as f32,
            (x0 + bw * 2 + gap) as f32,
            (cy - h / 2 + h) as f32,
            color,
        );
    }

    pub fn icon_skip(g: &mut DrawList, cx: i32, cy: i32, h: i32, color: u32, forward: bool) {
        let w = (h as f32 * 0.7).round() as i32;
        let bar = 2;
        let x0 = cx - (w + bar + 1) / 2;
        if forward {
            Self::tri(g, x0, cy, w, h, true, color);
            g.fill(
                (x0 + w + 1) as f32,
                (cy - h / 2) as f32,
                (x0 + w + 1 + bar) as f32,
                (cy - h / 2 + h) as f32,
                color,
            );
        } else {
            g.fill(
                x0 as f32,
                (cy - h / 2) as f32,
                (x0 + bar) as f32,
                (cy - h / 2 + h) as f32,
                color,
            );
            Self::tri(g, x0 + bar + 1, cy, w, h, false, color);
        }
    }

    pub fn icon_volume(g: &mut DrawList, cx: i32, cy: i32, waves: i32, color: u32) {
        g.fill(
            (cx - 5) as f32,
            (cy - 2) as f32,
            (cx - 2) as f32,
            (cy + 2) as f32,
            color,
        );
        for i in 0..4 {
            g.fill(
                (cx - 2 + i) as f32,
                (cy - 2 - i) as f32,
                (cx - 1 + i) as f32,
                (cy + 2 + i) as f32,
                color,
            );
        }
        if waves >= 1 {
            g.fill(
                (cx + 4) as f32,
                (cy - 2) as f32,
                (cx + 5) as f32,
                (cy + 2) as f32,
                color,
            );
        }
        if waves >= 2 {
            g.fill(
                (cx + 6) as f32,
                (cy - 4) as f32,
                (cx + 7) as f32,
                (cy + 4) as f32,
                color,
            );
        }
        if waves == 0 {
            for i in 0..4 {
                g.fill(
                    (cx + 3 + i) as f32,
                    (cy - 2 + i) as f32,
                    (cx + 4 + i) as f32,
                    (cy - 1 + i) as f32,
                    color,
                );
                g.fill(
                    (cx + 3 + i) as f32,
                    (cy + 1 - i) as f32,
                    (cx + 4 + i) as f32,
                    (cy + 2 - i) as f32,
                    color,
                );
            }
        }
    }

    pub fn icon_heart(g: &mut DrawList, x: i32, y: i32, color: u32) {
        const HEART: &[&str] = &[
            ".XX.XX.", "XXXXXXX", "XXXXXXX", ".XXXXX.", "..XXX..", "...X...",
        ];
        let mut row = 0usize;
        while row < HEART.len() {
            let mut rows = 1usize;
            while row + rows < HEART.len() && HEART[row + rows] == HEART[row] {
                rows += 1;
            }
            let bytes = HEART[row].as_bytes();
            let mut col = 0usize;
            while col < bytes.len() {
                if bytes[col] != b'X' {
                    col += 1;
                    continue;
                }
                let mut end = col;
                while end < bytes.len() && bytes[end] == b'X' {
                    end += 1;
                }
                g.fill(
                    (x + col as i32) as f32,
                    (y + row as i32) as f32,
                    (x + end as i32) as f32,
                    (y + row as i32 + rows as i32) as f32,
                    color,
                );
                col = end;
            }
            row += rows;
        }
    }
}

/// 24x24 platform icons matching `PlatformIcons.java`.
pub fn draw_platform_icon(g: &mut DrawList, p: Platform, x: i32, y: i32, size: i32) {
    let s = size as f32 / 24.0;
    let cx = x + (12.0 * s).round() as i32;
    let cy = y + (12.0 * s).round() as i32;
    let r = (10.5 * s).round() as i32;
    match p {
        Platform::Yandex => {
            PlayerTheme::circle(g, cx, cy, r, 0xFFFF_CC00);
            let sw = (2.6 * s).round().max(1.0) as i32;
            let top = y + (6.0 * s).round() as i32;
            let mid = y + (13.0 * s).round() as i32;
            let bot = y + (18.0 * s).round() as i32;
            g.fill(
                (cx + 1) as f32,
                top as f32,
                (cx + 1 + sw) as f32,
                bot as f32,
                0xFF14_1416,
            );
            let h = mid - top;
            let k = (4.5 * s).round().max(2.0) as i32;
            for i in 0..h {
                let lx = cx + 1 - k + i * k / h.max(1);
                g.fill(
                    lx as f32,
                    (top + i) as f32,
                    (lx + sw) as f32,
                    (top + i + 1) as f32,
                    0xFF14_1416,
                );
            }
            let h2 = bot - mid;
            for i in 0..h2 {
                let lx = cx + 1 - i * k / h2.max(1);
                g.fill(
                    lx as f32,
                    (mid + i) as f32,
                    (lx + sw) as f32,
                    (mid + i + 1) as f32,
                    0xFF14_1416,
                );
            }
        }
        Platform::SoundCloud => {
            let bx = x + s.round() as i32;
            let by = y + (s * 2.0).round() as i32;
            let bw_box = (size - (bx - x) * 2).max(4);
            let bh_box = (size - (by - y) * 2).max(4);
            let rad = (4.0 * s).round().clamp(2.0, 6.0) as i32;
            shapes::fill(g, bx, by, bw_box, bh_box, rad, 0xFFFF_5500);
            const BARS_WIDE: &[f32] = &[0.32, 0.56, 0.42, 0.74, 0.62, 0.88, 0.72, 0.92, 0.66];
            const BARS_MINI: &[f32] = &[0.45, 0.85, 0.62, 0.94, 0.70];
            let bars = if bw_box < 14 { BARS_MINI } else { BARS_WIDE };
            let pad_x = (2.2 * s).max(1.5);
            let inner_w = (bw_box as f32 - pad_x * 2.0).max(2.0);
            let slot = inner_w / bars.len() as f32;
            let bar_w = (slot * 0.62).max(0.85);
            let x_start = bx as f32 + pad_x + (slot - bar_w) * 0.5;
            let base_y = (by + bh_box) as f32 - (bh_box as f32 * 0.20).max(1.5);
            let max_h = (bh_box as f32 * 0.58).max(2.0);
            for (i, hfrac) in bars.iter().enumerate() {
                let h = (hfrac * max_h).max(1.5);
                let fx0 = x_start + i as f32 * slot;
                g.fill(fx0, base_y - h, fx0 + bar_w, base_y, 0xFFFF_FFFF);
            }
        }
        Platform::Local => {
            let fg = 0xFF34_D399;
            let fx = x + (3.0 * s).round() as i32;
            let fw = (18.0 * s).round() as i32;
            let fy = y + (7.0 * s).round() as i32;
            let fh = (12.0 * s).round() as i32;
            shapes::fill(
                g,
                fx,
                y + (4.5 * s).round() as i32,
                (8.0 * s).round() as i32,
                (4.0 * s).round() as i32,
                1,
                fg,
            );
            shapes::fill(g, fx, fy, fw, fh, 2, fg);
            let sw = (2.0 * s).round().max(1.0) as i32;
            let ny = y + (10.0 * s).round() as i32;
            let ny2 = y + (15.5 * s).round() as i32;
            g.fill(
                (cx + 1) as f32,
                ny as f32,
                (cx + 1 + sw) as f32,
                ny2 as f32,
                0xFF0E_3D2A,
            );
            g.fill(
                (cx - sw + 1) as f32,
                (ny2 - sw) as f32,
                (cx + 1 + sw) as f32,
                (ny2 + 1) as f32,
                0xFF0E_3D2A,
            );
            g.fill(
                (cx + 1) as f32,
                ny as f32,
                (cx + (5.0 * s).round() as i32) as f32,
                (ny + sw) as f32,
                0xFF0E_3D2A,
            );
        }
        Platform::Spotify => {
            PlayerTheme::circle(g, cx, cy, r, 0xFF1D_B954);
            let line = 0xFF11_1416;
            for row in [-3i32, 0, 3] {
                let half = match row {
                    -3 => (6.0 * s).round() as i32,
                    0 => (5.0 * s).round() as i32,
                    _ => (4.0 * s).round() as i32,
                };
                let ly = cy + (row as f32 * s).round() as i32;
                let th = (1.6 * s).round().max(1.0) as i32;
                shapes::fill(g, cx - half, ly - th / 2, half * 2, th, 1, line);
            }
        }
        Platform::YouTube => {
            let bx = x + s.round() as i32;
            let by = y + (s * 4.0).round() as i32;
            let bw_box = (size - (bx - x) * 2).max(6);
            let bh_box = (size - (by - y) * 2).max(4);
            let rad = (4.5 * s).round().clamp(2.0, 6.0) as i32;
            shapes::fill(g, bx, by, bw_box, bh_box, rad, 0xFFFF_0000);
            let th = (bh_box as f32 * 0.56).round().max(4.0) as i32;
            let half = th / 2;
            let tri_w = (half as f32 * 1.15).round().max(2.0) as i32;
            let x0 = cx - tri_w / 2 + 1;
            for dx in 0..tri_w {
                let h = ((tri_w - dx) * half / tri_w.max(1)).max(1);
                g.fill(
                    (x0 + dx) as f32,
                    (cy - h) as f32,
                    (x0 + dx + 1) as f32,
                    (cy + h + 1) as f32,
                    0xFFFF_FFFF,
                );
            }
        }
    }
}
