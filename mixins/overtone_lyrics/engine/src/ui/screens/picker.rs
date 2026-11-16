use crate::controller::PlayerController;
use crate::i18n;
use crate::ui::cover::{ModImageAtlas, hsv_to_rgb, rgb_to_hsv};
use crate::ui::screens::{PickerTarget, ScreenKey, ScreenRoute, TextInput};
use crate::ui::{DrawList, PlayerTheme, TexSlot, TextMetrics, shapes};

const SIZE: i32 = 120;
const HUE_W: i32 = 14;
const PW: i32 = 196;
const PH: i32 = 232;

#[derive(Clone, Debug)]
pub struct ColorPickerState {
    target: PickerTarget,
    hue: f32,
    sat: f32,
    val: f32,
    px: i32,
    py: i32,
    sx: i32,
    sy: i32,
    hx: i32,
    dragging: u8,
    hex: TextInput,
    ok_rect: [i32; 4],
    cancel_rect: [i32; 4],
}

impl ColorPickerState {
    pub fn new(initial_rgb: u32, target: PickerTarget) -> Self {
        let hsv = rgb_to_hsv(initial_rgb & 0xFF_FFFF);
        let mut s = Self {
            target,
            hue: hsv[0],
            sat: hsv[1],
            val: hsv[2],
            px: 0,
            py: 0,
            sx: 0,
            sy: 0,
            hx: 0,
            dragging: 0,
            hex: TextInput::new("#RRGGBB", 7),
            ok_rect: [0; 4],
            cancel_rect: [0; 4],
        };
        s.sync_hex();
        s
    }

    pub fn is_text_focused(&self) -> bool {
        self.hex.focused
    }

    pub fn init(&mut self, sw: i32, sh: i32) {
        self.px = (sw - PW) / 2;
        self.py = (sh - PH) / 2;
        self.sx = self.px + 16;
        self.sy = self.py + 34;
        self.hx = self.sx + SIZE + 12;
        let fy = self.sy + SIZE + 14;
        self.hex.set_bounds(self.px + 16 + 48, fy, PW - 32 - 48, 20);
        let by = fy + 26;
        let bw = (PW - 32 - 8) / 2;
        self.ok_rect = [self.px + 16, by, bw, 20];
        self.cancel_rect = [self.px + 16 + bw + 8, by, bw, 20];
    }

    pub fn current_rgb(&self) -> u32 {
        hsv_to_rgb(self.hue, self.sat, self.val)
    }

    fn sync_hex(&mut self) {
        self.hex.set_value(format!("#{:06X}", self.current_rgb()));
    }

    fn try_parse_hex(&mut self) {
        let trimmed = self.hex.value.trim().trim_start_matches('#');
        if trimmed.len() == 6
            && let Ok(rgb) = u32::from_str_radix(trimmed, 16)
        {
            let hsv = rgb_to_hsv(rgb);
            if hsv[1] > 0.01 && hsv[2] > 0.01 {
                self.hue = hsv[0];
            }
            self.sat = hsv[1];
            self.val = hsv[2];
        }
    }

    fn pick_sv(&mut self, mx: i32, my: i32) {
        self.sat = ((mx - self.sx) as f32 / SIZE as f32).clamp(0.0, 1.0);
        self.val = 1.0 - ((my - self.sy) as f32 / SIZE as f32).clamp(0.0, 1.0);
        self.sync_hex();
    }

    fn pick_hue(&mut self, my: i32) {
        self.hue = ((my - self.sy) as f32 / SIZE as f32).clamp(0.0, 0.9999);
        self.sync_hex();
    }

