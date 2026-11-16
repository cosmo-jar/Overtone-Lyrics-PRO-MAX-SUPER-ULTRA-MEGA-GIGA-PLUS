use crate::config::HudConfig;
use crate::controller::PlayerController;
use crate::i18n;
use crate::ui::cover::CoverArtManager;
use crate::ui::hud::{HudBounds, HudBox, HudState, anchor_hv};
use crate::ui::screens::{ScreenKey, ScreenRoute};
use crate::ui::{DrawList, PlayerTheme, TextMetrics, shapes};

const SNAP: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    Widget,
    Lyrics,
    Volume,
    Visualizer,
}

impl EditTarget {
    pub const ORDER: [Self; 4] = [Self::Visualizer, Self::Volume, Self::Lyrics, Self::Widget];

    pub fn label(self) -> String {
        let k = match self {
            Self::Widget => "edit.target.widget",
            Self::Lyrics => "edit.target.lyrics",
            Self::Volume => "edit.target.volume",
            Self::Visualizer => "edit.target.visualizer",
        };
        i18n::get(k)
    }
}

#[derive(Clone, Debug)]
pub struct HudEditScreenState {
    pub selected: EditTarget,
    dragging: Option<EditTarget>,
    gesture_decided: bool,
    start_x: i32,
    start_y: i32,
    acc_x: i32,
    acc_y: i32,
    done_rect: [i32; 4],
    reset_rect: [i32; 4],
}

impl Default for HudEditScreenState {
    fn default() -> Self {
        Self::new()
    }
}

impl HudEditScreenState {
    pub fn new() -> Self {
        Self {
            selected: EditTarget::Widget,
            dragging: None,
            gesture_decided: false,
            start_x: 0,
            start_y: 0,
            acc_x: 0,
            acc_y: 0,
            done_rect: [0; 4],
            reset_rect: [0; 4],
        }
    }

    pub fn init(&mut self, sw: i32, sh: i32) {
        let cx = sw / 2;
        let y = sh - 28;
        self.done_rect = [cx - 125, y, 120, 20];
        self.reset_rect = [cx + 5, y, 120, 20];
    }

    fn box_of(bounds: &HudBounds, t: EditTarget) -> Option<HudBox> {
        match t {
            EditTarget::Widget => bounds.widget,
            EditTarget::Lyrics => bounds.lyrics,
            EditTarget::Volume => bounds.volume,
            EditTarget::Visualizer => bounds.viz,
        }
    }

