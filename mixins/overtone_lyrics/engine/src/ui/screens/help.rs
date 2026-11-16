use crate::config::{HotkeyAction, HotkeyConfig};
use crate::controller::PlayerController;
use crate::i18n;
use crate::model::MOD_NAME;
use crate::ui::screens::{ScreenKey, ScreenRoute};
use crate::ui::{DrawList, PlayerTheme, TextMetrics, shapes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ill {
    None,
    KeyM,
    Cards,
    Search,
    Keys,
    LoginBtn,
    Address,
    Paste,
    Drop,
}

struct Step {
    id: &'static str,
    ill: Ill,
}

const PAGE_IDS: [&str; 5] = ["quick", "yandex", "other", "keys", "faq"];

const PAGES: [Option<&[Step]>; 5] = [
    Some(&[
        Step {
            id: "quick.1",
            ill: Ill::KeyM,
        },
        Step {
            id: "quick.2",
            ill: Ill::Cards,
        },
        Step {
            id: "quick.3",
            ill: Ill::Search,
        },
        Step {
            id: "quick.4",
            ill: Ill::Keys,
        },
    ]),
    Some(&[
        Step {
            id: "yandex.1",
            ill: Ill::LoginBtn,
        },
        Step {
            id: "yandex.2",
            ill: Ill::None,
        },
        Step {
            id: "yandex.3",
            ill: Ill::Address,
        },
        Step {
            id: "yandex.4",
            ill: Ill::Paste,
        },
    ]),
    Some(&[
        Step {
            id: "other.1",
            ill: Ill::None,
        },
        Step {
            id: "other.2",
            ill: Ill::None,
        },
        Step {
            id: "other.3",
            ill: Ill::Drop,
        },
        Step {
            id: "other.4",
            ill: Ill::None,
        },
    ]),
    None,
    Some(&[
        Step {
            id: "faq.1",
            ill: Ill::None,
        },
        Step {
            id: "faq.2",
            ill: Ill::None,
        },
        Step {
            id: "faq.3",
            ill: Ill::None,
        },
        Step {
            id: "faq.4",
            ill: Ill::None,
        },
    ]),
];

const ILL_W: i32 = 118;

#[derive(Clone, Debug)]
pub struct HelpScreenState {
    pub page: usize,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    back_rect: [i32; 4],
    next_rect: [i32; 4],
}

impl Default for HelpScreenState {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpScreenState {
    pub fn new() -> Self {
        Self {
            page: 0,
            px: 0,
            py: 0,
            pw: 460,
            ph: 320,
            back_rect: [0; 4],
            next_rect: [0; 4],
        }
    }

    pub fn init(&mut self, sw: i32, sh: i32) {
        self.pw = (sw - 16).min(460);
        self.ph = (sh - 16).min(320);
        self.px = (sw - self.pw) / 2;
        self.py = (sh - self.ph) / 2;
        self.back_rect = [self.px + 14, self.py + self.ph - 28, 72, 18];
        self.next_rect = [self.px + self.pw - 14 - 72, self.py + self.ph - 28, 72, 18];
    }

    fn go(&mut self, delta: isize) -> ScreenRoute {
        let next = self.page as isize + delta;
        if next < 0 {
            return ScreenRoute::Stay;
        }
        if next as usize >= PAGES.len() {
            return ScreenRoute::Pop;
        }
        self.page = next as usize;
        ScreenRoute::Stay
    }

    pub fn mouse_down(&mut self, mx: i32, my: i32, _button: i32) -> ScreenRoute {
        if self.page > 0 && in_rect(self.back_rect, mx, my) {
            return self.go(-1);
        }
        if in_rect(self.next_rect, mx, my) {
            return self.go(1);
        }
        let dots_y = self.py + self.ph - 22;
        let x0 = self.px + (self.pw - (PAGES.len() as i32 * 14)) / 2;
        if my >= dots_y - 4 && my < dots_y + 10 && mx >= x0 {
            let idx = ((mx - x0) / 14) as usize;
            if idx < PAGES.len() {
                self.page = idx;
            }
        }
        ScreenRoute::Stay
    }

    pub fn key_pressed(&mut self, key: ScreenKey) -> (bool, ScreenRoute) {
        match key {
            ScreenKey::Escape => (true, ScreenRoute::Pop),
            ScreenKey::Right | ScreenKey::Enter => (true, self.go(1)),
            ScreenKey::Left => (true, self.go(-1)),
            _ => (false, ScreenRoute::Stay),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
    ) {
        let th = PlayerTheme::from_config(&ctl.player_config(), &ctl.hud_config());
        let hk = ctl.hotkey_config();
        g.fill(0.0, 0.0, sw as f32, sh as f32, th.dim);
        th.window(g, self.px, self.py, self.pw, self.ph);

        g.text(
            format!("♪ {MOD_NAME}"),
            (self.px + 14) as f32,
            (self.py + 10) as f32,
            th.accent,
            false,
        );
        let page_title = i18n::get(&format!("help.page.{}", PAGE_IDS[self.page]));
        let ptw = metrics.width(&page_title);
        g.text(
            page_title,
            (self.px + self.pw - 14) as f32 - ptw,
            (self.py + 10) as f32,
            PlayerTheme::TEXT_MID,
            false,
        );
        g.fill(
            (self.px + 10) as f32,
            (self.py + 25) as f32,
            (self.px + self.pw - 10) as f32,
            (self.py + 26) as f32,
            PlayerTheme::argb(0xFF_FFFF, 16),
        );

        let top = self.py + 34;
        let area_h = self.ph - 72;
        if let Some(steps) = PAGES[self.page] {
            let gap = 7;
            let card_h = (area_h - gap * (steps.len() as i32 - 1)) / steps.len() as i32;
            let cx = self.px + 14;
            let cw = self.pw - 28;
            for (i, step) in steps.iter().enumerate() {
                let cy = top + i as i32 * (card_h + gap);
                shapes::fill(g, cx, cy, cw, card_h, 5, PlayerTheme::argb(0x10_1015, 210));
                shapes::ring(g, cx, cy, cw, card_h, 5, PlayerTheme::argb(0xFF_FFFF, 18));

                PlayerTheme::circle(g, cx + 14, cy + card_h / 2, 7, th.accent);
                let num = format!("{}", i + 1);
                let nw = metrics.width(&num);
                g.text(
                    num,
                    (cx + 14) as f32 - nw / 2.0,
                    (cy + card_h / 2) as f32 - 4.0,
                    PlayerTheme::TEXT_MAIN,
                    false,
                );

                let tx = cx + 28;
                let has_ill = step.ill != Ill::None;
                let tw = cw - 36 - if has_ill { ILL_W + 10 } else { 0 };
                let title = i18n::get(&format!("help.{}.t", step.id));
                let body = resolve_step_body(step.id, &hk);
                let wrapped = metrics.wrap(&body, tw.max(60) as f32);
                let lh = metrics.line_height() + 1.0;
                let total_th = lh + 2.0 + wrapped.len() as f32 * lh;
                let ty0 = cy as f32 + (card_h as f32 - total_th) / 2.0;
                g.text(
                    metrics.trim_to_width(&title, tw.max(60) as f32),
                    tx as f32,
                    ty0,
                    PlayerTheme::TEXT_MAIN,
                    false,
                );
                for (li, line) in wrapped.iter().enumerate() {
                    g.text(
                        line,
                        tx as f32,
                        ty0 + lh + 2.0 + li as f32 * lh,
                        PlayerTheme::TEXT_MID,
                        false,
                    );
                }

                if has_ill {
                    let ix = cx + cw - ILL_W - 8;
                    let iy = cy + 5;
                    let ih = (card_h - 10).max(20);
                    shapes::fill(g, ix, iy, ILL_W, ih, 4, PlayerTheme::argb(0x09_090D, 220));
                    draw_illustration(g, metrics, step.ill, ix, iy, ILL_W, ih, &th, &hk);
                }
            }
        } else {
            // Hotkeys overview table
            let cx = self.px + 14;
            let cw = self.pw - 28;
            let row_h = (area_h / HotkeyAction::ALL.len() as i32).max(16);
            for (i, act) in HotkeyAction::ALL.iter().enumerate() {
                let ry = top + i as i32 * row_h;
                if i % 2 == 0 {
                    shapes::fill(
                        g,
                        cx,
                        ry,
                        cw,
                        row_h - 1,
                        3,
                        PlayerTheme::argb(0x12_1218, 170),
                    );
                }
                g.text(
                    i18n::get(act.label_key()),
                    (cx + 8) as f32,
                    ry as f32 + (row_h as f32 - metrics.line_height()) / 2.0,
                    PlayerTheme::TEXT_MAIN,
                    false,
                );
                let kn = HotkeyConfig::display_name(hk.key_of(*act));
                let kw = (metrics.width(&kn) + 12.0).round() as i32;
                let kx = cx + cw - kw - 8;
                shapes::fill(g, kx, ry + 2, kw, row_h - 5, 3, th.track_bg);
                g.text(
                    kn,
                    (kx + 6) as f32,
                    ry as f32 + (row_h as f32 - metrics.line_height()) / 2.0,
                    th.accent,
                    false,
                );
            }
        }

        // Pagination dots
        let dots_y = self.py + self.ph - 22;
        let dx0 = self.px + (self.pw - (PAGES.len() as i32 * 14)) / 2;
        for i in 0..PAGES.len() {
            let col = if i == self.page {
                th.accent
            } else {
                th.track_bg
            };
            PlayerTheme::circle(
                g,
                dx0 + i as i32 * 14 + 7,
                dots_y + 3,
                if i == self.page { 3 } else { 2 },
                col,
            );
        }

        if self.page > 0 {
            draw_nav_btn(
                g,
                metrics,
                self.back_rect,
                &format!("‹ {}", i18n::get("btn.back")),
                false,
                &th,
                mx,
                my,
            );
        }
        let next_lbl = if self.page + 1 >= PAGES.len() {
            i18n::get("btn.done")
        } else {
            format!("{} ›", i18n::get("help.next"))
        };
        draw_nav_btn(g, metrics, self.next_rect, &next_lbl, true, &th, mx, my);
    }
}

fn in_rect(r: [i32; 4], mx: i32, my: i32) -> bool {
    mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3]
}

fn resolve_step_body(id: &str, hk: &HotkeyConfig) -> String {
    let k_open = HotkeyConfig::display_name(hk.key_of(HotkeyAction::Open));
    let k_prev = HotkeyConfig::display_name(hk.key_of(HotkeyAction::Prev));
    let k_pause = HotkeyConfig::display_name(hk.key_of(HotkeyAction::Pause));
    let k_next = HotkeyConfig::display_name(hk.key_of(HotkeyAction::Next));
    let k_hud = HotkeyConfig::display_name(hk.key_of(HotkeyAction::HudSettings));
    match id {
        "quick.1" => i18n::fmt(&format!("help.{id}.d"), &[&k_open]),
        "quick.4" => i18n::fmt(
            &format!("help.{id}.d"),
            &[&k_prev, &k_pause, &k_next, &k_hud],
        ),
        _ => i18n::get(&format!("help.{id}.d")),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_illustration(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    ill: Ill,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    th: &PlayerTheme,
    hk: &HotkeyConfig,
) {
    match ill {
        Ill::None => {}
        Ill::KeyM => {
            let kn = HotkeyConfig::display_name(hk.key_of(HotkeyAction::Open));
            let kw = (metrics.width(&kn) + 18.0).max(26.0) as i32;
            let kx = x + (w - kw) / 2;
            let ky = y + (h - 22) / 2;
            shapes::fill(g, kx, ky, kw, 22, 4, th.panel_top);
            shapes::ring(g, kx, ky, kw, 22, 4, th.accent);
            g.text(
                &kn,
                kx as f32 + (kw as f32 - metrics.width(&kn)) / 2.0,
                (ky + 6) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );
        }
        Ill::Cards => {
            for (i, rgb) in [0xFF_CC00, 0xFF_5500, 0x34_D399, 0x1D_B954]
                .iter()
                .enumerate()
            {
                let bx = x + 10 + i as i32 * 25;
                let by = y + (h - 16) / 2;
                shapes::fill(g, bx, by, 20, 16, 3, 0xFF00_0000 | *rgb);
            }
        }
        Ill::Search => {
            shapes::fill(g, x + 8, y + (h - 16) / 2, w - 38, 16, 3, th.field_bg);
            shapes::fill(g, x + w - 26, y + (h - 16) / 2, 18, 16, 3, th.accent);
            g.text(
                "♪",
                (x + 13) as f32,
                (y + (h - 16) / 2 + 4) as f32,
                PlayerTheme::TEXT_DIM,
                false,
            );
        }
        Ill::Keys => {
            for (i, act) in [HotkeyAction::Prev, HotkeyAction::Pause, HotkeyAction::Next]
                .iter()
                .enumerate()
            {
                let k = HotkeyConfig::display_name(hk.key_of(*act));
                let kx = x + 12 + i as i32 * 33;
                let ky = y + (h - 18) / 2;
                shapes::fill(g, kx, ky, 26, 18, 3, th.track_bg);
                g.text(
                    &k,
                    kx as f32 + (26.0 - metrics.width(&k)) / 2.0,
                    (ky + 5) as f32,
                    th.accent,
                    false,
                );
            }
        }
        Ill::LoginBtn => {
            shapes::fill(g, x + 16, y + (h - 18) / 2, w - 32, 18, 4, th.accent);
            let lbl = "Auth";
            g.text(
                lbl,
                x as f32 + (w as f32 - metrics.width(lbl)) / 2.0,
                (y + (h - 18) / 2 + 5) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );
        }
        Ill::Address => {
            shapes::fill(g, x + 8, y + (h - 14) / 2, w - 16, 14, 3, th.field_bg);
            g.text(
                "oauth…",
                (x + 12) as f32,
                (y + (h - 14) / 2 + 3) as f32,
                th.accent,
                false,
            );
        }
        Ill::Paste => {
            shapes::fill(g, x + 10, y + 8, w - 20, 14, 3, th.field_bg);
            shapes::fill(g, x + 22, y + 26, w - 44, 12, 3, th.accent);
        }
        Ill::Drop => {
            shapes::ring(g, x + 14, y + 6, w - 28, h - 12, 4, th.accent);
            g.text(
                ".mp3 / .lrc",
                (x + 24) as f32,
                (y + h / 2 - 4) as f32,
                PlayerTheme::TEXT_MID,
                false,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_nav_btn(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    r: [i32; 4],
    label: &str,
    primary: bool,
    th: &PlayerTheme,
    mx: i32,
    my: i32,
) {
    let hov = in_rect(r, mx, my);
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