    fn commit(&self, ctl: &PlayerController) {
        let rgb = (self.current_rgb() & 0xFF_FFFF) as i32;
        match self.target {
            PickerTarget::PlayerAccent => ctl.update_player_config(|p| {
                p.custom_accent = rgb;
                p.sync_accent = false;
            }),
            PickerTarget::HudAccent => ctl.update_hud_config(|h| {
                h.custom_accent = rgb;
            }),
            PickerTarget::WorldAccent2 => {
                // Keep closest Accent enum for HUD world gradient and apply custom hue as custom_accent if needed
                ctl.update_hud_config(|h| {
                    h.custom_accent = rgb;
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        atlas: &mut ModImageAtlas,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
    ) {
        let th = PlayerTheme::from_config(&ctl.player_config(), &ctl.hud_config());
        g.fill(0.0, 0.0, sw as f32, sh as f32, th.dim);
        th.window(g, self.px, self.py, PW, PH);
        g.text(
            i18n::raw("picker.title"),
            (self.px + 16) as f32,
            (self.py + 12) as f32,
            th.accent,
            false,
        );

        atlas.sync_picker(self.hue);

        g.texture(
            TexSlot::PickerSquare,
            self.sx as f32,
            self.sy as f32,
            SIZE as f32,
            SIZE as f32,
            1.0,
        );
        let kx = self.sx + (self.sat * SIZE as f32).round() as i32;
        let ky = self.sy + ((1.0 - self.val) * SIZE as f32).round() as i32;
        shapes::ring(g, kx - 3, ky - 3, 7, 7, 1, 0xFFFF_FFFF);

        g.texture(
            TexSlot::PickerHue,
            self.hx as f32,
            self.sy as f32,
            HUE_W as f32,
            SIZE as f32,
            1.0,
        );
        let hy = self.sy + (self.hue * SIZE as f32).round() as i32;
        g.fill(
            (self.hx - 2) as f32,
            (hy - 1) as f32,
            (self.hx + HUE_W + 2) as f32,
            (hy + 2) as f32,
            0xFFFF_FFFF,
        );

        let fy = self.sy + SIZE + 14;
        shapes::fill(
            g,
            self.px + 16,
            fy,
            40,
            20,
            4,
            0xFF00_0000 | self.current_rgb(),
        );
        shapes::ring(g, self.px + 16, fy, 40, 20, 4, th.track_bg);

        self.hex.render(
            g,
            metrics,
            th.radius,
            th.accent,
            th.track_bg,
            th.field_bg,
            PlayerTheme::TEXT_MAIN,
            PlayerTheme::TEXT_DIM,
        );

        draw_picker_btn(
            g,
            metrics,
            self.ok_rect,
            &i18n::raw("btn.ok"),
            true,
            th.accent,
            th.btn_bg,
            mx,
            my,
        );
        draw_picker_btn(
            g,
            metrics,
            self.cancel_rect,
            &i18n::raw("btn.cancel"),
            false,
            th.accent,
            th.btn_bg,
            mx,
            my,
        );
    }

    pub fn mouse_down(
        &mut self,
        mx: i32,
        my: i32,
        _button: i32,
        ctl: &PlayerController,
        _sw: i32,
        _sh: i32,
    ) -> ScreenRoute {
        self.hex.focused = self.hex.hit(mx, my);
        if mx >= self.sx && mx < self.sx + SIZE && my >= self.sy && my < self.sy + SIZE {
            self.dragging = 1;
            self.pick_sv(mx, my);
            return ScreenRoute::Stay;
        }
        if mx >= self.hx && mx < self.hx + HUE_W && my >= self.sy && my < self.sy + SIZE {
            self.dragging = 2;
            self.pick_hue(my);
            return ScreenRoute::Stay;
        }
        if hit_rect(self.ok_rect, mx, my) {
            self.commit(ctl);
            return ScreenRoute::Pop;
        }
        if hit_rect(self.cancel_rect, mx, my) {
            return ScreenRoute::Pop;
        }
        ScreenRoute::Stay
    }

    pub fn mouse_drag(&mut self, mx: i32, my: i32) {
        if self.dragging == 1 {
            self.pick_sv(mx, my);
        } else if self.dragging == 2 {
            self.pick_hue(my);
        }
    }

    pub fn mouse_up(&mut self) {
        self.dragging = 0;
    }

    pub fn key_pressed(
        &mut self,
        key: ScreenKey,
        ctrl: bool,
        ctl: &PlayerController,
    ) -> (bool, ScreenRoute) {
        if key == ScreenKey::Escape {
            return (true, ScreenRoute::Pop);
        }
        if key == ScreenKey::Enter {
            self.commit(ctl);
            return (true, ScreenRoute::Pop);
        }
        if self.hex.handle_key(key, ctrl) {
            self.try_parse_hex();
            return (true, ScreenRoute::Stay);
        }
        (false, ScreenRoute::Stay)
    }

    pub fn char_typed(&mut self, ch: char) -> bool {
        if !self.hex.focused {
            return false;
        }
        if !(ch == '#' || ch.is_ascii_hexdigit()) {
            return false;
        }
        let ok = self.hex.insert_char(ch);
        if ok {
            self.try_parse_hex();
        }
        ok
    }

    pub fn paste_text(&mut self, txt: &str) -> bool {
        if !self.hex.focused {
            return false;
        }
        let filtered: String = txt
            .chars()
            .filter(|c| *c == '#' || c.is_ascii_hexdigit())
            .collect();
        let ok = self.hex.insert_str(&filtered);
        if ok {
            self.try_parse_hex();
        }
        ok
    }
}

fn hit_rect(r: [i32; 4], mx: i32, my: i32) -> bool {
    r[2] > 0 && mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3]
}

#[allow(clippy::too_many_arguments)]
fn draw_picker_btn(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    r: [i32; 4],
    label: &str,
    primary: bool,
    accent: u32,
    btn_bg: u32,
    mx: i32,
    my: i32,
) {
    let hov = hit_rect(r, mx, my);
    let bg = if primary {
        if hov {
            PlayerTheme::lighten(accent, 0.14)
        } else {
            accent
        }
    } else if hov {
        PlayerTheme::lighten(btn_bg, 0.14)
    } else {
        btn_bg
    };
    shapes::fill(g, r[0], r[1], r[2], r[3], 4, bg);
    let tw = metrics.width(label);
    let tx = r[0] as f32 + (r[2] as f32 - tw) / 2.0;
    let ty = r[1] as f32 + (r[3] as f32 - metrics.line_height()) / 2.0;
    g.text(label, tx, ty, PlayerTheme::TEXT_MAIN, false);
}