    fn pick(bounds: &HudBounds, mx: i32, my: i32) -> Option<EditTarget> {
        for t in EditTarget::ORDER {
            if let Some(b) = Self::box_of(bounds, t)
                && b.contains(mx, my)
            {
                return Some(t);
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn move_to(
        bounds: &HudBounds,
        t: EditTarget,
        nx: i32,
        ny: i32,
        snap: bool,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) {
        let Some(b) = Self::box_of(bounds, t) else {
            return;
        };
        let mut x = nx;
        if snap && (x + b.w / 2 - sw / 2).abs() <= SNAP {
            x = sw / 2 - b.w / 2;
        }
        let anchor = choose_anchor(x + b.w / 2, ny + b.h / 2, sw, sh);
        let (ox, oy) = HudBounds::place_offset(anchor, b.w, b.h, sw, sh, x, ny);
        ctl.update_hud_config(|c| match t {
            EditTarget::Widget => {
                c.anchor = anchor;
                c.offset_x = ox;
                c.offset_y = oy;
            }
            EditTarget::Lyrics => {
                c.lyrics_anchor = anchor;
                c.lyrics_offset_x = ox;
                c.lyrics_offset_y = oy;
            }
            EditTarget::Volume => {
                c.volume_anchor = anchor;
                c.volume_offset_x = ox;
                c.volume_offset_y = oy;
            }
            EditTarget::Visualizer => {
                c.viz_anchor = anchor;
                c.viz_offset_x = ox;
                c.viz_offset_y = oy;
            }
        });
    }

    fn nudge(t: EditTarget, dx: i32, dy: i32, ctl: &PlayerController) {
        ctl.update_hud_config(|c| match t {
            EditTarget::Widget => {
                let (h, v) = anchor_hv(c.anchor);
                c.offset_x += if h > 0 { -dx } else { dx };
                c.offset_y += if v > 0 { -dy } else { dy };
            }
            EditTarget::Lyrics => {
                let (h, v) = anchor_hv(c.lyrics_anchor);
                c.lyrics_offset_x += if h > 0 { -dx } else { dx };
                c.lyrics_offset_y += if v > 0 { -dy } else { dy };
            }
            EditTarget::Volume => {
                let (h, v) = anchor_hv(c.volume_anchor);
                c.volume_offset_x += if h > 0 { -dx } else { dx };
                c.volume_offset_y += if v > 0 { -dy } else { dy };
            }
            EditTarget::Visualizer => {
                let (h, v) = anchor_hv(c.viz_anchor);
                c.viz_offset_x += if h > 0 { -dx } else { dx };
                c.viz_offset_y += if v > 0 { -dy } else { dy };
            }
        });
    }

    fn step_scale(t: EditTarget, sy: f32, ctl: &PlayerController) {
        let step = if sy > 0.0 { 0.05 } else { -0.05 };
        ctl.update_hud_config(|c| match t {
            EditTarget::Widget => c.scale = (c.scale + step).clamp(0.5, 2.0),
            EditTarget::Lyrics => c.lyrics_scale = (c.lyrics_scale + step).clamp(0.5, 2.0),
            EditTarget::Volume => c.volume_scale = (c.volume_scale + step).clamp(0.5, 2.0),
            EditTarget::Visualizer => c.viz_scale = (c.viz_scale + step).clamp(0.5, 2.0),
        });
    }

    fn reset_positions(ctl: &PlayerController) {
        let def = HudConfig::default();
        ctl.update_hud_config(|c| {
            c.anchor = def.anchor;
            c.offset_x = def.offset_x;
            c.offset_y = def.offset_y;
            c.scale = def.scale;
            c.lyrics_anchor = def.lyrics_anchor;
            c.lyrics_offset_x = def.lyrics_offset_x;
            c.lyrics_offset_y = def.lyrics_offset_y;
            c.lyrics_scale = def.lyrics_scale;
            c.volume_anchor = def.volume_anchor;
            c.volume_offset_x = def.volume_offset_x;
            c.volume_offset_y = def.volume_offset_y;
            c.volume_scale = def.volume_scale;
            c.viz_anchor = def.viz_anchor;
            c.viz_offset_x = def.viz_offset_x;
            c.viz_offset_y = def.viz_offset_y;
            c.viz_scale = def.viz_scale;
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mouse_down(
        &mut self,
        mx: i32,
        my: i32,
        _button: i32,
        hud: &HudState,
        ctl: &PlayerController,
        _sw: i32,
        _sh: i32,
    ) -> ScreenRoute {
        if hit_rect(self.done_rect, mx, my) {
            return ScreenRoute::Pop;
        }
        if hit_rect(self.reset_rect, mx, my) {
            Self::reset_positions(ctl);
            return ScreenRoute::Stay;
        }
        if let Some(hit) = Self::pick(&hud.bounds, mx, my) {
            self.selected = hit;
            self.dragging = Some(hit);
            self.gesture_decided = false;
            self.start_x = mx;
            self.start_y = my;
            self.acc_x = 0;
            self.acc_y = 0;
        }
        ScreenRoute::Stay
    }

    pub fn mouse_drag(
        &mut self,
        mx: i32,
        my: i32,
        hud: &HudState,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) {
        let Some(t) = self.dragging else {
            return;
        };
        let Some(b) = Self::box_of(&hud.bounds, t) else {
            return;
        };
        let dx = mx - self.start_x;
        let dy = my - self.start_y;
        self.start_x = mx;
        self.start_y = my;
        self.acc_x += dx;
        self.acc_y += dy;
        if !self.gesture_decided && (self.acc_x.abs() > 2 || self.acc_y.abs() > 2) {
            self.gesture_decided = true;
        }
        if self.gesture_decided {
            let nx = (b.x + self.acc_x).clamp(0, (sw - b.w).max(0));
            let ny = (b.y + self.acc_y).clamp(0, (sh - b.h).max(0));
            self.acc_x = 0;
            self.acc_y = 0;
            Self::move_to(&hud.bounds, t, nx, ny, true, ctl, sw, sh);
        }
    }

    pub fn mouse_up(&mut self) {
        self.dragging = None;
    }

    pub fn mouse_scroll(
        &mut self,
        mx: i32,
        my: i32,
        sy: f32,
        hud: &HudState,
        ctl: &PlayerController,
    ) {
        let t = Self::pick(&hud.bounds, mx, my).unwrap_or(self.selected);
        self.selected = t;
        Self::step_scale(t, sy, ctl);
    }

    pub fn key_pressed(
        &mut self,
        key: ScreenKey,
        shift: bool,
        ctl: &PlayerController,
    ) -> (bool, ScreenRoute) {
        let step = if shift { 10 } else { 1 };
        match key {
            ScreenKey::Escape => (true, ScreenRoute::Pop),
            ScreenKey::Tab => {
                self.selected = match self.selected {
                    EditTarget::Widget => EditTarget::Lyrics,
                    EditTarget::Lyrics => EditTarget::Volume,
                    EditTarget::Volume => EditTarget::Visualizer,
                    EditTarget::Visualizer => EditTarget::Widget,
                };
                (true, ScreenRoute::Stay)
            }
            ScreenKey::Left => {
                Self::nudge(self.selected, -step, 0, ctl);
                (true, ScreenRoute::Stay)
            }
            ScreenKey::Right => {
                Self::nudge(self.selected, step, 0, ctl);
                (true, ScreenRoute::Stay)
            }
            ScreenKey::Up => {
                Self::nudge(self.selected, 0, -step, ctl);
                (true, ScreenRoute::Stay)
            }
            ScreenKey::Down => {
                Self::nudge(self.selected, 0, step, ctl);
                (true, ScreenRoute::Stay)
            }
            _ => (false, ScreenRoute::Stay),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        hud: &mut HudState,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
        dt: f32,
    ) {
        let th = PlayerTheme::from_config(&ctl.player_config(), &ctl.hud_config());
        g.fill(
            0.0,
            0.0,
            sw as f32,
            sh as f32,
            PlayerTheme::argb(0x00_0000, 90),
        );
        // Guide centerline
        g.fill(
            (sw / 2) as f32,
            0.0,
            (sw / 2 + 1) as f32,
            sh as f32,
            PlayerTheme::argb(0xFF_FFFF, 28),
        );
        g.fill(
            0.0,
            (sh / 2) as f32,
            sw as f32,
            (sh / 2 + 1) as f32,
            PlayerTheme::argb(0xFF_FFFF, 28),
        );

        hud.render_all(
            g, metrics, ctl, covers, sw, sh, dt, true, false, false, false, false,
        );

        for t in [
            EditTarget::Widget,
            EditTarget::Lyrics,
            EditTarget::Volume,
            EditTarget::Visualizer,
        ] {
            if let Some(b) = Self::box_of(&hud.bounds, t) {
                let is_sel = t == self.selected;
                let border = if is_sel {
                    th.accent
                } else {
                    PlayerTheme::argb(0xFF_FFFF, 80)
                };
                shapes::ring(g, b.x - 1, b.y - 1, b.w + 2, b.h + 2, 2, border);
                let tag = t.label();
                g.text(tag, b.x as f32, (b.y - 10).max(2) as f32, border, true);
            }
        }

        let tip = i18n::fmt("edit.hint", &[&self.selected.label()]);
        let tw = metrics.width(&tip);
        g.text(
            tip,
            (sw as f32 - tw) / 2.0,
            12.0,
            PlayerTheme::TEXT_MAIN,
            true,
        );

        draw_edit_btn(
            g,
            metrics,
            self.done_rect,
            &i18n::get("btn.done"),
            true,
            &th,
            mx,
            my,
        );
        draw_edit_btn(
            g,
            metrics,
            self.reset_rect,
            &i18n::get("edit.reset"),
            false,
            &th,
            mx,
            my,
        );
    }
}

fn choose_anchor(cx: i32, cy: i32, sw: i32, sh: i32) -> crate::config::Anchor {
    use crate::config::Anchor;
    let h = if cx < sw / 3 {
        -1
    } else if cx > (sw * 2) / 3 {
        1
    } else {
        0
    };
    let v = if cy < sh / 3 {
        -1
    } else if cy > (sh * 2) / 3 {
        1
    } else {
        0
    };
    match (h, v) {
        (-1, -1) => Anchor::TopLeft,
        (0, -1) => Anchor::TopCenter,
        (1, -1) => Anchor::TopRight,
        (-1, 0) => Anchor::MidLeft,
        (0, 0) => Anchor::Center,
        (1, 0) => Anchor::MidRight,
        (-1, 1) => Anchor::BottomLeft,
        (0, 1) => Anchor::BottomCenter,
        _ => Anchor::BottomRight,
    }
}

fn hit_rect(r: [i32; 4], mx: i32, my: i32) -> bool {
    mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3]
}

#[allow(clippy::too_many_arguments)]
fn draw_edit_btn(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    r: [i32; 4],
    label: &str,
    primary: bool,
    th: &PlayerTheme,
    mx: i32,
    my: i32,
) {
    let hov = hit_rect(r, mx, my);
    let bg = if primary {
        if hov {
            PlayerTheme::lighten(th.accent, 0.14)
        } else {
            th.accent
        }
    } else if hov {
        PlayerTheme::lighten(th.btn_bg, 0.14)
    } else {
        th.btn_bg
    };
    shapes::fill(g, r[0], r[1], r[2], r[3], 4, bg);
    let tw = metrics.width(label);
    g.text(
        label,
        r[0] as f32 + (r[2] as f32 - tw) / 2.0,
        r[1] as f32 + (r[3] as f32 - metrics.line_height()) / 2.0,
        PlayerTheme::TEXT_MAIN,
        false,
    );
}
