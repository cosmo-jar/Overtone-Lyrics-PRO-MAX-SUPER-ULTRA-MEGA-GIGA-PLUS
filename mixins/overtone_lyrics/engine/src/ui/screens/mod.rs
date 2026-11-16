pub mod config;
pub mod edit;
pub mod help;
pub mod picker;
pub mod player;

use crate::cinema::CinematicController;
use crate::controller::PlayerController;
use crate::ui::cover::{CoverArtManager, ModImageAtlas};
use crate::ui::hud::HudState;
use crate::ui::world_lyrics::WorldLyricsState;
use crate::ui::{DrawList, TextMetrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKey {
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Space,
    Tab,
    Char(char),
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenSideEffect {
    OpenUrl(String),
    CopyClipboard(String),
    CloseHostScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerTarget {
    PlayerAccent,
    HudAccent,
    WorldAccent2,
}

#[derive(Clone, Debug)]
pub enum ScreenRoute {
    Stay,
    Pop,
    CloseAll,
    OpenConfig(config::ConfigTab),
    OpenEdit,
    OpenHelp,
    OpenPicker {
        initial_rgb: u32,
        target: PickerTarget,
    },
    SideEffect(ScreenSideEffect),
}

/// Pure-Rust single-line editable input box backing Search, Tokens, Client ID, Hex, Musixmatch token.
#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub value: String,
    pub hint: String,
    pub cursor: usize,
    pub focused: bool,
    pub max_len: usize,
    pub scroll_x: usize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl TextInput {
    pub fn new(hint: impl Into<String>, max_len: usize) -> Self {
        Self {
            value: String::new(),
            hint: hint.into(),
            cursor: 0,
            focused: false,
            max_len: max_len.max(16),
            scroll_x: 0,
            x: 0,
            y: 0,
            w: 120,
            h: 20,
        }
    }

    pub fn set_bounds(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
    }

    pub fn hit(&self, mx: i32, my: i32) -> bool {
        self.w > 0 && mx >= self.x && mx < self.x + self.w && my >= self.y && my < self.y + self.h
    }

    pub fn set_value(&mut self, v: impl Into<String>) {
        self.value = v.into();
        if self.value.chars().count() > self.max_len {
            self.value = self.value.chars().take(self.max_len).collect();
        }
        self.cursor = self.value.len();
    }

    pub fn insert_char(&mut self, ch: char) -> bool {
        if !self.focused || ch.is_control() {
            return false;
        }
        if self.value.chars().count() >= self.max_len {
            return false;
        }
        let pos = self.cursor.min(self.value.len());
        self.value.insert(pos, ch);
        self.cursor = pos + ch.len_utf8();
        true
    }

    pub fn insert_str(&mut self, s: &str) -> bool {
        if !self.focused {
            return false;
        }
        let mut changed = false;
        for ch in s.chars().filter(|c| !c.is_control()) {
            if !self.insert_char(ch) {
                break;
            }
            changed = true;
        }
        changed
    }

    pub fn handle_key(&mut self, key: ScreenKey, ctrl: bool) -> bool {
        if !self.focused {
            return false;
        }
        match key {
            ScreenKey::Backspace => {
                if self.cursor > 0 && !self.value.is_empty() {
                    let prev = self.value[..self.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.value.remove(prev);
                    self.cursor = prev;
                    return true;
                }
                true
            }
            ScreenKey::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
                true
            }
            ScreenKey::Left => {
                if self.cursor > 0 {
                    self.cursor = self.value[..self.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                true
            }
            ScreenKey::Right => {
                if self.cursor < self.value.len()
                    && let Some(ch) = self.value[self.cursor..].chars().next()
                {
                    self.cursor += ch.len_utf8();
                }
                true
            }
            ScreenKey::Home => {
                self.cursor = 0;
                true
            }
            ScreenKey::End => {
                self.cursor = self.value.len();
                true
            }
            ScreenKey::Char('a') | ScreenKey::Char('A') if ctrl => {
                self.cursor = self.value.len();
                true
            }
            ScreenKey::Char('u') | ScreenKey::Char('U') if ctrl => {
                self.value.clear();
                self.cursor = 0;
                true
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        radius: i32,
        focus_col: u32,
        track_bg: u32,
        field_bg: u32,
        text_main: u32,
        text_dim: u32,
    ) {
        let br = radius.min(4);
        crate::ui::shapes::fill(
            g,
            self.x - 1,
            self.y - 1,
            self.w + 2,
            self.h + 2,
            br + 1,
            if self.focused { focus_col } else { track_bg },
        );
        crate::ui::shapes::fill(g, self.x, self.y, self.w, self.h, br, field_bg);
        let pad = 6;
        let inner_w = (self.w - pad * 2).max(12) as f32;
        let ty = self.y as f32 + (self.h as f32 - metrics.line_height()) / 2.0;
        g.push_scissor((self.x + pad) as f32, self.y as f32, inner_w, self.h as f32);
        if self.value.is_empty() && !self.hint.is_empty() {
            let shown = metrics.ellipsize(&self.hint, inner_w);
            g.text(shown, (self.x + pad) as f32, ty, text_dim, false);
        } else {
            let before = &self.value[..self.cursor.min(self.value.len())];
            let cursor_px = metrics.width(before);
            let mut offset = 0.0f32;
            if cursor_px > inner_w - 4.0 {
                offset = cursor_px - (inner_w - 4.0);
            }
            g.text(
                &self.value,
                (self.x + pad) as f32 - offset,
                ty,
                text_main,
                false,
            );
            if self.focused {
                let cx = (self.x + pad) as f32 + cursor_px - offset;
                g.fill(
                    cx,
                    ty - 1.0,
                    cx + 1.0,
                    ty + metrics.line_height() + 1.0,
                    text_main,
                );
            }
        }
        g.pop_scissor();
    }
}

pub enum ScreenPage {
    Player(Box<player::PlayerScreenState>),
    Config(config::HudConfigScreenState),
    Edit(edit::HudEditScreenState),
    Picker(picker::ColorPickerState),
    Help(help::HelpScreenState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseTarget {
    Pop,
    CloseAll,
}

pub struct ScreenStack {
    stack: Vec<ScreenPage>,
    pub side_effects: Vec<ScreenSideEffect>,
    pub preview_hud: HudState,
    anim_open_at: std::time::Instant,
    anim_closing: Option<(std::time::Instant, CloseTarget)>,
}

impl Default for ScreenStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenStack {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            side_effects: Vec::new(),
            preview_hud: HudState::default(),
            anim_open_at: std::time::Instant::now(),
            anim_closing: None,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    pub fn is_text_input_focused(&self) -> bool {
        match self.stack.last() {
            Some(ScreenPage::Player(s)) => s.is_text_focused(),
            Some(ScreenPage::Config(s)) => s.is_text_focused(),
            Some(ScreenPage::Picker(s)) => s.is_text_focused(),
            _ => false,
        }
    }

    pub fn is_listening_hotkey(&self) -> bool {
        matches!(self.stack.last(), Some(ScreenPage::Config(s)) if s.is_listening_hotkey())
    }

    fn should_animate(top: &ScreenPage, anim: crate::config::WindowAnim) -> bool {
        anim != crate::config::WindowAnim::Off && !matches!(top, ScreenPage::Edit(_))
    }

    fn reset_open_anim(&mut self) {
        self.anim_closing = None;
        self.anim_open_at = std::time::Instant::now();
    }

    pub fn open_player(&mut self, ctl: &PlayerController, sw: i32, sh: i32) {
        self.stack.clear();
        let mut scr = player::PlayerScreenState::new();
        scr.init(ctl, sw, sh);
        self.stack.push(ScreenPage::Player(Box::new(scr)));
        self.reset_open_anim();
    }

    pub fn open_config(
        &mut self,
        tab: config::ConfigTab,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) {
        let mut scr = config::HudConfigScreenState::new(tab);
        scr.init(ctl, sw, sh);
        self.stack.push(ScreenPage::Config(scr));
        self.reset_open_anim();
    }

    pub fn open_edit_direct(&mut self, _ctl: &PlayerController, sw: i32, sh: i32) {
        let mut e = edit::HudEditScreenState::new();
        e.init(sw, sh);
        self.stack.push(ScreenPage::Edit(e));
        self.anim_closing = None;
    }

    pub fn open_help_direct(&mut self, sw: i32, sh: i32) {
        let mut h = help::HelpScreenState::new();
        h.init(sw, sh);
        self.stack.push(ScreenPage::Help(h));
        self.reset_open_anim();
    }

    pub fn close_all(&mut self) {
        self.anim_closing = None;
        self.stack.clear();
        self.side_effects.push(ScreenSideEffect::CloseHostScreen);
    }

    pub fn close_all_animated(&mut self, ctl: &PlayerController) {
        let anim = ctl.player_config().window_anim;
        let can_anim = self
            .stack
            .last()
            .is_some_and(|top| Self::should_animate(top, anim));
        if self.anim_closing.is_some() || !can_anim {
            self.close_all();
        } else {
            self.anim_closing = Some((std::time::Instant::now(), CloseTarget::CloseAll));
        }
    }

    pub fn pop(&mut self) {
        self.anim_closing = None;
        self.stack.pop();
        if self.stack.is_empty() {
            self.side_effects.push(ScreenSideEffect::CloseHostScreen);
        }
    }

    fn finish_close(&mut self, target: CloseTarget, ctl: &PlayerController, sw: i32, sh: i32) {
        self.anim_closing = None;
        match target {
            CloseTarget::CloseAll => self.close_all(),
            CloseTarget::Pop => {
                self.stack.pop();
                if self.stack.is_empty() {
                    self.side_effects.push(ScreenSideEffect::CloseHostScreen);
                } else {
                    self.anim_open_at = std::time::Instant::now();
                    if let Some(ScreenPage::Player(p)) = self.stack.last_mut() {
                        p.init(ctl, sw, sh);
                    } else if let Some(ScreenPage::Config(c)) = self.stack.last_mut() {
                        c.init(ctl, sw, sh);
                    }
                }
            }
        }
    }

    pub fn poll_side_effects(&mut self) -> Vec<ScreenSideEffect> {
        std::mem::take(&mut self.side_effects)
    }

    pub fn tick(
        &mut self,
        ctl: &PlayerController,
        cinema: &mut CinematicController,
        sw: i32,
        sh: i32,
    ) {
        if let Some((started, target)) = self.anim_closing
            && started.elapsed().as_secs_f32() >= 0.220
        {
            self.finish_close(target, ctl, sw, sh);
            return;
        }
        let mut drained = Vec::new();
        let open_help = if let Some(top) = self.stack.last_mut() {
            match top {
                ScreenPage::Player(s) => {
                    let h = s.tick(ctl, sw, sh);
                    drained.extend(s.poll_side_effects());
                    h
                }
                ScreenPage::Config(_)
                | ScreenPage::Edit(_)
                | ScreenPage::Picker(_)
                | ScreenPage::Help(_) => {
                    let _ = cinema;
                    false
                }
            }
        } else {
            false
        };
        self.side_effects.extend(drained);
        if open_help {
            let mut h = help::HelpScreenState::new();
            h.init(sw, sh);
            self.stack.push(ScreenPage::Help(h));
            self.reset_open_anim();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        atlas: &mut ModImageAtlas,
        world_lyrics: &mut WorldLyricsState,
        cinema: &mut CinematicController,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
        dt: f32,
    ) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let _ = world_lyrics;
        let anim = ctl.player_config().window_anim;
        if !Self::should_animate(top, anim) {
            Self::render_page(
                top,
                &mut self.preview_hud,
                g,
                metrics,
                ctl,
                covers,
                atlas,
                cinema,
                sw,
                sh,
                mx,
                my,
                dt,
            );
            return;
        }

        let (rmx, rmy) = if self.anim_closing.is_some() {
            (-9999, -9999)
        } else {
            (mx, my)
        };
        let mut sub = DrawList::new();
        Self::render_page(
            top,
            &mut self.preview_hud,
            &mut sub,
            metrics,
            ctl,
            covers,
            atlas,
            cinema,
            sw,
            sh,
            rmx,
            rmy,
            dt,
        );
        let (dim_alpha, tx, ty, sx, sy, panel_alpha) =
            compute_window_anim(anim, self.anim_open_at, self.anim_closing.map(|(s, _)| s));

        if let Some(&crate::ui::DrawCmd::FillRect { x, y, w, h, argb }) = sub.cmds.first()
            && x <= 1.0
            && y <= 1.0
            && w >= sw as f32 - 2.0
            && h >= sh as f32 - 2.0
        {
            g.cmds.push(crate::ui::DrawCmd::FillRect {
                x,
                y,
                w,
                h,
                argb: crate::ui::scale_argb_alpha(argb, dim_alpha),
            });
            g.append_transformed_cmds(
                &sub.cmds[1..],
                sw as f32 * 0.5,
                sh as f32 * 0.5,
                tx,
                ty,
                sx,
                sy,
                panel_alpha,
            );
        } else {
            g.append_transformed_cmds(
                &sub.cmds,
                sw as f32 * 0.5,
                sh as f32 * 0.5,
                tx,
                ty,
                sx,
                sy,
                panel_alpha,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_page(
        page: &mut ScreenPage,
        preview_hud: &mut HudState,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        atlas: &mut ModImageAtlas,
        cinema: &mut CinematicController,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
        dt: f32,
    ) {
        match page {
            ScreenPage::Player(s) => s.render(g, metrics, ctl, covers, sw, sh, mx, my, dt),
            ScreenPage::Config(s) => s.render(g, metrics, ctl, sw, sh, mx, my, dt, cinema),
            ScreenPage::Edit(s) => {
                s.render(g, metrics, ctl, covers, preview_hud, sw, sh, mx, my, dt)
            }
            ScreenPage::Picker(s) => s.render(g, metrics, ctl, atlas, sw, sh, mx, my),
            ScreenPage::Help(s) => s.render(g, metrics, ctl, sw, sh, mx, my),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mouse_down(
        &mut self,
        mx: i32,
        my: i32,
        button: i32,
        ctl: &PlayerController,
        world_lyrics: &mut WorldLyricsState,
        cinema: &mut CinematicController,
        metrics: &dyn TextMetrics,
        sw: i32,
        sh: i32,
    ) {
        if self.anim_closing.is_some() {
            return;
        }
        let mut drained = Vec::new();
        let mut cfg_action = None;
        let route = {
            let Some(top) = self.stack.last_mut() else {
                return;
            };
            match top {
                ScreenPage::Player(s) => {
                    let r = s.mouse_down(mx, my, button, ctl, sw, sh);
                    drained.extend(s.poll_side_effects());
                    r
                }
                ScreenPage::Config(s) => {
                    cfg_action = s.is_world_demo_row(mx, my, ctl);
                    s.mouse_down(mx, my, button, ctl, sw, sh)
                }
                ScreenPage::Edit(s) => s.mouse_down(mx, my, button, &self.preview_hud, ctl, sw, sh),
                ScreenPage::Picker(s) => s.mouse_down(mx, my, button, ctl, sw, sh),
                ScreenPage::Help(s) => s.mouse_down(mx, my, button),
            }
        };
        self.side_effects.extend(drained);
        if let Some(act) = cfg_action {
            self.dispatch_config_action(act, ctl, world_lyrics, cinema, metrics);
        }
        self.apply_route(route, ctl, sw, sh);
    }

    pub fn mouse_drag(&mut self, mx: i32, my: i32, ctl: &PlayerController, sw: i32, sh: i32) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        match top {
            ScreenPage::Player(s) => s.mouse_drag(mx, my, ctl),
            ScreenPage::Edit(s) => s.mouse_drag(mx, my, &self.preview_hud, ctl, sw, sh),
            ScreenPage::Picker(s) => s.mouse_drag(mx, my),
            ScreenPage::Config(s) => s.mouse_drag(mx, my, ctl),
            ScreenPage::Help(_) => {}
        }
    }

    pub fn mouse_up(&mut self, mx: i32, my: i32, button: i32, ctl: &PlayerController) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        match top {
            ScreenPage::Player(s) => s.mouse_up(mx, my, button, ctl),
            ScreenPage::Edit(s) => s.mouse_up(),
            ScreenPage::Picker(s) => s.mouse_up(),
            ScreenPage::Config(s) => s.mouse_up(),
            ScreenPage::Help(_) => {}
        }
    }

    pub fn mouse_scroll(
        &mut self,
        mx: i32,
        my: i32,
        sy: f32,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        match top {
            ScreenPage::Player(s) => s.mouse_scroll(mx, my, sy, ctl),
            ScreenPage::Config(s) => s.mouse_scroll(mx, my, sy, sw, sh),
            ScreenPage::Edit(s) => s.mouse_scroll(mx, my, sy, &self.preview_hud, ctl),
            ScreenPage::Picker(_) | ScreenPage::Help(_) => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn key_pressed(
        &mut self,
        key: ScreenKey,
        raw_key: &str,
        ctrl: bool,
        shift: bool,
        ctl: &PlayerController,
        world_lyrics: &mut WorldLyricsState,
        cinema: &mut CinematicController,
        metrics: &dyn TextMetrics,
        sw: i32,
        sh: i32,
    ) -> bool {
        let Some(top) = self.stack.last_mut() else {
            return false;
        };
        let (handled, route) = match top {
            ScreenPage::Player(s) => s.key_pressed(key, raw_key, ctrl, ctl, sw, sh),
            ScreenPage::Config(s) => s.key_pressed(
                key,
                raw_key,
                ctrl,
                ctl,
                world_lyrics,
                cinema,
                metrics,
                sw,
                sh,
            ),
            ScreenPage::Edit(s) => s.key_pressed(key, shift, ctl),
            ScreenPage::Picker(s) => s.key_pressed(key, ctrl, ctl),
            ScreenPage::Help(s) => s.key_pressed(key),
        };
        self.apply_route(route, ctl, sw, sh);
        handled
    }

    pub fn char_typed(&mut self, ch: char, ctl: &PlayerController) -> bool {
        let Some(top) = self.stack.last_mut() else {
            return false;
        };
        match top {
            ScreenPage::Player(s) => s.char_typed(ch),
            ScreenPage::Config(s) => s.char_typed(ch, ctl),
            ScreenPage::Picker(s) => s.char_typed(ch),
            ScreenPage::Edit(_) | ScreenPage::Help(_) => false,
        }
    }

    pub fn paste_text(&mut self, txt: &str, ctl: &PlayerController) -> bool {
        let Some(top) = self.stack.last_mut() else {
            return false;
        };
        match top {
            ScreenPage::Player(s) => s.paste_text(txt),
            ScreenPage::Config(s) => s.paste_text(txt, ctl),
            ScreenPage::Picker(s) => s.paste_text(txt),
            ScreenPage::Edit(_) | ScreenPage::Help(_) => false,
        }
    }

    pub fn on_files_drop(&mut self, paths: &[std::path::PathBuf], ctl: &PlayerController) -> bool {
        if let Some(ScreenPage::Player(s)) = self.stack.last_mut() {
            return s.on_files_drop(paths, ctl);
        }
        false
    }

    pub fn dispatch_config_action(
        &mut self,
        action: config::ConfigAction,
        ctl: &PlayerController,
        world_lyrics: &mut WorldLyricsState,
        cinema: &mut CinematicController,
        metrics: &dyn TextMetrics,
    ) {
        match action {
            config::ConfigAction::SpawnWorldDemo => {
                let cfg = ctl.hud_config();
                world_lyrics.spawn_demo(&cfg, metrics);
            }
            config::ConfigAction::ToggleCinemaPreview => {
                cinema.toggle(0.0, &ctl.hud_config());
            }
        }
    }

    fn apply_route(&mut self, route: ScreenRoute, ctl: &PlayerController, sw: i32, sh: i32) {
        match route {
            ScreenRoute::Stay => {}
            ScreenRoute::Pop => {
                let anim = ctl.player_config().window_anim;
                let can_anim = self
                    .stack
                    .last()
                    .is_some_and(|top| Self::should_animate(top, anim));
                if self.anim_closing.is_some() || !can_anim {
                    self.finish_close(CloseTarget::Pop, ctl, sw, sh);
                } else {
                    self.anim_closing = Some((std::time::Instant::now(), CloseTarget::Pop));
                }
            }
            ScreenRoute::CloseAll => self.close_all_animated(ctl),
            ScreenRoute::OpenConfig(tab) => self.open_config(tab, ctl, sw, sh),
            ScreenRoute::OpenEdit => {
                let mut e = edit::HudEditScreenState::new();
                e.init(sw, sh);
                self.stack.push(ScreenPage::Edit(e));
                self.anim_closing = None;
            }
            ScreenRoute::OpenHelp => {
                let mut h = help::HelpScreenState::new();
                h.init(sw, sh);
                self.stack.push(ScreenPage::Help(h));
                self.reset_open_anim();
            }
            ScreenRoute::OpenPicker {
                initial_rgb,
                target,
            } => {
                let mut pk = picker::ColorPickerState::new(initial_rgb, target);
                pk.init(sw, sh);
                self.stack.push(ScreenPage::Picker(pk));
                self.reset_open_anim();
            }
            ScreenRoute::SideEffect(se) => self.side_effects.push(se),
        }
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

fn compute_window_anim(
    anim: crate::config::WindowAnim,
    open_at: std::time::Instant,
    closing_at: Option<std::time::Instant>,
) -> (f32, f32, f32, f32, f32, f32) {
    use crate::config::WindowAnim;
    if let Some(closed) = closing_at {
        let raw = (closed.elapsed().as_secs_f32() / 0.220).clamp(0.0, 1.0);
        let e = ease_in_cubic(raw);
        let fade = (1.0 - e).clamp(0.0, 1.0);
        return match anim {
            WindowAnim::Off => (1.0, 0.0, 0.0, 1.0, 1.0, 1.0),
            WindowAnim::Zoom => {
                let s = 0.90 + 0.10 * fade;
                (fade, 0.0, 0.0, s, s, fade)
            }
            WindowAnim::Slide => (fade, 0.0, 26.0 * e, 1.0, 1.0, fade),
            WindowAnim::Pop => {
                let s = (1.0 - 0.16 * e).clamp(0.01, 1.2);
                (fade, 0.0, 0.0, s, s, fade)
            }
            WindowAnim::Unfold => {
                let t = 1.0 - raw;
                let (sx, sy) = if t > 0.5 {
                    let u = (t - 0.5) * 2.0;
                    (1.0, 0.03 + 0.97 * ease_out_cubic(u))
                } else {
                    let u = t * 2.0;
                    (0.05 + 0.95 * ease_out_cubic(u), 0.03)
                };
                (t, 0.0, 0.0, sx, sy, (t * 2.0).clamp(0.0, 1.0))
            }
        };
    }

    let dur = if anim == WindowAnim::Pop {
        0.360
    } else {
        0.340
    };
    let raw = (open_at.elapsed().as_secs_f32() / dur).clamp(0.0, 1.0);
    if raw >= 1.0 {
        return (1.0, 0.0, 0.0, 1.0, 1.0, 1.0);
    }
    let e = ease_out_cubic(raw);
    match anim {
        WindowAnim::Off => (1.0, 0.0, 0.0, 1.0, 1.0, 1.0),
        WindowAnim::Zoom => {
            let s = 0.90 + 0.10 * e;
            (e, 0.0, 0.0, s, s, e)
        }
        WindowAnim::Slide => (e, 0.0, 26.0 * (1.0 - e), 1.0, 1.0, e),
        WindowAnim::Pop => {
            let dim = ease_out_cubic((raw * 1.4).clamp(0.0, 1.0));
            let decay = (-6.5 * raw).exp();
            let s = (1.0 + decay * (0.14 * (raw * 14.0).sin() - 0.18 * (raw * 14.0).cos()))
                .clamp(0.65, 1.15);
            (dim, 0.0, 0.0, s, s, dim)
        }
        WindowAnim::Unfold => {
            let (sx, sy) = if raw < 0.45 {
                let u = raw / 0.45;
                (0.05 + 0.95 * ease_out_cubic(u), 0.03)
            } else {
                let u = (raw - 0.45) / 0.55;
                (1.0, 0.03 + 0.97 * ease_out_cubic(u))
            };
            let dim = ease_out_cubic((raw * 1.8).clamp(0.0, 1.0));
            (dim, 0.0, 0.0, sx, sy, (raw * 2.5).clamp(0.0, 1.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MonoMetrics;

    #[test]
    fn screen_stack_opens_all_five_screens_and_dispatches_actions() {
        let ctl = PlayerController::new(false);
        let mut stack = ScreenStack::new();
        let mut covers = CoverArtManager::new();
        let _ = &mut covers;
        let mut atlas = ModImageAtlas::new();
        let mut world = WorldLyricsState::new();
        let mut cinema = CinematicController::new();
        let metrics = MonoMetrics;

        stack.open_player(&ctl, 640, 360);
        assert!(stack.is_open());
        let mut dl = DrawList::new();
        stack.render(
            &mut dl,
            &metrics,
            &ctl,
            &covers,
            &mut atlas,
            &mut world,
            &mut cinema,
            640,
            360,
            0,
            0,
            0.016,
        );
        assert!(!dl.cmds.is_empty());

        stack.open_config(config::ConfigTab::World, &ctl, 640, 360);
        stack.dispatch_config_action(
            config::ConfigAction::SpawnWorldDemo,
            &ctl,
            &mut world,
            &mut cinema,
            &metrics,
        );
        assert!(world.active_group_count() >= 1);

        stack.dispatch_config_action(
            config::ConfigAction::ToggleCinemaPreview,
            &ctl,
            &mut world,
            &mut cinema,
            &metrics,
        );
        assert!(cinema.active());

        stack.apply_route(ScreenRoute::OpenEdit, &ctl, 640, 360);
        let mut dl2 = DrawList::new();
        stack.render(
            &mut dl2,
            &metrics,
            &ctl,
            &covers,
            &mut atlas,
            &mut world,
            &mut cinema,
            640,
            360,
            100,
            100,
            0.016,
        );
        assert!(!dl2.cmds.is_empty());

        stack.apply_route(
            ScreenRoute::OpenPicker {
                initial_rgb: 0x33_AAFF,
                target: PickerTarget::HudAccent,
            },
            &ctl,
            640,
            360,
        );
        let mut dl3 = DrawList::new();
        stack.render(
            &mut dl3,
            &metrics,
            &ctl,
            &covers,
            &mut atlas,
            &mut world,
            &mut cinema,
            640,
            360,
            100,
            100,
            0.016,
        );
        assert!(!dl3.cmds.is_empty());

        stack.apply_route(ScreenRoute::OpenHelp, &ctl, 640, 360);
        let mut dl4 = DrawList::new();
        stack.render(
            &mut dl4,
            &metrics,
            &ctl,
            &covers,
            &mut atlas,
            &mut world,
            &mut cinema,
            640,
            360,
            100,
            100,
            0.016,
        );
        assert!(!dl4.cmds.is_empty());

        stack.close_all();
        assert!(!stack.is_open());
        assert!(
            stack
                .poll_side_effects()
                .contains(&ScreenSideEffect::CloseHostScreen)
        );
    }
}
