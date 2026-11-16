use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::PlayerConfig;
use crate::controller::{PlayerController, RepeatMode, Tab};
use crate::i18n;
use crate::lyrics::cache as lyrics_cache;
use crate::model::{EntityKind, LyricLine, MusicEntity, Platform, Track};
use crate::providers::spotify::{REDIRECT_URI, SpotifyAuthState};
use crate::providers::yandex::DevicePollResult;
use crate::ui::cover::CoverArtManager;
use crate::ui::screens::config::ConfigTab;
use crate::ui::screens::{ScreenKey, ScreenRoute, ScreenSideEffect, TextInput};
use crate::ui::{DrawList, PlayerTheme, TexSlot, TextMetrics, draw_platform_icon, shapes};

const MAX_ACCOUNT_ROWS: usize = 4;
const ROW_H: i32 = 28;
const ACT_W: i32 = 16;
const VOL_BAR_W: i32 = 66;
const WAVE_PALETTE: [u32; 5] = [0xFF8A3D, 0xFF4D8D, 0xA05CFF, 0x3D8BFF, 0x2FD6C3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Picker,
    Login,
    Main,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Track(usize),
    Entity(usize),
}

#[derive(Clone)]
struct PendingDeviceAuth {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_at: Instant,
    next_poll: Instant,
    poll_interval_sec: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn hit(self, mx: i32, my: i32) -> bool {
        self.w > 0
            && self.h > 0
            && mx >= self.x
            && mx < self.x + self.w
            && my >= self.y
            && my < self.y + self.h
    }
}

pub struct PlayerScreenState {
    pub mode: Mode,
    built_at: Instant,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    lx: i32,
    lw: i32,
    rx: i32,
    rw: i32,
    list_top: i32,
    rows_per_page: usize,
    page: usize,
    by: i32,
    cy: i32,
    vol_bar_x: i32,
    cover_px: i32,
    ly_top: i32,
    ly_bottom: i32,

    queue_mode: bool,
    add_account_mode: bool,
    spotify_cid_prefilled: bool,
    dragging_seek: bool,
    seek_frac: f32,
    dragging_vol: bool,
    seek_glow: f32,
    row_glow: Vec<f32>,

    search_box: TextInput,
    token_box: TextInput,

    device_auth: Option<PendingDeviceAuth>,
    pending_side_effects: Vec<ScreenSideEffect>,

    anim_track_id: String,
    anim_start: Instant,
    wrapped_for_id: String,
    wrapped_for_count: usize,
    wrapped_for_src: String,
    wrap_w: i32,
    wrapped: Vec<Vec<String>>,
    tops: Vec<i32>,
    total_h: i32,
    scroll_px: f32,
    manual_offset_px: f32,

    chip_rect: Rect,
    artist_rect: Rect,
    album_rect: Rect,
    hover_tip: Option<(Rect, String)>,
    strip_minus_rect: Rect,
    strip_plus_rect: Rect,
    strip_reset_rect: Rect,
    strip_mode_rect: Rect,
    strip_dsp_rect: Rect,
    strip_src_rect: Rect,
    strip_sync_rect: Rect,
    strip_own_rect: Rect,
    shuffle_rect: Rect,
    repeat_rect: Rect,
}

impl Default for PlayerScreenState {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerScreenState {
    pub fn new() -> Self {
        Self {
            mode: Mode::Main,
            built_at: Instant::now(),
            px: 0,
            py: 0,
            pw: 680,
            ph: 400,
            lx: 0,
            lw: 240,
            rx: 0,
            rw: 380,
            list_top: 0,
            rows_per_page: 8,
            page: 0,
            by: 0,
            cy: 0,
            vol_bar_x: 0,
            cover_px: 88,
            ly_top: 0,
            ly_bottom: 0,
            queue_mode: false,
            add_account_mode: false,
            spotify_cid_prefilled: false,
            dragging_seek: false,
            seek_frac: 0.0,
            dragging_vol: false,
            seek_glow: 0.0,
            row_glow: Vec::new(),
            search_box: TextInput::new(i18n::get("player.search"), 120),
            token_box: TextInput::new(i18n::get("login.paste_hint"), 16_384),
            device_auth: None,
            pending_side_effects: Vec::new(),
            anim_track_id: String::new(),
            anim_start: Instant::now(),
            wrapped_for_id: String::new(),
            wrapped_for_count: 0,
            wrapped_for_src: String::new(),
            wrap_w: 0,
            wrapped: Vec::new(),
            tops: Vec::new(),
            total_h: 0,
            scroll_px: 0.0,
            manual_offset_px: 0.0,
            chip_rect: Rect::default(),
            artist_rect: Rect::default(),
            album_rect: Rect::default(),
            hover_tip: None,
            strip_minus_rect: Rect::default(),
            strip_plus_rect: Rect::default(),
            strip_reset_rect: Rect::default(),
            strip_mode_rect: Rect::default(),
            strip_dsp_rect: Rect::default(),
            strip_src_rect: Rect::default(),
            strip_sync_rect: Rect::default(),
            strip_own_rect: Rect::default(),
            shuffle_rect: Rect::default(),
            repeat_rect: Rect::default(),
        }
    }

    pub fn init(&mut self, ctl: &PlayerController, sw: i32, sh: i32) {
        let pc = ctl.player_config();
        let scale = (pc.window_scale.clamp(60, 140) as f32) / 100.0;
        self.pw = (((sw - 40) as f32).min(780.0) * scale)
            .round()
            .clamp(420.0, (sw - 12).max(420) as f32) as i32;
        self.ph = (((sh - 28) as f32).min(430.0) * scale)
            .round()
            .clamp(280.0, (sh - 12).max(280) as f32) as i32;
        self.px = (sw - self.pw) / 2;
        self.py = (sh - self.ph) / 2;
        self.lw = (self.pw * 38 / 100).clamp(180, 290);
        self.lx = self.px + 12;
        self.rx = self.lx + self.lw + 18;
        self.rw = (self.px + self.pw - 14 - self.rx).max(180);
        self.by = self.py + self.ph - 52;
        self.cy = self.py + self.ph - 19;
        self.vol_bar_x = self.lx + 26;
        self.page = ctl.list_page();
        self.queue_mode = ctl.queue_mode();

        let plat = ctl.active_platform();
        if self.mode == Mode::Main && plat.needs_login() && !ctl.has_auth(plat) {
            self.mode = Mode::Login;
        } else if self.mode == Mode::Login && ctl.has_auth(plat) {
            self.mode = Mode::Main;
        }

        self.search_box.hint = i18n::get(if plat == Platform::Local {
            "player.search_local"
        } else {
            "player.search"
        });
        let q = ctl.state.lock().unwrap().search_query.clone();
        if self.search_box.value != q && !self.search_box.focused {
            self.search_box.set_value(q);
        }

        self.token_box.hint = match plat {
            Platform::SoundCloud => i18n::get("login.sc_hint"),
            Platform::Spotify => i18n::get("login.sp_hint"),
            Platform::YouTube => i18n::get("login.yt_hint"),
            _ => i18n::get("login.paste_hint"),
        };
        if plat == Platform::Spotify
            && !self.spotify_cid_prefilled
            && !self.token_box.focused
            && self.token_box.value.is_empty()
        {
            self.spotify_cid_prefilled = true;
            let cid = ctl.spotify.client_id();
            if !cid.is_empty() {
                self.token_box.set_value(cid);
            }
        }

        let has_page = ctl.current_page().is_some();
        let tab_h = 18;
        let top_y = self.py + 30;
        self.search_box.set_bounds(self.lx, top_y, self.lw - 56, 18);
        self.list_top = top_y + 22 + tab_h + 4 + if has_page { 20 } else { 0 };
        let list_bot = self.py + self.ph - 48;
        self.rows_per_page = ((list_bot - self.list_top) / ROW_H).max(3) as usize;
        if self.row_glow.len() < self.rows_per_page {
            self.row_glow.resize(self.rows_per_page, 0.0);
        }

        self.cover_px = if !pc.show_cover {
            0
        } else {
            pc.cover_px.clamp(40, (self.by - self.py - 110).max(40))
        };
        self.ly_top = self.py + 34 + self.cover_px.max(48) + 12;
        self.ly_bottom = self.by - 28;
    }

    pub fn tick(&mut self, ctl: &PlayerController, sw: i32, sh: i32) -> bool {
        // First-open help popup
        let open_help = {
            let mut st = ctl.state.lock().unwrap();
            if !st.help_seen {
                st.help_seen = true;
                st.session_dirty = true;
                true
            } else {
                false
            }
        };

        // Poll active Yandex Device OAuth
        if let Some(mut auth) = self.device_auth.take() {
            let now = Instant::now();
            if now >= auth.expires_at {
                ctl.state.lock().unwrap().error_msg = "Device code expired".to_owned();
            } else {
                if now >= auth.next_poll {
                    match ctl.poll_yandex_device_auth(&auth.device_code) {
                        DevicePollResult::Authorized { token, label } => {
                            let _ = token;
                            ctl.state.lock().unwrap().status_msg =
                                i18n::fmt("status.logging_in", &[&label]);
                            ctl.select_platform(Platform::Yandex);
                            self.mode = Mode::Main;
                            self.add_account_mode = false;
                            self.init(ctl, sw, sh);
                            return open_help;
                        }
                        DevicePollResult::SlowDown => {
                            auth.poll_interval_sec = (auth.poll_interval_sec + 3).min(15);
                            auth.next_poll = now + Duration::from_secs(auth.poll_interval_sec);
                        }
                        DevicePollResult::Pending => {
                            auth.next_poll = now + Duration::from_secs(auth.poll_interval_sec);
                        }
                        DevicePollResult::Failed(e) => {
                            ctl.state.lock().unwrap().error_msg = e;
                            return open_help;
                        }
                    }
                }
                self.device_auth = Some(auth);
            }
        }

        // Check if Spotify PKCE login finished in background
        if self.mode == Mode::Login && ctl.active_platform() == Platform::Spotify {
            match ctl.spotify.auth_state() {
                SpotifyAuthState::Authorized(name) => {
                    ctl.state.lock().unwrap().status_msg = i18n::fmt("status.logging_in", &[&name]);
                    ctl.select_platform(Platform::Spotify);
                    self.mode = Mode::Main;
                    self.init(ctl, sw, sh);
                }
                SpotifyAuthState::Failed(err) => {
                    let mut st = ctl.state.lock().unwrap();
                    st.status_msg.clear();
                    st.error_msg = err;
                }
                SpotifyAuthState::Idle | SpotifyAuthState::WaitingForBrowser => {}
            }
        }

        open_help
    }

    pub fn poll_side_effects(&mut self) -> Vec<ScreenSideEffect> {
        std::mem::take(&mut self.pending_side_effects)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
        dt: f32,
    ) {
        self.hover_tip = None;
        self.init(ctl, sw, sh);
        let pc = ctl.player_config();
        let hud = ctl.hud_config();
        let th = PlayerTheme::current(&pc, &hud);

        g.fill(0.0, 0.0, sw as f32, sh as f32, th.dim);
        th.window(g, self.px, self.py, self.pw, self.ph);

        // Header bar: platform chip + status + Help + Settings
        let plat = ctl.active_platform();
        let chip_label = plat.title();
        let chip_w = (metrics.width(&chip_label) + 28.0).round() as i32;
        self.chip_rect = Rect::new(self.px + 12, self.py + 6, chip_w, 18);
        let chip_bg = if self.chip_rect.hit(mx, my) {
            th.hover
        } else {
            th.track_bg
        };
        th.rrect(
            g,
            self.chip_rect.x,
            self.chip_rect.y,
            self.chip_rect.w,
            self.chip_rect.h,
            4,
            chip_bg,
        );
        draw_platform_icon(g, plat, self.chip_rect.x + 4, self.chip_rect.y + 3, 12);
        g.text(
            &chip_label,
            (self.chip_rect.x + 20) as f32,
            (self.chip_rect.y + 5) as f32,
            PlayerTheme::TEXT_MAIN,
            false,
        );
        self.track_tip(self.chip_rect, mx, my, "tip.platform");

        // Help (?) & Settings (Gear)
        let help_r = Rect::new(self.px + self.pw - 48, self.py + 6, 18, 18);
        let gear_r = Rect::new(self.px + self.pw - 26, self.py + 6, 18, 18);
        self.track_tip(help_r, mx, my, "tip.help");
        self.track_tip(gear_r, mx, my, "tip.settings");
        for (r, lbl) in [(help_r, "?"), (gear_r, "⚙")] {
            let bg = if r.hit(mx, my) { th.hover } else { th.track_bg };
            th.rrect(g, r.x, r.y, r.w, r.h, 4, bg);
            let tw = metrics.width(lbl);
            g.text(
                lbl,
                r.x as f32 + (r.w as f32 - tw) / 2.0,
                (r.y + 5) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );
        }

        // Status / error message in header center
        let (status_msg, error_msg) = {
            let st = ctl.state.lock().unwrap();
            (st.status_msg.clone(), st.error_msg.clone())
        };
        let header_msg = if !error_msg.is_empty() {
            Some((error_msg, PlayerTheme::WARN))
        } else if !status_msg.is_empty() {
            Some((status_msg, PlayerTheme::TEXT_DIM))
        } else {
            None
        };
        if let Some((msg, col)) = header_msg {
            let right_edge = if self.mode == Mode::Main {
                if ctl.has_auth(plat) {
                    self.px + self.pw - 204
                } else {
                    self.px + self.pw - 142
                }
            } else {
                help_r.x
            };
            let avail = (right_edge - (self.chip_rect.x + self.chip_rect.w + 16)).max(40) as f32;
            let trimmed = metrics.ellipsize(&msg, avail);
            g.text(
                trimmed,
                (self.chip_rect.x + self.chip_rect.w + 10) as f32,
                (self.py + 10) as f32,
                col,
                false,
            );
        }

        match self.mode {
            Mode::Picker => self.render_picker(g, metrics, ctl, &th, mx, my),
            Mode::Login => self.render_login(g, metrics, ctl, &th, mx, my),
            Mode::Main => self.render_main(g, metrics, ctl, covers, &pc, &th, mx, my, dt),
        }
        if let Some((anchor, tip)) = self.hover_tip.clone() {
            self.draw_hover_tooltip(g, metrics, &th, anchor, &tip);
        }
    }

    fn track_tip(&mut self, r: Rect, mx: i32, my: i32, key: &str) {
        if r.hit(mx, my) {
            self.hover_tip = Some((r, i18n::get(key)));
        }
    }

    fn draw_hover_tooltip(
        &self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        th: &PlayerTheme,
        anchor: Rect,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let max_w = (self.pw - 24).max(120) as f32;
        let fit = metrics.ellipsize(text, max_w);
        let tw = metrics.width(&fit).ceil() as i32;
        let bw = tw + 14;
        let bh = 18;
        let bx = (anchor.x + anchor.w / 2 - bw / 2).clamp(self.px + 6, self.px + self.pw - bw - 6);
        let above_y = anchor.y - bh - 5;
        let by = if above_y >= self.py + 4 {
            above_y
        } else {
            anchor.y + anchor.h + 5
        };
        shapes::fill(
            g,
            bx - 1,
            by - 1,
            bw + 2,
            bh + 2,
            5,
            PlayerTheme::argb(th.accent_rgb, 140),
        );
        shapes::fill(g, bx, by, bw, bh, 4, 0xF212_121A);
        g.text(
            fit,
            (bx + 7) as f32,
            by as f32 + (bh as f32 - metrics.line_height()) / 2.0,
            PlayerTheme::TEXT_MAIN,
            false,
        );
    }

    fn picker_back_rect(&self) -> Rect {
        Rect::new(self.px + 14, self.py + self.ph - 28, 72, 18)
    }

    fn yandex_tab_rects(&self, y: i32) -> (Rect, Rect, Rect) {
        let wave_w = 76;
        let wave_r = Rect::new(self.lx, y, wave_w, 16);
        let rem = (self.lw - wave_w - 4).max(76);
        let liked_w = (rem * 32 / 100).clamp(44, 62);
        let search_w = rem - liked_w - 4;
        let liked_r = Rect::new(self.lx + wave_w + 4, y, liked_w, 16);
        let search_r = Rect::new(liked_r.x + liked_w + 4, y, search_w, 16);
        (wave_r, liked_r, search_r)
    }

    fn render_picker(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let title = i18n::get("picker.title");
        let tw = metrics.width(&title);
        g.text(
            title,
            self.px as f32 + (self.pw as f32 - tw) / 2.0,
            (self.py + 34) as f32,
            PlayerTheme::TEXT_MAIN,
            true,
        );
        let sub = i18n::get("picker.sub");
        let sub_fit = metrics.ellipsize(&sub, (self.pw - 32) as f32);
        let sw_txt = metrics.width(&sub_fit);
        g.text(
            sub_fit,
            self.px as f32 + (self.pw as f32 - sw_txt) / 2.0,
            (self.py + 48) as f32,
            PlayerTheme::TEXT_DIM,
            false,
        );
        let cards = self.picker_cards();
        let active = ctl.active_platform();
        let elapsed_ms = self.built_at.elapsed().as_secs_f32() * 1000.0;
        let pulse_t = elapsed_ms / 1000.0;
        let playing = ctl.is_playing();
        let cur_plat = ctl.current_track().map(|t| t.platform());

        for (i, (plat, r)) in cards.into_iter().enumerate() {
            let ms = elapsed_ms - i as f32 * 55.0;
            let e = (ms / 240.0).clamp(0.0, 1.0);
            let enter = 1.0 - (1.0 - e).powi(3);
            if enter < 0.06 {
                continue;
            }
            let dy = ((1.0 - enter) * 10.0).round() as i32;
            let hov = r.hit(mx, my);
            let y = r.y + dy - if hov { 2 } else { 0 };
            let br = (th.radius + 3).min(10);
            if plat == active {
                shapes::fill(
                    g,
                    r.x - 1,
                    y - 1,
                    r.w + 2,
                    r.h + 2,
                    br + 1,
                    PlayerTheme::fade(th.accent, enter),
                );
            } else if hov {
                shapes::fill(
                    g,
                    r.x - 1,
                    y - 1,
                    r.w + 2,
                    r.h + 2,
                    br + 1,
                    PlayerTheme::argb(th.accent_rgb, 120),
                );
            }
            let bg = if hov { th.hover } else { th.track_bg };
            shapes::fill(g, r.x, y, r.w, r.h, br, PlayerTheme::fade(bg, enter));

            draw_platform_icon(g, plat, r.x + 11, y + (r.h - 30) / 2, 30);
            let authed = ctl.has_auth(plat);
            let tx = r.x + 52;
            let tw_avail = (r.w - if authed { 112 } else { 72 }).max(40) as f32;
            let ty = y + (r.h - 22) / 2;
            g.text(
                metrics.trim_to_width(&plat.title(), tw_avail),
                tx as f32,
                ty as f32,
                PlayerTheme::fade(PlayerTheme::TEXT_MAIN, enter),
                false,
            );
            let sub = self.card_subtitle(ctl, plat);
            g.text(
                metrics.ellipsize(&sub, tw_avail),
                tx as f32,
                (ty + 12) as f32,
                PlayerTheme::fade(PlayerTheme::TEXT_DIM, enter),
                false,
            );
            if authed {
                let out_r = Rect::new(r.x + r.w - 52, y + (r.h - 16) / 2, 44, 16);
                draw_pill_btn(
                    g,
                    metrics,
                    out_r,
                    &i18n::get("player.logout"),
                    false,
                    th,
                    mx,
                    my,
                );
                let live = playing && cur_plat == Some(plat);
                let alpha = if live {
                    (170.0 + 85.0 * (pulse_t * 4.0).sin()).round() as i32
                } else {
                    255
                };
                PlayerTheme::circle(
                    g,
                    r.x + r.w - 60,
                    y + r.h / 2,
                    3,
                    PlayerTheme::argb(0x4A_DE80, alpha),
                );
            }
        }
        let back_r = self.picker_back_rect();
        draw_pill_btn(
            g,
            metrics,
            back_r,
            &format!("< {}", i18n::get("btn.back")),
            false,
            th,
            mx,
            my,
        );
        self.track_tip(back_r, mx, my, "tip.picker_back");
        let priv_txt = i18n::get("picker.privacy");
        let priv_fit = metrics.ellipsize(&priv_txt, (self.pw - 180) as f32);
        let pw_txt = metrics.width(&priv_fit);
        g.text(
            priv_fit,
            self.px as f32 + (self.pw as f32 - pw_txt) / 2.0,
            (self.py + self.ph - 22) as f32,
            PlayerTheme::TEXT_DIM,
            false,
        );
    }

    fn card_subtitle(&self, ctl: &PlayerController, plat: Platform) -> String {
        let name = ctl.account_name(plat);
        if !plat.needs_login() {
            if name.trim().is_empty() {
                i18n::get("card.no_account")
            } else {
                name
            }
        } else if !ctl.has_auth(plat) {
            i18n::get("card.need_login")
        } else if name.trim().is_empty() {
            i18n::get("card.logged_in")
        } else {
            name
        }
    }

    fn picker_cards(&self) -> [(Platform, Rect); 5] {
        let cw = ((self.pw - 72) / 2).min(240);
        let ch = 48;
        let gap = 10;
        let x0 = self.px + (self.pw - (cw * 2 + gap)) / 2;
        let y0 = self.py + (self.ph - (ch * 3 + gap * 2)) / 2 + 12;
        let xc = self.px + (self.pw - cw) / 2;
        [
            (Platform::Yandex, Rect::new(x0, y0, cw, ch)),
            (Platform::SoundCloud, Rect::new(x0 + cw + gap, y0, cw, ch)),
            (Platform::YouTube, Rect::new(x0, y0 + ch + gap, cw, ch)),
            (
                Platform::Local,
                Rect::new(x0 + cw + gap, y0 + ch + gap, cw, ch),
            ),
            (
                Platform::Spotify,
                Rect::new(xc, y0 + (ch + gap) * 2, cw, ch),
            ),
        ]
    }

    fn render_login(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let plat = ctl.active_platform();
        let cx = self.px + self.pw / 2;
        let draw_centered = |g: &mut DrawList, s: &str, y: i32, col: u32| {
            let fit = metrics.ellipsize(s, (self.pw - 32) as f32);
            let w = metrics.width(&fit);
            g.text(fit, cx as f32 - w / 2.0, y as f32, col, false);
        };

        let back_btn = Rect::new(self.px + 14, self.py + self.ph - 28, 72, 18);
        draw_pill_btn(
            g,
            metrics,
            back_btn,
            &format!("‹ {}", i18n::get("btn.back")),
            false,
            th,
            mx,
            my,
        );

        if plat == Platform::SoundCloud {
            draw_centered(
                g,
                &i18n::get("login.sc_title"),
                self.py + 44,
                PlayerTheme::TEXT_MAIN,
            );
            draw_centered(
                g,
                &i18n::get("login.sc_1"),
                self.py + 62,
                PlayerTheme::TEXT_MID,
            );
            draw_centered(
                g,
                &i18n::get("login.sc_2"),
                self.py + 76,
                PlayerTheme::TEXT_MID,
            );
            self.token_box.set_bounds(cx - 140, self.py + 108, 280, 22);
            self.token_box.render(
                g,
                metrics,
                th.radius,
                th.accent,
                th.track_bg,
                th.field_bg,
                PlayerTheme::TEXT_MAIN,
                PlayerTheme::TEXT_DIM,
            );
            let ok_r = Rect::new(cx - 140, self.py + 138, 134, 22);
            let open_r = Rect::new(cx + 6, self.py + 138, 134, 22);
            draw_pill_btn(
                g,
                metrics,
                ok_r,
                &i18n::get("login.confirm"),
                true,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                open_r,
                &i18n::get("login.open_soundcloud"),
                false,
                th,
                mx,
                my,
            );
            if ctl.has_auth(Platform::SoundCloud) {
                let out_r = Rect::new(cx - 70, self.py + 168, 140, 20);
                draw_pill_btn(
                    g,
                    metrics,
                    out_r,
                    &i18n::get("player.sc_logout"),
                    false,
                    th,
                    mx,
                    my,
                );
            }
            draw_centered(
                g,
                &i18n::get("login.sc_privacy"),
                self.py + self.ph - 22,
                PlayerTheme::TEXT_DIM,
            );
            return;
        }

        if plat == Platform::Spotify {
            draw_centered(
                g,
                &i18n::get("login.sp_title"),
                self.py + 38,
                PlayerTheme::TEXT_MAIN,
            );
            draw_centered(
                g,
                &i18n::get("login.sp_1"),
                self.py + 54,
                PlayerTheme::TEXT_MID,
            );
            let copy_r = Rect::new(cx - 150, self.py + 68, 300, 18);
            draw_pill_btn(g, metrics, copy_r, REDIRECT_URI, false, th, mx, my);
            draw_centered(
                g,
                &i18n::get("login.sp_2"),
                self.py + 92,
                PlayerTheme::TEXT_MID,
            );
            draw_centered(
                g,
                &i18n::get("login.sp_3"),
                self.py + 105,
                PlayerTheme::TEXT_DIM,
            );
            self.token_box.set_bounds(cx - 140, self.py + 124, 280, 22);
            self.token_box.render(
                g,
                metrics,
                th.radius,
                th.accent,
                th.track_bg,
                th.field_bg,
                PlayerTheme::TEXT_MAIN,
                PlayerTheme::TEXT_DIM,
            );
            let ok_r = Rect::new(cx - 140, self.py + 154, 134, 22);
            let open_r = Rect::new(cx + 6, self.py + 154, 134, 22);
            draw_pill_btn(
                g,
                metrics,
                ok_r,
                &i18n::get("login.confirm"),
                true,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                open_r,
                &i18n::get("login.open_spotify"),
                false,
                th,
                mx,
                my,
            );
            if ctl.spotify.auth_state() == SpotifyAuthState::WaitingForBrowser {
                draw_centered(g, &i18n::get("status.sp_browser"), self.py + 186, th.accent);
            } else if ctl.has_auth(Platform::Spotify) {
                let out_r = Rect::new(cx - 70, self.py + 184, 140, 20);
                draw_pill_btn(
                    g,
                    metrics,
                    out_r,
                    &i18n::get("player.logout"),
                    false,
                    th,
                    mx,
                    my,
                );
            }
            draw_centered(
                g,
                &i18n::get("login.sp_privacy"),
                self.py + self.ph - 22,
                PlayerTheme::TEXT_DIM,
            );
            return;
        }

        if plat == Platform::YouTube {
            draw_centered(
                g,
                &i18n::get("login.yt_title"),
                self.py + 44,
                PlayerTheme::TEXT_MAIN,
            );
            draw_centered(
                g,
                &i18n::get("login.yt_1"),
                self.py + 62,
                PlayerTheme::TEXT_MID,
            );
            draw_centered(
                g,
                &i18n::get("login.yt_2"),
                self.py + 76,
                PlayerTheme::TEXT_MID,
            );
            self.token_box.set_bounds(cx - 140, self.py + 108, 280, 22);
            self.token_box.render(
                g,
                metrics,
                th.radius,
                th.accent,
                th.track_bg,
                th.field_bg,
                PlayerTheme::TEXT_MAIN,
                PlayerTheme::TEXT_DIM,
            );
            let ok_r = Rect::new(cx - 140, self.py + 138, 134, 22);
            let open_r = Rect::new(cx + 6, self.py + 138, 134, 22);
            draw_pill_btn(
                g,
                metrics,
                ok_r,
                &i18n::get("login.confirm"),
                true,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                open_r,
                &i18n::get("login.open_yt"),
                false,
                th,
                mx,
                my,
            );
            if ctl.has_auth(Platform::YouTube) {
                let out_r = Rect::new(cx - 70, self.py + 168, 140, 20);
                draw_pill_btn(
                    g,
                    metrics,
                    out_r,
                    &i18n::get("player.logout"),
                    false,
                    th,
                    mx,
                    my,
                );
            }
            draw_centered(
                g,
                &i18n::get("login.yt_privacy"),
                self.py + self.ph - 22,
                PlayerTheme::TEXT_DIM,
            );
            return;
        }

        // Yandex login
        let saved = ctl.saved_yandex_accounts();
        if !self.add_account_mode && self.device_auth.is_none() && !saved.is_empty() {
            draw_centered(
                g,
                &i18n::get("login.choose"),
                self.py + 40,
                PlayerTheme::TEXT_MAIN,
            );
            draw_centered(
                g,
                &i18n::get("login.saved_sub"),
                self.py + 52,
                PlayerTheme::TEXT_DIM,
            );
            let shown = saved.len().min(MAX_ACCOUNT_ROWS);
            for (i, acc) in saved.iter().take(shown).enumerate() {
                let ry = self.py + 68 + i as i32 * 26;
                let row_r = Rect::new(cx - 120, ry, 212, 22);
                let del_r = Rect::new(cx + 96, ry, 24, 22);
                draw_pill_btn(g, metrics, row_r, &acc.label, false, th, mx, my);
                draw_pill_btn(g, metrics, del_r, "x", false, th, mx, my);
            }
            let add_r = Rect::new(cx - 100, self.py + 72 + shown as i32 * 26, 200, 22);
            draw_pill_btn(
                g,
                metrics,
                add_r,
                &i18n::get("login.add_account"),
                true,
                th,
                mx,
                my,
            );
            draw_centered(
                g,
                &i18n::get("login.forget"),
                self.py + self.ph - 22,
                PlayerTheme::TEXT_DIM,
            );
            return;
        }

        draw_centered(
            g,
            &i18n::get("login.ya_title"),
            self.py + 38,
            PlayerTheme::TEXT_MAIN,
        );
        if let Some(ref code) = self.device_auth {
            draw_centered(
                g,
                &i18n::get("login.code_1"),
                self.py + 56,
                PlayerTheme::TEXT_MID,
            );
            draw_centered(
                g,
                &i18n::get("login.code_2"),
                self.py + 70,
                PlayerTheme::TEXT_MID,
            );
            let shown_code = code.user_code.to_ascii_uppercase();
            g.push_scale(2.2);
            let cw = metrics.width(&shown_code) * 2.2;
            g.text(
                &shown_code,
                (cx as f32 - cw / 2.0) / 2.2,
                (self.py + 88) as f32 / 2.2,
                th.accent,
                true,
            );
            g.pop_scale();
            draw_centered(
                g,
                &i18n::get("login.code_3"),
                self.py + 120,
                PlayerTheme::TEXT_MID,
            );
            let left = code
                .expires_at
                .saturating_duration_since(Instant::now())
                .as_secs();
            let rem = format!("{}:{:02}", left / 60, left % 60);
            draw_centered(
                g,
                &i18n::fmt("login.code_4", &[&rem]),
                self.py + 134,
                PlayerTheme::TEXT_DIM,
            );

            let r1 = Rect::new(cx - 150, self.py + 158, 144, 20);
            let r2 = Rect::new(cx + 6, self.py + 158, 144, 20);
            let r3 = Rect::new(cx - 60, self.py + 184, 120, 20);
            draw_pill_btn(
                g,
                metrics,
                r1,
                &i18n::get("login.code_reopen"),
                false,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                r2,
                &i18n::get("login.code_copy"),
                false,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                r3,
                &i18n::get("login.code_cancel"),
                false,
                th,
                mx,
                my,
            );
            return;
        }

        draw_centered(
            g,
            &i18n::get("login.code_hint"),
            self.py + 54,
            PlayerTheme::TEXT_MID,
        );
        let code_r = Rect::new(cx - 115, self.py + 70, 230, 22);
        draw_pill_btn(
            g,
            metrics,
            code_r,
            &i18n::get("login.code_button"),
            true,
            th,
            mx,
            my,
        );
        draw_centered(
            g,
            &i18n::get("login.or_link"),
            self.py + 102,
            PlayerTheme::TEXT_DIM,
        );

        self.token_box.set_bounds(cx - 140, self.py + 118, 280, 22);
        self.token_box.render(
            g,
            metrics,
            th.radius,
            th.accent,
            th.track_bg,
            th.field_bg,
            PlayerTheme::TEXT_MAIN,
            PlayerTheme::TEXT_DIM,
        );
        let ok_r = Rect::new(cx - 140, self.py + 146, 134, 22);
        let open_r = Rect::new(cx + 6, self.py + 146, 134, 22);
        draw_pill_btn(
            g,
            metrics,
            ok_r,
            &i18n::get("login.confirm"),
            true,
            th,
            mx,
            my,
        );
        draw_pill_btn(
            g,
            metrics,
            open_r,
            &i18n::get("login.open_yandex"),
            false,
            th,
            mx,
            my,
        );

        if !saved.is_empty() {
            let back_saved = Rect::new(cx - 90, self.py + 176, 180, 20);
            draw_pill_btn(
                g,
                metrics,
                back_saved,
                &i18n::get("login.back_saved"),
                false,
                th,
                mx,
                my,
            );
        }
        draw_centered(
            g,
            &i18n::get("login.ya_privacy"),
            self.py + self.ph - 22,
            PlayerTheme::TEXT_DIM,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_main(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        covers: &CoverArtManager,
        pc: &PlayerConfig,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
        dt: f32,
    ) {
        let plat = ctl.active_platform();
        // Top-right Quick buttons: Logout (when signed in) / Queue
        if ctl.has_auth(plat) {
            let logout_r = Rect::new(self.px + self.pw - 196, self.py + 6, 56, 18);
            draw_pill_btn(
                g,
                metrics,
                logout_r,
                &i18n::get("player.logout"),
                false,
                th,
                mx,
                my,
            );
            self.track_tip(logout_r, mx, my, "tip.logout");
        }
        let q_len = ctl.queue_snapshot().len();
        let q_lbl = format!("{} {}", i18n::get("player.queue"), q_len);
        let q_btn = Rect::new(self.px + self.pw - 134, self.py + 6, 78, 18);
        draw_pill_btn(g, metrics, q_btn, &q_lbl, self.queue_mode, th, mx, my);

        // Left panel background
        th.rrect(
            g,
            self.px + 8,
            self.py + 26,
            self.lw + 8,
            self.ph - 34,
            (th.radius - 2).max(0),
            th.panel2,
        );

        // Search box + Find btn
        self.search_box.render(
            g,
            metrics,
            th.radius,
            th.accent,
            th.track_bg,
            th.field_bg,
            PlayerTheme::TEXT_MAIN,
            PlayerTheme::TEXT_DIM,
        );
        let find_r = Rect::new(self.lx + self.lw - 52, self.py + 30, 52, 18);
        draw_pill_btn(
            g,
            metrics,
            find_r,
            &i18n::get("player.find"),
            false,
            th,
            mx,
            my,
        );

        // Platform sub-bar: Tabs / Liked / Folder / Wave
        self.draw_left_tabs(g, metrics, ctl, plat, th, mx, my);

        // Track/Entity list + pagination + volume
        self.draw_list(g, metrics, ctl, th, mx, my);
        let max_p = self.max_page(ctl);
        let (prev_r, next_r, pager_lbl_y) = self.pager_rects();
        let page_lbl = format!("{} / {}", self.page + 1, max_p + 1);
        let pw = metrics.width(&page_lbl);
        g.text(
            page_lbl,
            (self.lx + self.lw / 2) as f32 - pw / 2.0,
            pager_lbl_y as f32,
            PlayerTheme::TEXT_DIM,
            false,
        );
        draw_pill_btn(g, metrics, prev_r, "<", false, th, mx, my);
        draw_pill_btn(g, metrics, next_r, ">", false, th, mx, my);

        self.draw_volume(g, metrics, ctl, th, mx, my);

        // Right panel: Now playing + Lyrics + Controls
        let cur = ctl.current_track();
        let Some(ref tr) = cur else {
            self.artist_rect = Rect::default();
            self.album_rect = Rect::default();
            let mid_y = self.py + 40 + (self.by - self.py - 40) / 2 - 8;
            let mid_x = (self.rx + self.rw / 2) as f32;
            if plat == Platform::Local {
                let s1 = i18n::get("player.drop");
                let s2 = i18n::get("player.drop2");
                g.text(
                    &s1,
                    mid_x - metrics.width(&s1) / 2.0,
                    mid_y as f32,
                    PlayerTheme::TEXT_MID,
                    false,
                );
                g.text(
                    &s2,
                    mid_x - metrics.width(&s2) / 2.0,
                    (mid_y + 12) as f32,
                    PlayerTheme::TEXT_DIM,
                    false,
                );
            } else {
                let s1 = i18n::get("player.empty");
                g.text(
                    &s1,
                    mid_x - metrics.width(&s1) / 2.0,
                    mid_y as f32,
                    PlayerTheme::TEXT_MID,
                    false,
                );
            }
            self.draw_progress(g, metrics, ctl, None, th, mx, my, dt);
            self.draw_transport(g, metrics, ctl, None, th, mx, my);
            return;
        };

        if self.anim_track_id != tr.id {
            self.anim_track_id = tr.id.clone();
            self.anim_start = Instant::now();
        }
        let ap = (self.anim_start.elapsed().as_secs_f32() / 0.45).clamp(0.0, 1.0);
        let ae = 1.0 - (1.0 - ap).powi(3);
        let slide = ((1.0 - ae) * 14.0).round() as i32;
        let aa = (255.0 * ae).round().max(5.0) as i32;
        let secs = self.built_at.elapsed().as_secs_f64();
        let wave = ctl.wave_shown();
        let playing = ctl.is_playing();

        let mut text_x = self.rx;
        if self.cover_px > 0 {
            covers.offer_big(&tr.id, tr.cover_url.as_deref());
            let cs = ((self.cover_px as f32) * (0.9 + 0.1 * ae)).round() as i32;
            let cx0 = self.rx + (self.cover_px - cs) / 2;
            let cy0 = self.py + 32 + (self.cover_px - cs) / 2;
            if wave && playing {
                let pulse = (0.5 + 0.5 * (secs * 2.0).sin()) as f32;
                let rgb = wave_palette_at(secs * 0.5);
                shapes::ring(
                    g,
                    self.rx - 2,
                    self.py + 30,
                    self.cover_px + 4,
                    self.cover_px + 4,
                    3,
                    PlayerTheme::argb(rgb, (90.0 + 90.0 * pulse).round() as i32),
                );
                shapes::ring(
                    g,
                    self.rx - 3,
                    self.py + 29,
                    self.cover_px + 6,
                    self.cover_px + 6,
                    3,
                    PlayerTheme::argb(rgb, (30.0 + 40.0 * pulse).round() as i32),
                );
            }
            th.rrect(
                g,
                cx0 + 2,
                cy0 + 4,
                cs,
                cs,
                8,
                PlayerTheme::argb(0, (85.0 * ae).round() as i32),
            );
            if covers.has_big(&tr.id) {
                g.texture(
                    TexSlot::CoverBig,
                    cx0 as f32,
                    cy0 as f32,
                    cs as f32,
                    cs as f32,
                    ae,
                );
            } else {
                th.rrect(g, cx0, cy0, cs, cs, 8, th.track_bg);
                let nw = metrics.width("♪");
                g.text(
                    "♪",
                    cx0 as f32 + (cs as f32 - nw) / 2.0,
                    cy0 as f32 + (cs as f32 - metrics.line_height()) / 2.0 + 1.0,
                    PlayerTheme::TEXT_DIM,
                    false,
                );
            }
            text_x = self.rx + self.cover_px + 14;
        }

        let avail_w = (self.rx + self.rw - text_x - if wave { 44 } else { 12 }).max(60);
        let ts = if self.cover_px >= 60 { 1.4 } else { 1.0 };
        let title_col = PlayerTheme::argb(
            if pc.accent_text {
                th.accent
            } else {
                PlayerTheme::TEXT_MAIN
            },
            aa,
        );
        let mid_col = PlayerTheme::argb(PlayerTheme::TEXT_MID, aa);
        let dim_col = PlayerTheme::argb(PlayerTheme::TEXT_DIM, aa);
        let ty = self.py + 34;

        g.push_scale(ts);
        let t_fit = metrics.trim_to_width(&tr.title, avail_w as f32 / ts);
        g.text(
            t_fit,
            ((text_x + slide) as f32) / ts,
            (ty as f32) / ts,
            title_col,
            true,
        );
        g.pop_scale();

        let ay = ty + (metrics.line_height() * ts).round() as i32 + 4;
        let artist_text = metrics.trim_to_width(&tr.artist, avail_w as f32);
        let yandex_track = tr.platform() == Platform::Yandex;
        let aw = metrics.width(&artist_text).round() as i32;
        self.artist_rect = if yandex_track && !artist_text.is_empty() {
            Rect::new(text_x, ay, aw, metrics.line_height().round() as i32 + 2)
        } else {
            Rect::default()
        };
        g.text(
            &artist_text,
            (text_x + slide) as f32,
            ay as f32,
            if self.artist_rect.hit(mx, my) {
                th.accent
            } else {
                mid_col
            },
            false,
        );

        let mut next_y = ay + metrics.line_height().round() as i32 + 3;
        self.album_rect = Rect::default();
        if self.cover_px >= 60 && !tr.album.trim().is_empty() {
            let alb_text = metrics.trim_to_width(&tr.album, avail_w as f32);
            let alw = metrics.width(&alb_text).round() as i32;
            if yandex_track && !alb_text.is_empty() {
                self.album_rect = Rect::new(
                    text_x,
                    next_y,
                    alw,
                    metrics.line_height().round() as i32 + 2,
                );
            }
            g.text(
                &alb_text,
                (text_x + slide) as f32,
                next_y as f32,
                if self.album_rect.hit(mx, my) {
                    th.accent
                } else {
                    dim_col
                },
                false,
            );
            next_y += metrics.line_height().round() as i32 + 3;
        }
        if wave && self.cover_px >= 60 {
            let bl = i18n::get("player.wave");
            let bw = metrics.width(&bl).round() as i32 + 14;
            shapes::fill(
                g,
                text_x + slide,
                next_y + 2,
                bw,
                12,
                3,
                PlayerTheme::argb(wave_palette_at(secs * 0.35 * WAVE_PALETTE.len() as f64), aa),
            );
            g.text(
                bl,
                (text_x + slide + 7) as f32,
                (next_y + 4) as f32,
                PlayerTheme::argb(0xFFFFFF, aa),
                false,
            );
        }

        if pc.show_lyrics && self.ly_bottom - self.ly_top > 28 {
            self.render_lyrics(g, metrics, ctl, tr, th, mx, my, dt);
            self.draw_lyrics_control_strip(g, metrics, ctl, tr, pc, th, mx, my);
        }

        self.draw_progress(g, metrics, ctl, Some(tr), th, mx, my, dt);
        self.draw_transport(g, metrics, ctl, Some(tr), th, mx, my);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_left_tabs(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        plat: Platform,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let y = self.py + 52;
        if plat == Platform::Local {
            let bw = (self.lw - 8) / 3;
            draw_pill_btn(
                g,
                metrics,
                Rect::new(self.lx, y, bw, 16),
                &i18n::get("player.folder"),
                false,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                Rect::new(self.lx + bw + 4, y, bw, 16),
                &i18n::get("player.refresh"),
                false,
                th,
                mx,
                my,
            );
            draw_pill_btn(
                g,
                metrics,
                Rect::new(self.lx + (bw + 4) * 2, y, self.lw - (bw + 4) * 2, 16),
                &i18n::get("player.likes"),
                false,
                th,
                mx,
                my,
            );
            return;
        }

        if plat == Platform::Yandex {
            let (wave_r, liked_r, search_r) = self.yandex_tab_rects(y);
            self.draw_wave_button(g, metrics, ctl, wave_r, th, mx, my);
            self.track_tip(wave_r, mx, my, "tip.wave");
            let cur_tab = ctl.state.lock().unwrap().tab;
            for (tab, r, tip) in [
                (Tab::Liked, liked_r, "tip.likes"),
                (Tab::Search, search_r, "tip.tab_search"),
            ] {
                let lbl = metrics.trim_to_width(&i18n::get(tab_key(tab)), (r.w - 6) as f32);
                draw_pill_btn(
                    g,
                    metrics,
                    r,
                    &lbl,
                    cur_tab == tab && !self.queue_mode,
                    th,
                    mx,
                    my,
                );
                self.track_tip(r, mx, my, tip);
            }
            return;
        }

        // YouTube: Likes + Charts (+ Login when signed out; top bar shows Logout when signed in)
        if plat == Platform::YouTube {
            if ctl.has_auth(Platform::YouTube) {
                let bw = (self.lw - 4) / 2;
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx, y, bw, 16),
                    &i18n::get("player.likes"),
                    false,
                    th,
                    mx,
                    my,
                );
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx + bw + 4, y, self.lw - bw - 4, 16),
                    &i18n::get("player.yt_charts"),
                    false,
                    th,
                    mx,
                    my,
                );
            } else {
                let bw = (self.lw - 8) / 3;
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx, y, bw, 16),
                    &i18n::get("player.likes"),
                    false,
                    th,
                    mx,
                    my,
                );
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx + bw + 4, y, bw, 16),
                    &i18n::get("player.yt_charts"),
                    false,
                    th,
                    mx,
                    my,
                );
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx + (bw + 4) * 2, y, self.lw - (bw + 4) * 2, 16),
                    &i18n::get("player.sc_login"),
                    false,
                    th,
                    mx,
                    my,
                );
            }
            return;
        }

        // SoundCloud / Spotify: Likes + Account Login/Logout
        let bw = (self.lw - 4) / 2;
        draw_pill_btn(
            g,
            metrics,
            Rect::new(self.lx, y, bw, 16),
            &i18n::get("player.likes"),
            false,
            th,
            mx,
            my,
        );
        let right_lbl = if ctl.has_auth(plat) {
            i18n::get(if plat == Platform::SoundCloud {
                "player.sc_logout"
            } else {
                "player.logout"
            })
        } else {
            i18n::get("player.sc_login")
        };
        draw_pill_btn(
            g,
            metrics,
            Rect::new(self.lx + bw + 4, y, self.lw - bw - 4, 16),
            &right_lbl,
            false,
            th,
            mx,
            my,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_wave_button(
        &self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        r: Rect,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let hov = r.hit(mx, my);
        let on = ctl.wave_shown();
        let fetching = ctl.state.lock().unwrap().wave_fetching;
        let secs = self.built_at.elapsed().as_secs_f64();
        let rad = (th.radius + 2).min(r.h / 2);
        let wrgb = wave_palette_at(secs * 0.18 * WAVE_PALETTE.len() as f64);
        let fill_rgb = PlayerTheme::mix(wrgb, 0x11_1115, if hov { 0.25 } else { 0.38 });
        shapes::fill(g, r.x, r.y, r.w, r.h, rad, 0xFF00_0000 | fill_rgb);
        if on {
            shapes::ring(g, r.x, r.y, r.w, r.h, rad, PlayerTheme::argb(0xFF_FFFF, 70));
        }
        let playing = on && ctl.is_playing();
        for i in 0..4 {
            let k = if playing {
                0.5 + 0.5 * (secs * (5.0 + i as f64 * 1.7) + i as f64 * 1.3).sin()
            } else {
                0.25
            };
            let bh = 2 + (k * (r.h - 8) as f64).round() as i32;
            let bx = r.x + 5 + i * 3;
            let base = r.y + r.h - 3;
            g.fill(
                bx as f32,
                (base - bh) as f32,
                (bx + 2) as f32,
                base as f32,
                PlayerTheme::TEXT_MAIN,
            );
        }
        let raw = if fetching {
            i18n::get("player.wave_starting")
        } else {
            i18n::get(if on {
                "player.wave_stop"
            } else {
                "player.wave"
            })
        };
        let lbl = metrics.trim_to_width(&raw, (r.w - 22) as f32);
        let tx = (r.x + 19) as f32 + ((r.w - 21) as f32 - metrics.width(&lbl)) / 2.0;
        g.text(lbl, tx, (r.y + 4) as f32, PlayerTheme::TEXT_MAIN, true);
    }

    fn collect_items(
        &self,
        ctl: &PlayerController,
    ) -> (Vec<Track>, Vec<MusicEntity>, Vec<RowKind>) {
        if self.queue_mode {
            let tracks = ctl.queue_snapshot();
            let rows = (0..tracks.len()).map(RowKind::Track).collect();
            return (tracks, Vec::new(), rows);
        }
        if let Some(pg) = ctl.current_page() {
            let mut rows = Vec::with_capacity(pg.entities.len() + pg.tracks.len());
            for i in 0..pg.entities.len() {
                rows.push(RowKind::Entity(i));
            }
            for i in 0..pg.tracks.len() {
                rows.push(RowKind::Track(i));
            }
            return (pg.tracks, pg.entities, rows);
        }
        if ctl.wave_shown() {
            let tracks = ctl.wave_snapshot();
            let rows = (0..tracks.len()).map(RowKind::Track).collect();
            return (tracks, Vec::new(), rows);
        }
        let st = ctl.state.lock().unwrap();
        if !st.search_tracks.is_empty() || !st.search_entities.is_empty() {
            let tracks = st.search_tracks.clone();
            let entities = st.search_entities.clone();
            let mut rows = Vec::new();
            for i in 0..entities.len() {
                rows.push(RowKind::Entity(i));
            }
            for i in 0..tracks.len() {
                rows.push(RowKind::Track(i));
            }
            return (tracks, entities, rows);
        }
        match st.tab {
            Tab::Wave => {
                let entities = st.stations.clone();
                let rows = (0..entities.len()).map(RowKind::Entity).collect();
                (Vec::new(), entities, rows)
            }
            Tab::Liked => {
                let tracks = st.liked.clone();
                let rows = (0..tracks.len()).map(RowKind::Track).collect();
                (tracks, Vec::new(), rows)
            }
            Tab::Search => {
                let mut rows = Vec::new();
                for i in 0..st.playlists.len() {
                    rows.push(RowKind::Entity(i));
                }
                let tracks = st.liked.clone();
                for i in 0..tracks.len() {
                    rows.push(RowKind::Track(i));
                }
                (tracks, st.playlists.clone(), rows)
            }
        }
    }

    fn max_page(&self, ctl: &PlayerController) -> usize {
        let (_, _, rows) = self.collect_items(ctl);
        if rows.is_empty() {
            0
        } else {
            (rows.len() - 1) / self.rows_per_page.max(1)
        }
    }

    fn draw_list(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        if let Some(pg) = ctl.current_page() {
            let hdr_y = self.list_top - 19;
            draw_pill_btn(
                g,
                metrics,
                Rect::new(self.lx, hdr_y, 52, 15),
                &format!("‹ {}", i18n::get("btn.back")),
                false,
                th,
                mx,
                my,
            );
            let title = if pg.source.is_mine() {
                i18n::get("entity.mine")
            } else {
                pg.source.title.clone()
            };
            let play_w = if pg.tracks.is_empty() { 0 } else { 78 };
            let trimmed = metrics.trim_to_width(&title, (self.lw - 58 - play_w) as f32);
            g.text(
                trimmed,
                (self.lx + 56) as f32,
                (hdr_y + 4) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );
            if !pg.tracks.is_empty() {
                draw_pill_btn(
                    g,
                    metrics,
                    Rect::new(self.lx + self.lw - 74, hdr_y, 74, 15),
                    &format!("▶ {}", i18n::get("player.play_all")),
                    true,
                    th,
                    mx,
                    my,
                );
            }
        }

        let (tracks, entities, rows) = self.collect_items(ctl);
        if rows.is_empty() {
            let wave_loading = ctl.state.lock().unwrap().wave_fetching;
            let key = if self.queue_mode {
                "player.queue_empty"
            } else if wave_loading {
                "player.wave_loading"
            } else if ctl.active_platform() == Platform::Local {
                "player.no_files"
            } else {
                "player.tracks_here"
            };
            let msg = i18n::get(key);
            let mw = metrics.width(&msg);
            g.text(
                msg,
                (self.lx + self.lw / 2) as f32 - mw / 2.0,
                (self.list_top + 14) as f32,
                PlayerTheme::TEXT_DIM,
                false,
            );
            return;
        }

        let max_p = (rows.len() - 1) / self.rows_per_page.max(1);
        if self.page > max_p {
            self.page = max_p;
            ctl.set_list_page(max_p);
        }
        let cur_id = ctl.current_track().map(|t| t.id).unwrap_or_default();
        let text_w = (self.lw - 18 - ACT_W * 2).max(40);
        let playing = ctl.is_playing();
        let secs = self.built_at.elapsed().as_secs_f64();

        for i in 0..self.rows_per_page {
            let idx = self.page * self.rows_per_page + i;
            let Some(&kind) = rows.get(idx) else {
                break;
            };
            let y = self.list_top + i as i32 * ROW_H;
            let row_rect = Rect::new(self.lx, y, self.lw, ROW_H - 2);
            let hov = row_rect.hit(mx, my);

            match kind {
                RowKind::Track(ti) => {
                    let Some(t) = tracks.get(ti) else {
                        continue;
                    };
                    let is_cur = !cur_id.is_empty() && t.id == cur_id;
                    self.row_glow[i] = smooth_step(self.row_glow[i], hov && !is_cur);
                    if is_cur {
                        th.rrect(g, self.lx, y, self.lw, ROW_H - 2, 4, th.row_current);
                        draw_equalizer(g, self.lx + 5, y + 4, ROW_H - 10, playing, secs, th.accent);
                    } else if self.row_glow[i] > 0.02 {
                        th.rrect(
                            g,
                            self.lx,
                            y,
                            self.lw,
                            ROW_H - 2,
                            4,
                            PlayerTheme::blend(th.panel2, th.hover, self.row_glow[i]),
                        );
                    }
                    let tx = if is_cur { self.lx + 16 } else { self.lx + 8 };
                    let tw = (if is_cur { text_w - 8 } else { text_w }) as f32;
                    g.text(
                        metrics.trim_to_width(&t.title, tw),
                        tx as f32,
                        (y + 3) as f32,
                        if is_cur {
                            th.accent
                        } else {
                            PlayerTheme::TEXT_MAIN
                        },
                        false,
                    );
                    g.text(
                        metrics.trim_to_width(&t.artist, tw),
                        tx as f32,
                        (y + 13) as f32,
                        PlayerTheme::TEXT_DIM,
                        false,
                    );
                    if !hov && !is_cur && ctl.cache.has(&t.id) {
                        PlayerTheme::circle(
                            g,
                            self.lx + self.lw - ACT_W - 4,
                            y + (ROW_H - 2) / 2,
                            2,
                            PlayerTheme::argb(th.accent_rgb, 150),
                        );
                    }
                    if hov || is_cur {
                        let (a1, a2) = if self.queue_mode {
                            ("^", "x")
                        } else {
                            ("+", ">")
                        };
                        let r1 = Rect::new(
                            self.lx + self.lw - ACT_W * 2 - 2,
                            y + 3,
                            ACT_W - 2,
                            ROW_H - 8,
                        );
                        let r2 = Rect::new(self.lx + self.lw - ACT_W, y + 3, ACT_W - 2, ROW_H - 8);
                        th.rrect(
                            g,
                            r1.x,
                            r1.y,
                            r1.w,
                            r1.h,
                            3,
                            if r1.hit(mx, my) {
                                th.accent
                            } else {
                                th.track_bg
                            },
                        );
                        th.rrect(
                            g,
                            r2.x,
                            r2.y,
                            r2.w,
                            r2.h,
                            3,
                            if r2.hit(mx, my) {
                                th.accent
                            } else {
                                th.track_bg
                            },
                        );
                        g.text(
                            a1,
                            (r1.x + r1.w / 2) as f32 - metrics.width(a1) / 2.0,
                            (y + 7) as f32,
                            PlayerTheme::TEXT_MAIN,
                            false,
                        );
                        g.text(
                            a2,
                            (r2.x + r2.w / 2) as f32 - metrics.width(a2) / 2.0,
                            (y + 7) as f32,
                            PlayerTheme::TEXT_MAIN,
                            false,
                        );
                    }
                }
                RowKind::Entity(ei) => {
                    let Some(ent) = entities.get(ei) else {
                        continue;
                    };
                    self.row_glow[i] = smooth_step(self.row_glow[i], hov);
                    if self.row_glow[i] > 0.02 {
                        th.rrect(
                            g,
                            self.lx,
                            y,
                            self.lw,
                            ROW_H - 2,
                            4,
                            PlayerTheme::blend(th.panel2, th.hover, self.row_glow[i]),
                        );
                    }
                    let sz = ROW_H - 6;
                    let wave_tok = ctl.state.lock().unwrap().wave_token.clone();
                    let playing_ent =
                        ent.kind == EntityKind::Station && ctl.wave_shown() && ent.id == wave_tok;
                    let box_col = if ent.is_mine() || playing_ent {
                        th.accent
                    } else {
                        PlayerTheme::argb(th.accent_rgb, if hov { 160 } else { 110 })
                    };
                    if ent.kind == EntityKind::Artist {
                        PlayerTheme::circle(
                            g,
                            self.lx + 4 + sz / 2,
                            y + 2 + sz / 2,
                            sz / 2,
                            box_col,
                        );
                    } else {
                        th.rrect(g, self.lx + 4, y + 2, sz, sz, 4, box_col);
                    }
                    let letter: String = if ent.is_mine() {
                        "★".to_owned()
                    } else {
                        ent.title
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_else(|| "?".to_owned())
                    };
                    let lw = metrics.width(&letter);
                    g.text(
                        letter,
                        (self.lx + 4) as f32 + (sz as f32 - lw) / 2.0,
                        (y + 8) as f32,
                        PlayerTheme::TEXT_MAIN,
                        false,
                    );
                    let tx = self.lx + sz + 10;
                    let tw = (self.lw - sz - 16).max(30) as f32;
                    let title = if ent.is_mine() {
                        i18n::get("entity.mine")
                    } else {
                        ent.title.clone()
                    };
                    let sub = if ent.is_mine() {
                        i18n::get("entity.mine_sub")
                    } else {
                        ent.subtitle.clone()
                    };
                    g.text(
                        metrics.trim_to_width(&title, tw),
                        tx as f32,
                        (y + 3) as f32,
                        if hov {
                            th.accent
                        } else {
                            PlayerTheme::TEXT_MAIN
                        },
                        false,
                    );
                    g.text(
                        metrics.trim_to_width(&sub, tw),
                        tx as f32,
                        (y + 13) as f32,
                        PlayerTheme::TEXT_DIM,
                        false,
                    );
                }
            }
        }
    }

    fn pager_rects(&self) -> (Rect, Rect, i32) {
        let pager_y = self.py + self.ph - 45;
        let prev_r = Rect::new(self.lx, pager_y, 20, 14);
        let next_r = Rect::new(self.lx + self.lw - 20, pager_y, 20, 14);
        (prev_r, next_r, pager_y + 3)
    }

    fn mute_rect(&self) -> Rect {
        Rect::new(self.lx, self.cy - 9, 20, 18)
    }

    fn set_page(&mut self, ctl: &PlayerController, page: usize) {
        self.page = page;
        ctl.set_list_page(page);
    }

    fn set_queue_mode(&mut self, ctl: &PlayerController, q: bool) {
        self.queue_mode = q;
        ctl.set_queue_mode(q);
    }

    fn draw_volume(
        &self,
        g: &mut DrawList,
        _metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let vol = ctl.volume();
        let mute_r = self.mute_rect();
        let bg = if mute_r.hit(mx, my) {
            th.hover
        } else {
            th.track_bg
        };
        th.rrect(g, mute_r.x, mute_r.y, mute_r.w, mute_r.h, 4, bg);
        PlayerTheme::icon_volume(
            g,
            mute_r.x + 9,
            self.cy,
            if vol <= 0.0 {
                0
            } else if vol < 0.5 {
                1
            } else {
                2
            },
            PlayerTheme::TEXT_MAIN,
        );

        let bh = 4;
        let bar_y = self.cy - bh / 2;
        th.rrect(g, self.vol_bar_x, bar_y, VOL_BAR_W, bh, 2, th.track_bg);
        let fill = (VOL_BAR_W as f32 * vol).round() as i32;
        if fill > 0 {
            th.rrect(g, self.vol_bar_x, bar_y, fill.max(bh), bh, 2, th.accent);
        }
        let kx = (self.vol_bar_x + fill).clamp(self.vol_bar_x + 3, self.vol_bar_x + VOL_BAR_W - 3);
        PlayerTheme::circle(g, kx, self.cy, 4, PlayerTheme::TEXT_MAIN);
        let pct = format!("{}%", (vol * 100.0).round() as i32);
        g.text(
            pct,
            (self.vol_bar_x + VOL_BAR_W + 6) as f32,
            (self.cy - 4) as f32,
            PlayerTheme::TEXT_DIM,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_lyrics(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        tr: &Track,
        th: &PlayerTheme,
        _mx: i32,
        _my: i32,
        dt: f32,
    ) {
        th.rrect(
            g,
            self.rx - 4,
            self.ly_top - 4,
            self.rw + 8,
            self.ly_bottom - self.ly_top + 8,
            4,
            PlayerTheme::argb(th.panel2, 150),
        );
        let mid_x = (self.rx + self.rw / 2) as f32;
        let mid_y = ((self.ly_top + self.ly_bottom) / 2 - 4) as f32;

        let draw_centered = |g: &mut DrawList, msg: &str| {
            let lines = metrics.wrap(msg, (self.rw - 16).max(80) as f32);
            let lh = (metrics.line_height().round() + 2.0).max(10.0);
            let start_y = mid_y - ((lines.len().saturating_sub(1)) as f32 * lh) / 2.0;
            for (idx, line) in lines.iter().enumerate() {
                g.text(
                    line,
                    mid_x - metrics.width(line) / 2.0,
                    start_y + idx as f32 * lh,
                    PlayerTheme::TEXT_DIM,
                    false,
                );
            }
        };

        if ctl.is_idle() {
            draw_centered(g, &i18n::get("player.press_play"));
            return;
        }

        let loading = ctl.state.lock().unwrap().lyrics_loading;
        let cand_opt = ctl.active_lyrics();
        let Some(cand) = cand_opt else {
            let key = if loading {
                "player.lyrics_loading"
            } else if ctl.plain_lyrics_waiting_for_dsp() {
                "err.lyrics_unsynced_blocked"
            } else {
                "player.lyrics_none"
            };
            draw_centered(g, &i18n::get(key));
            return;
        };

        // Never render unsynced lines on screen
        if !cand.lines.iter().any(LyricLine::is_synced) {
            let key = if loading {
                "player.lyrics_loading"
            } else {
                "err.lyrics_unsynced_blocked"
            };
            draw_centered(g, &i18n::get(key));
            return;
        }

        self.ensure_wrapped(metrics, &tr.id, cand.source.label(), &cand.lines);
        let cur = ctl.current_lyric_index();
        let pos_ms = ctl.position_ms();
        let lh = metrics.line_height().round() as i32 + 2;
        let view_h = self.ly_bottom - self.ly_top;
        let auto_target = cur
            .and_then(|c| {
                self.tops
                    .get(c)
                    .map(|&t| t as f32 + self.wrapped[c].len() as f32 * lh as f32 / 2.0)
            })
            .unwrap_or(0.0);
        let target = (auto_target + self.manual_offset_px).max(0.0);
        self.scroll_px += (target - self.scroll_px) * (1.0 - (-dt.max(0.001) * 10.0).exp());
        let origin = self.ly_top + view_h / 2;

        g.push_scissor(
            self.rx as f32,
            self.ly_top as f32,
            self.rw as f32,
            view_h as f32,
        );
        for (i, wlines) in self.wrapped.iter().enumerate() {
            let y = origin + self.tops[i] - self.scroll_px.round() as i32;
            let h = wlines.len() as i32 * lh;
            if y + h < self.ly_top || y > self.ly_bottom {
                continue;
            }
            let is_cur = cur == Some(i);
            let color = if is_cur {
                th.accent
            } else {
                let d = cur
                    .map(|c| (i as isize - c as isize).abs() as f32)
                    .unwrap_or(4.0);
                let near = (1.0 - d / 4.0).clamp(0.0, 1.0);
                0xFF00_0000 | PlayerTheme::mix(PlayerTheme::TEXT_DIM, PlayerTheme::TEXT_MID, near)
            };
            if is_cur && cand.lines[i].has_word_timings() && wlines.len() == 1 {
                // Render karaoke word sweep on single-line word-timed entries
                let mut wx = (self.rx + 4) as f32;
                let wy = y as f32;
                let eff_pos =
                    ctl.adjusted_lyrics_pos_ms(pos_ms, &tr.id, ctl.hud_config().lyrics_shift_ms);
                for wt in &cand.lines[i].words {
                    let active = eff_pos >= wt.time_ms;
                    let wcol = if active {
                        th.accent
                    } else {
                        PlayerTheme::TEXT_MID
                    };
                    g.text(&wt.text, wx, wy, wcol, false);
                    wx += metrics.width(&wt.text) + metrics.width(" ");
                }
            } else {
                for (k, part) in wlines.iter().enumerate() {
                    g.text(
                        part,
                        (self.rx + 4) as f32,
                        (y + k as i32 * lh) as f32,
                        color,
                        false,
                    );
                }
            }
        }
        g.pop_scissor();
    }

    fn ensure_wrapped(
        &mut self,
        metrics: &dyn TextMetrics,
        track_id: &str,
        src_lbl: &str,
        lines: &[LyricLine],
    ) {
        let w = (self.rw - 10).max(40);
        if self.wrapped_for_id == track_id
            && self.wrapped_for_src == src_lbl
            && self.wrapped_for_count == lines.len()
            && self.wrap_w == w
        {
            return;
        }
        self.wrapped.clear();
        self.tops.clear();
        let lh = metrics.line_height().round() as i32 + 2;
        let mut y = 0;
        for line in lines {
            let parts = metrics.wrap(&line.text, w as f32);
            let h = parts.len().max(1) as i32 * lh + 8;
            self.wrapped.push(parts);
            self.tops.push(y);
            y += h;
        }
        self.total_h = y;
        self.wrapped_for_id = track_id.to_owned();
        self.wrapped_for_src = src_lbl.to_owned();
        self.wrapped_for_count = lines.len();
        self.wrap_w = w;
        self.manual_offset_px = 0.0;
        self.scroll_px = 0.0;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lyrics_control_strip(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        tr: &Track,
        pc: &PlayerConfig,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        self.strip_minus_rect = Rect::default();
        self.strip_plus_rect = Rect::default();
        self.strip_reset_rect = Rect::default();
        self.strip_mode_rect = Rect::default();
        self.strip_dsp_rect = Rect::default();
        self.strip_src_rect = Rect::default();
        self.strip_sync_rect = Rect::default();
        self.strip_own_rect = Rect::default();

        let y = self.ly_bottom + 6;
        // Shift controls
        let minus_r = Rect::new(self.rx, y, 16, 14);
        draw_pill_btn(g, metrics, minus_r, "-", false, th, mx, my);
        self.strip_minus_rect = minus_r;
        self.track_tip(minus_r, mx, my, "tip.shift_minus");

        let shift = ctl.track_shift_ms(&tr.id);
        let v = format!("{:+.1}", shift as f32 / 1000.0);
        let shift_lbl = i18n::fmt("player.shift", &[&v]);
        g.text(
            &shift_lbl,
            (self.rx + 20) as f32,
            (y + 3) as f32,
            if shift == 0 {
                PlayerTheme::TEXT_DIM
            } else {
                th.accent
            },
            false,
        );
        let shift_end = self.rx + 22 + metrics.width(&shift_lbl).round() as i32 + 4;
        let plus_r = Rect::new(shift_end, y, 16, 14);
        draw_pill_btn(g, metrics, plus_r, "+", false, th, mx, my);
        self.strip_plus_rect = plus_r;
        self.track_tip(plus_r, mx, my, "tip.shift_plus");

        let mut next_x = shift_end + 18;
        if shift != 0 {
            let reset_r = Rect::new(next_x, y, 14, 14);
            draw_pill_btn(g, metrics, reset_r, "x", false, th, mx, my);
            self.strip_reset_rect = reset_r;
            self.track_tip(reset_r, mx, my, "tip.shift_reset");
            next_x += 16;
        }

        // Rightmost: remove custom or hint own lyrics
        let own_w = 34;
        let own_r = Rect::new(self.rx + self.rw - own_w, y, own_w, 14);
        let custom = lyrics_cache::has_custom(tr);
        let own_lbl = if custom { "×LRC" } else { "+LRC" };
        draw_pill_btn(g, metrics, own_r, own_lbl, custom, th, mx, my);
        self.strip_own_rect = own_r;
        self.track_tip(own_r, mx, my, "tip.own_lrc");

        let limit_x = own_r.x - 4;

        // PRO MAX Strip: Mode, DSP Auto-Align toggle, Candidate switcher, Sync-now
        let mode_str = i18n::get(pc.lyrics_source_mode.key());
        let mode_short = metrics.trim_to_width(&mode_str, 108.0);
        let mode_w = (metrics.width(&mode_short).round() as i32 + 8).min(116);
        if next_x + mode_w <= limit_x {
            let r = Rect::new(next_x, y, mode_w, 14);
            draw_pill_btn(g, metrics, r, &mode_short, false, th, mx, my);
            self.strip_mode_rect = r;
            self.track_tip(r, mx, my, "tip.source_mode");
            next_x += mode_w + 3;
        }

        let dsp_w = 28;
        if next_x + dsp_w <= limit_x {
            let r = Rect::new(next_x, y, dsp_w, 14);
            draw_pill_btn(g, metrics, r, "DSP", pc.dsp_auto_align, th, mx, my);
            self.strip_dsp_rect = r;
            self.track_tip(r, mx, my, "tip.dsp");
            next_x += dsp_w + 3;
        }

        if let Some((lbl, idx, total, is_dsp)) = {
            let st = ctl.state.lock().unwrap();
            st.lyrics.as_ref().and_then(|lr| {
                lr.candidates.get(lr.active_idx).map(|c| {
                    (
                        c.source.label(),
                        lr.active_idx + 1,
                        lr.candidates.len(),
                        c.is_auto_dsp,
                    )
                })
            })
        } {
            let src_txt = format!("{lbl} {idx}/{total}");
            let src_short = metrics.trim_to_width(&src_txt, 88.0);
            let src_w = (metrics.width(&src_short).round() as i32 + 10).min(96);
            if next_x + src_w <= limit_x {
                let r = Rect::new(next_x, y, src_w, 14);
                draw_pill_btn(g, metrics, r, &src_short, is_dsp, th, mx, my);
                self.strip_src_rect = r;
                self.track_tip(r, mx, my, "tip.candidate");
                next_x += src_w + 3;
            }
        }

        let avail_sync = (limit_x - next_x).max(0);
        if avail_sync >= 36 {
            let sync_lbl = metrics.trim_to_width(
                &i18n::get("player.sync_line_now"),
                (avail_sync - 8).min(88) as f32,
            );
            let sync_w = (metrics.width(&sync_lbl).round() as i32 + 10).min(avail_sync);
            let r = Rect::new(next_x, y, sync_w, 14);
            draw_pill_btn(g, metrics, r, &sync_lbl, false, th, mx, my);
            self.strip_sync_rect = r;
            self.track_tip(r, mx, my, "tip.sync_now");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_progress(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        tr: Option<&Track>,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
        _dt: f32,
    ) {
        let dur = tr.map(|t| t.duration_ms.max(1)).unwrap_or(0);
        let mut pos = if tr.is_none() || ctl.is_idle() {
            0
        } else {
            ctl.position_ms()
        };
        if self.dragging_seek && dur > 0 {
            pos = (self.seek_frac * dur as f32).round() as u64;
        }
        let hover = tr.is_some()
            && !ctl.is_idle()
            && (self.dragging_seek
                || (mx >= self.rx
                    && mx < self.rx + self.rw
                    && my >= self.by - 6
                    && my < self.by + 10));
        self.seek_glow = smooth_step(self.seek_glow, hover);
        let bh = 4 + self.seek_glow.round() as i32 * 2;
        let bar_y = self.by + 2 - bh / 2;
        th.rrect(g, self.rx, bar_y, self.rw, bh, bh / 2, th.track_bg);
        let fill = if dur == 0 {
            0
        } else {
            ((self.rw as f64) * (pos as f64 / dur as f64).min(1.0)).round() as i32
        };
        if fill > 0 {
            th.rrect(g, self.rx, bar_y, fill.max(bh), bh, bh / 2, th.accent);
        }
        if tr.is_some() {
            let kx = (self.rx + fill).clamp(self.rx + 4, self.rx + self.rw - 4);
            PlayerTheme::circle(
                g,
                kx,
                self.by + 2,
                5 + self.seek_glow.round() as i32,
                th.accent,
            );
            PlayerTheme::circle(
                g,
                kx,
                self.by + 2,
                2 + self.seek_glow.round() as i32,
                PlayerTheme::TEXT_MAIN,
            );
        }
        g.text(
            fmt_ms(pos),
            self.rx as f32,
            (self.by + 10) as f32,
            PlayerTheme::TEXT_MID,
            false,
        );
        let d_str = fmt_ms(dur);
        let dw = metrics.width(&d_str);
        g.text(
            d_str,
            (self.rx + self.rw) as f32 - dw,
            (self.by + 10) as f32,
            PlayerTheme::TEXT_DIM,
            false,
        );

        if hover && dur > 0 {
            let frac = ((mx - self.rx) as f32 / self.rw.max(1) as f32).clamp(0.0, 1.0);
            let h_ms = (frac * dur as f32).round() as u64;
            let s = fmt_ms(h_ms);
            let sw_txt = metrics.width(&s).round() as i32 + 8;
            let sx = (mx - sw_txt / 2).clamp(self.rx, self.rx + self.rw - sw_txt);
            th.rrect(g, sx, self.by - 20, sw_txt, 12, 3, th.field_bg);
            g.text(
                s,
                (sx + 4) as f32,
                (self.by - 18) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_transport(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        tr: Option<&Track>,
        th: &PlayerTheme,
        mx: i32,
        my: i32,
    ) {
        let mid_x = self.rx + self.rw / 2;
        let cy = self.cy;
        let playing = ctl.is_playing();
        let (shuffle, repeat) = {
            let st = ctl.state.lock().unwrap();
            (st.shuffle, st.repeat)
        };

        // Prev / Play(big) / Next
        let prev_r = Rect::new(mid_x - 38, cy - 11, 22, 22);
        let play_r = Rect::new(mid_x - 13, cy - 13, 26, 26);
        let next_r = Rect::new(mid_x + 16, cy - 11, 22, 22);

        PlayerTheme::circle(
            g,
            prev_r.x + 11,
            cy,
            11,
            if prev_r.hit(mx, my) {
                th.hover
            } else {
                th.track_bg
            },
        );
        PlayerTheme::icon_skip(g, prev_r.x + 11, cy, 8, PlayerTheme::TEXT_MAIN, false);
        self.track_tip(prev_r, mx, my, "tip.prev");

        let play_bg = if play_r.hit(mx, my) {
            th.accent_hover
        } else {
            th.accent
        };
        PlayerTheme::circle(g, mid_x, cy, 13, play_bg);
        if playing {
            PlayerTheme::icon_pause(g, mid_x, cy, 11, PlayerTheme::TEXT_MAIN);
        } else {
            PlayerTheme::icon_play(g, mid_x, cy, 12, PlayerTheme::TEXT_MAIN);
        }
        self.track_tip(play_r, mx, my, "tip.play");

        PlayerTheme::circle(
            g,
            next_r.x + 11,
            cy,
            11,
            if next_r.hit(mx, my) {
                th.hover
            } else {
                th.track_bg
            },
        );
        PlayerTheme::icon_skip(g, next_r.x + 11, cy, 8, PlayerTheme::TEXT_MAIN, true);
        self.track_tip(next_r, mx, my, "tip.next");

        // -10s / +10s
        let back10_r = Rect::new(mid_x - 66, cy - 9, 24, 18);
        draw_pill_btn(g, metrics, back10_r, "-10", false, th, mx, my);
        self.track_tip(back10_r, mx, my, "tip.back10");

        let fwd10_r = Rect::new(mid_x + 42, cy - 9, 24, 18);
        draw_pill_btn(g, metrics, fwd10_r, "+10", false, th, mx, my);
        self.track_tip(fwd10_r, mx, my, "tip.fwd10");

        // Shuffle / Repeat / Like / Dislike
        let left_max = ((mid_x - 94) - self.rx).max(52);
        let shuf_txt = i18n::get("player.shuffle");
        let shuf_lbl = metrics.trim_to_width(&shuf_txt, (left_max - 8) as f32);
        let shuf_w = (metrics.width(&shuf_lbl).round() as i32 + 10).clamp(50, left_max);
        let shuf_r = Rect::new(self.rx, cy - 9, shuf_w, 18);
        self.shuffle_rect = shuf_r;
        draw_pill_btn(g, metrics, shuf_r, &shuf_lbl, shuffle, th, mx, my);
        self.track_tip(shuf_r, mx, my, "tip.shuffle");

        let rep_k = match repeat {
            RepeatMode::Off => "repeat.off",
            RepeatMode::All => "repeat.all",
            RepeatMode::One => "repeat.one",
        };
        let rep_val = i18n::get(rep_k);
        let rep_txt = i18n::fmt("player.repeat_short", &[&rep_val]);
        let right_max = ((self.rx + self.rw) - (mid_x + 92)).max(56);
        let rep_lbl = metrics.trim_to_width(&rep_txt, (right_max - 8) as f32);
        let rep_w = (metrics.width(&rep_lbl).round() as i32 + 10).clamp(56, right_max);
        let rep_r = Rect::new(self.rx + self.rw - rep_w, cy - 9, rep_w, 18);
        self.repeat_rect = rep_r;
        draw_pill_btn(
            g,
            metrics,
            rep_r,
            &rep_lbl,
            repeat != RepeatMode::Off,
            th,
            mx,
            my,
        );
        self.track_tip(rep_r, mx, my, "tip.repeat");

        if ctl.can_like(tr) {
            let like_r = Rect::new(mid_x - 90, cy - 9, 18, 18);
            PlayerTheme::circle(
                g,
                like_r.x + 9,
                cy,
                9,
                if like_r.hit(mx, my) {
                    th.hover
                } else {
                    th.track_bg
                },
            );
            let liked = tr.is_some_and(|t| ctl.state.lock().unwrap().liked_ids.contains(&t.id));
            PlayerTheme::icon_heart(
                g,
                like_r.x + 9 - PlayerTheme::HEART_W / 2,
                cy - PlayerTheme::HEART_H / 2,
                if liked {
                    th.accent
                } else {
                    PlayerTheme::TEXT_MID
                },
            );
            self.track_tip(like_r, mx, my, "tip.like");
        }
        if ctl.wave_shown() && tr.is_some() {
            let dis_r = Rect::new(mid_x + 70, cy - 9, 18, 18);
            self.track_tip(dis_r, mx, my, "tip.dislike");
            PlayerTheme::circle(
                g,
                dis_r.x + 9,
                cy,
                9,
                if dis_r.hit(mx, my) {
                    th.hover
                } else {
                    th.track_bg
                },
            );
            for i in 0..6 {
                g.fill(
                    (dis_r.x + 6 + i) as f32,
                    (cy - 3 + i) as f32,
                    (dis_r.x + 7 + i) as f32,
                    (cy - 2 + i) as f32,
                    PlayerTheme::TEXT_MID,
                );
                g.fill(
                    (dis_r.x + 6 + i) as f32,
                    (cy + 2 - i) as f32,
                    (dis_r.x + 7 + i) as f32,
                    (cy + 3 - i) as f32,
                    PlayerTheme::TEXT_MID,
                );
            }
        }
    }

    pub fn is_text_focused(&self) -> bool {
        (self.mode == Mode::Login && self.token_box.focused)
            || (self.mode == Mode::Main && self.search_box.focused)
    }

    pub fn mouse_down(
        &mut self,
        mx: i32,
        my: i32,
        _button: i32,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) -> ScreenRoute {
        self.init(ctl, sw, sh);

        if self.chip_rect.hit(mx, my) {
            ctl.spotify.cancel_login();
            if self.mode == Mode::Picker {
                let plat = ctl.active_platform();
                self.mode = if plat.needs_login() && !ctl.has_auth(plat) {
                    Mode::Login
                } else {
                    Mode::Main
                };
            } else {
                self.mode = Mode::Picker;
                self.built_at = Instant::now();
            }
            self.add_account_mode = false;
            return ScreenRoute::Stay;
        }
        let help_r = Rect::new(self.px + self.pw - 48, self.py + 6, 18, 18);
        if help_r.hit(mx, my) {
            return ScreenRoute::OpenHelp;
        }
        let gear_r = Rect::new(self.px + self.pw - 26, self.py + 6, 18, 18);
        if gear_r.hit(mx, my) {
            return ScreenRoute::OpenConfig(ConfigTab::Player);
        }

        match self.mode {
            Mode::Picker => {
                if self.picker_back_rect().hit(mx, my) {
                    let plat = ctl.active_platform();
                    self.mode = if plat.needs_login() && !ctl.has_auth(plat) {
                        Mode::Login
                    } else {
                        Mode::Main
                    };
                    return ScreenRoute::Stay;
                }
                for (plat, r) in self.picker_cards() {
                    if ctl.has_auth(plat)
                        && Rect::new(r.x + r.w - 52, r.y + (r.h - 16) / 2, 44, 16).hit(mx, my)
                    {
                        ctl.logout_platform(plat);
                        self.token_box.set_value("");
                        self.token_box.focused = false;
                        self.add_account_mode = false;
                        self.spotify_cid_prefilled = false;
                        return ScreenRoute::Stay;
                    }
                    if r.hit(mx, my) {
                        ctl.spotify.cancel_login();
                        self.spotify_cid_prefilled = false;
                        self.token_box.set_value("");
                        self.token_box.focused = false;
                        ctl.select_platform(plat);
                        self.set_queue_mode(ctl, false);
                        self.set_page(ctl, 0);
                        if plat.needs_login() && !ctl.has_auth(plat) {
                            self.mode = Mode::Login;
                        } else {
                            self.mode = Mode::Main;
                        }
                        self.init(ctl, sw, sh);
                        return ScreenRoute::Stay;
                    }
                }
                ScreenRoute::Stay
            }
            Mode::Login => self.mouse_down_login(mx, my, ctl, sw, sh),
            Mode::Main => self.mouse_down_main(mx, my, ctl, sw, sh),
        }
    }

    fn mouse_down_login(
        &mut self,
        mx: i32,
        my: i32,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) -> ScreenRoute {
        let plat = ctl.active_platform();
        let cx = self.px + self.pw / 2;
        let back_btn = Rect::new(self.px + 14, self.py + self.ph - 28, 72, 18);
        if back_btn.hit(mx, my) {
            ctl.spotify.cancel_login();
            {
                let mut st = ctl.state.lock().unwrap();
                st.status_msg.clear();
                st.error_msg.clear();
            }
            self.mode = if ctl.has_auth(plat) || !plat.needs_login() {
                Mode::Main
            } else {
                Mode::Picker
            };
            self.device_auth = None;
            return ScreenRoute::Stay;
        }

        self.token_box.focused = self.token_box.hit(mx, my);
        if self.token_box.focused {
            return ScreenRoute::Stay;
        }

        if plat == Platform::SoundCloud {
            if Rect::new(cx - 140, self.py + 138, 134, 22).hit(mx, my) {
                ctl.link_soundcloud(self.token_box.value.clone());
                self.mode = Mode::Main;
                self.init(ctl, sw, sh);
            } else if Rect::new(cx + 6, self.py + 138, 134, 22).hit(mx, my) {
                self.pending_side_effects.push(ScreenSideEffect::OpenUrl(
                    "https://soundcloud.com".to_owned(),
                ));
            } else if ctl.has_auth(Platform::SoundCloud)
                && Rect::new(cx - 70, self.py + 168, 140, 20).hit(mx, my)
            {
                ctl.logout_active();
            }
            return ScreenRoute::Stay;
        }

        if plat == Platform::Spotify {
            if Rect::new(cx - 150, self.py + 68, 300, 18).hit(mx, my) {
                self.pending_side_effects
                    .push(ScreenSideEffect::CopyClipboard(REDIRECT_URI.to_owned()));
            } else if Rect::new(cx - 140, self.py + 154, 134, 22).hit(mx, my) {
                ctl.login_spotify(self.token_box.value.clone());
            } else if Rect::new(cx + 6, self.py + 154, 134, 22).hit(mx, my) {
                self.pending_side_effects.push(ScreenSideEffect::OpenUrl(
                    "https://developer.spotify.com/dashboard".to_owned(),
                ));
            } else if ctl.has_auth(Platform::Spotify)
                && Rect::new(cx - 70, self.py + 184, 140, 20).hit(mx, my)
            {
                ctl.logout_active();
            }
            return ScreenRoute::Stay;
        }

        if plat == Platform::YouTube {
            if Rect::new(cx - 140, self.py + 138, 134, 22).hit(mx, my) {
                ctl.login_youtube(self.token_box.value.clone());
                self.mode = Mode::Main;
                self.init(ctl, sw, sh);
            } else if Rect::new(cx + 6, self.py + 138, 134, 22).hit(mx, my) {
                self.pending_side_effects.push(ScreenSideEffect::OpenUrl(
                    "https://music.youtube.com".to_owned(),
                ));
            } else if ctl.has_auth(Platform::YouTube)
                && Rect::new(cx - 70, self.py + 168, 140, 20).hit(mx, my)
            {
                ctl.logout_active();
            }
            return ScreenRoute::Stay;
        }

        // Yandex login
        let saved = ctl.saved_yandex_accounts();
        if !self.add_account_mode && self.device_auth.is_none() && !saved.is_empty() {
            let shown = saved.len().min(MAX_ACCOUNT_ROWS);
            for (i, acc) in saved.iter().take(shown).enumerate() {
                let ry = self.py + 68 + i as i32 * 26;
                if Rect::new(cx - 120, ry, 212, 22).hit(mx, my) {
                    ctl.login_yandex_saved(acc.label.clone(), acc.token.clone());
                    self.mode = Mode::Main;
                    self.init(ctl, sw, sh);
                    return ScreenRoute::Stay;
                }
                if Rect::new(cx + 96, ry, 24, 22).hit(mx, my) {
                    ctl.forget_yandex_account(&acc.token);
                    return ScreenRoute::Stay;
                }
            }
            if Rect::new(cx - 100, self.py + 72 + shown as i32 * 26, 200, 22).hit(mx, my) {
                self.add_account_mode = true;
            }
            return ScreenRoute::Stay;
        }

        if let Some(ref code) = self.device_auth {
            if Rect::new(cx - 150, self.py + 158, 144, 20).hit(mx, my) {
                self.pending_side_effects
                    .push(ScreenSideEffect::OpenUrl(code.verification_url.clone()));
            } else if Rect::new(cx + 6, self.py + 158, 144, 20).hit(mx, my) {
                self.pending_side_effects
                    .push(ScreenSideEffect::CopyClipboard(code.user_code.clone()));
            } else if Rect::new(cx - 60, self.py + 184, 120, 20).hit(mx, my) {
                self.device_auth = None;
            }
            return ScreenRoute::Stay;
        }

        if Rect::new(cx - 115, self.py + 70, 230, 22).hit(mx, my) {
            match ctl.start_yandex_device_auth() {
                Ok(sess) => {
                    self.pending_side_effects
                        .push(ScreenSideEffect::CopyClipboard(sess.user_code.clone()));
                    self.pending_side_effects
                        .push(ScreenSideEffect::OpenUrl(sess.verification_url.clone()));
                    self.device_auth = Some(PendingDeviceAuth {
                        device_code: sess.device_code,
                        user_code: sess.user_code,
                        verification_url: sess.verification_url,
                        expires_at: Instant::now() + Duration::from_secs(sess.expires_in_sec),
                        next_poll: Instant::now() + Duration::from_secs(sess.interval_sec),
                        poll_interval_sec: sess.interval_sec,
                    });
                }
                Err(e) => ctl.state.lock().unwrap().error_msg = e,
            }
            return ScreenRoute::Stay;
        }
        if Rect::new(cx - 140, self.py + 146, 134, 22).hit(mx, my) {
            ctl.submit_yandex_token(self.token_box.value.clone());
            self.mode = Mode::Main;
            self.add_account_mode = false;
            self.init(ctl, sw, sh);
        } else if Rect::new(cx + 6, self.py + 146, 134, 22).hit(mx, my) {
            self.pending_side_effects.push(ScreenSideEffect::OpenUrl(
                "https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d".to_owned(),
            ));
        } else if !saved.is_empty() && Rect::new(cx - 90, self.py + 176, 180, 20).hit(mx, my) {
            self.add_account_mode = false;
        }
        ScreenRoute::Stay
    }

    fn mouse_down_main(
        &mut self,
        mx: i32,
        my: i32,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) -> ScreenRoute {
        // Header Logout button (Yandex / SoundCloud / YouTube / Spotify)
        let plat_hdr = ctl.active_platform();
        if ctl.has_auth(plat_hdr)
            && Rect::new(self.px + self.pw - 196, self.py + 6, 56, 18).hit(mx, my)
        {
            ctl.logout_active();
            self.spotify_cid_prefilled = false;
            self.add_account_mode = false;
            self.token_box.set_value("");
            self.token_box.focused = false;
            self.mode = Mode::Login;
            self.init(ctl, sw, sh);
            return ScreenRoute::Stay;
        }

        // Queue button
        if Rect::new(self.px + self.pw - 134, self.py + 6, 78, 18).hit(mx, my) {
            let next_q = !self.queue_mode;
            self.set_queue_mode(ctl, next_q);
            self.set_page(ctl, 0);
            return ScreenRoute::Stay;
        }

        // Search input + Find
        self.search_box.focused = self.search_box.hit(mx, my);
        if self.search_box.focused {
            return ScreenRoute::Stay;
        }
        if Rect::new(self.lx + self.lw - 52, self.py + 30, 52, 18).hit(mx, my) {
            self.set_queue_mode(ctl, false);
            self.set_page(ctl, 0);
            ctl.search(&self.search_box.value);
            return ScreenRoute::Stay;
        }

        // Left sub-bar actions
        let plat = ctl.active_platform();
        let tab_y = self.py + 52;
        if plat == Platform::Local {
            let bw = (self.lw - 8) / 3;
            if Rect::new(self.lx, tab_y, bw, 16).hit(mx, my) {
                ctl.open_local_folder();
                return ScreenRoute::Stay;
            }
            if Rect::new(self.lx + bw + 4, tab_y, bw, 16).hit(mx, my) {
                self.set_queue_mode(ctl, false);
                self.set_page(ctl, 0);
                ctl.search(&self.search_box.value);
                return ScreenRoute::Stay;
            }
            if Rect::new(self.lx + (bw + 4) * 2, tab_y, self.lw - (bw + 4) * 2, 16).hit(mx, my) {
                self.set_queue_mode(ctl, false);
                self.set_page(ctl, 0);
                ctl.load_liked();
                return ScreenRoute::Stay;
            }
        } else if plat == Platform::Yandex {
            let (wave_r, liked_r, search_r) = self.yandex_tab_rects(tab_y);
            if wave_r.hit(mx, my) {
                self.set_queue_mode(ctl, false);
                if ctl.wave_shown() {
                    ctl.stop_wave();
                } else {
                    ctl.start_wave("user:onyourwave", &i18n::get("player.wave"));
                }
                return ScreenRoute::Stay;
            }
            if liked_r.hit(mx, my) {
                self.set_queue_mode(ctl, false);
                self.set_page(ctl, 0);
                ctl.select_tab(Tab::Liked);
                return ScreenRoute::Stay;
            }
            if search_r.hit(mx, my) {
                self.set_queue_mode(ctl, false);
                self.set_page(ctl, 0);
                ctl.select_tab(Tab::Search);
                return ScreenRoute::Stay;
            }
        } else if plat == Platform::YouTube {
            if ctl.has_auth(Platform::YouTube) {
                let bw = (self.lw - 4) / 2;
                if Rect::new(self.lx, tab_y, bw, 16).hit(mx, my) {
                    self.set_queue_mode(ctl, false);
                    self.set_page(ctl, 0);
                    ctl.load_liked();
                    return ScreenRoute::Stay;
                }
                if Rect::new(self.lx + bw + 4, tab_y, self.lw - bw - 4, 16).hit(mx, my) {
                    self.set_queue_mode(ctl, false);
                    self.set_page(ctl, 0);
                    ctl.load_youtube_charts();
                    return ScreenRoute::Stay;
                }
            } else {
                let bw = (self.lw - 8) / 3;
                if Rect::new(self.lx, tab_y, bw, 16).hit(mx, my) {
                    self.set_queue_mode(ctl, false);
                    self.set_page(ctl, 0);
                    ctl.load_liked();
                    return ScreenRoute::Stay;
                }
                if Rect::new(self.lx + bw + 4, tab_y, bw, 16).hit(mx, my) {
                    self.set_queue_mode(ctl, false);
                    self.set_page(ctl, 0);
                    ctl.load_youtube_charts();
                    return ScreenRoute::Stay;
                }
                if Rect::new(self.lx + (bw + 4) * 2, tab_y, self.lw - (bw + 4) * 2, 16).hit(mx, my)
                {
                    self.token_box.set_value("");
                    self.token_box.focused = false;
                    self.mode = Mode::Login;
                    return ScreenRoute::Stay;
                }
            }
        } else {
            let bw = (self.lw - 4) / 2;
            if Rect::new(self.lx, tab_y, bw, 16).hit(mx, my) {
                self.set_queue_mode(ctl, false);
                self.set_page(ctl, 0);
                ctl.load_liked();
                return ScreenRoute::Stay;
            }
            if Rect::new(self.lx + bw + 4, tab_y, self.lw - bw - 4, 16).hit(mx, my) {
                if ctl.has_auth(plat) {
                    ctl.logout_active();
                    self.mode = Mode::Login;
                    self.init(ctl, sw, sh);
                } else {
                    self.mode = Mode::Login;
                }
                return ScreenRoute::Stay;
            }
        }

        // Page header (< Back / Play All)
        if let Some(pg) = ctl.current_page() {
            let hdr_y = self.list_top - 19;
            if Rect::new(self.lx, hdr_y, 52, 15).hit(mx, my) {
                ctl.pop_page();
                self.set_page(ctl, 0);
                self.init(ctl, sw, sh);
                return ScreenRoute::Stay;
            }
            if !pg.tracks.is_empty() && Rect::new(self.lx + self.lw - 74, hdr_y, 74, 15).hit(mx, my)
            {
                ctl.play_from(pg.tracks, 0);
                return ScreenRoute::Stay;
            }
        }

        // Pager
        let max_p = self.max_page(ctl);
        let (prev_r, next_r, _) = self.pager_rects();
        if prev_r.hit(mx, my) {
            self.set_page(ctl, self.page.saturating_sub(1));
            return ScreenRoute::Stay;
        }
        if next_r.hit(mx, my) {
            if self.page < max_p {
                self.set_page(ctl, self.page + 1);
            }
            return ScreenRoute::Stay;
        }

        // Volume mute + bar
        if self.mute_rect().hit(mx, my) {
            ctl.toggle_mute();
            return ScreenRoute::Stay;
        }
        if mx >= self.vol_bar_x - 3
            && mx <= self.vol_bar_x + VOL_BAR_W + 3
            && my >= self.cy - 7
            && my <= self.cy + 7
        {
            self.dragging_vol = true;
            ctl.set_volume(((mx - self.vol_bar_x) as f32 / VOL_BAR_W as f32).clamp(0.0, 1.0));
            return ScreenRoute::Stay;
        }

        // List rows
        let (tracks, entities, rows) = self.collect_items(ctl);
        for i in 0..self.rows_per_page {
            let idx = self.page * self.rows_per_page + i;
            let Some(&kind) = rows.get(idx) else {
                break;
            };
            let y = self.list_top + i as i32 * ROW_H;
            if !Rect::new(self.lx, y, self.lw, ROW_H - 2).hit(mx, my) {
                continue;
            }
            match kind {
                RowKind::Track(ti) => {
                    let r1 = Rect::new(
                        self.lx + self.lw - ACT_W * 2 - 2,
                        y + 3,
                        ACT_W - 2,
                        ROW_H - 8,
                    );
                    let r2 = Rect::new(self.lx + self.lw - ACT_W, y + 3, ACT_W - 2, ROW_H - 8);
                    if self.queue_mode {
                        if r1.hit(mx, my) {
                            ctl.queue_move_up(ti);
                        } else if r2.hit(mx, my) {
                            ctl.remove_from_queue(ti);
                        } else {
                            ctl.play_queue_index(ti);
                        }
                    } else if let Some(t) = tracks.get(ti) {
                        if r1.hit(mx, my) {
                            ctl.add_to_user_queue(t.clone());
                        } else if r2.hit(mx, my) {
                            ctl.play_next_track(t.clone());
                        } else if ctl.wave_shown() && ctl.current_page().is_none() {
                            ctl.play_wave_index(ti);
                        } else {
                            ctl.play_from(tracks, ti);
                        }
                    }
                }
                RowKind::Entity(ei) => {
                    if let Some(ent) = entities.get(ei) {
                        if ent.kind == EntityKind::Station {
                            self.set_queue_mode(ctl, false);
                            ctl.start_wave(&ent.id, &ent.title);
                        } else {
                            ctl.open_entity(ent.clone());
                        }
                        self.set_page(ctl, 0);
                        self.init(ctl, sw, sh);
                    }
                }
            }
            return ScreenRoute::Stay;
        }

        // Artist / Album clickable metadata
        if let Some(ref tr) = ctl.current_track() {
            if self.artist_rect.hit(mx, my) {
                self.set_queue_mode(ctl, false);
                ctl.open_artist_of(tr);
                self.set_page(ctl, 0);
                return ScreenRoute::Stay;
            }
            if self.album_rect.hit(mx, my) {
                self.set_queue_mode(ctl, false);
                ctl.open_album_of(tr);
                self.set_page(ctl, 0);
                return ScreenRoute::Stay;
            }

            // Lyrics click-to-seek when synced
            if mx >= self.rx
                && mx < self.rx + self.rw
                && my >= self.ly_top
                && my < self.ly_bottom
                && let Some(cand) = ctl.active_lyrics()
                && cand.lines.iter().any(LyricLine::is_synced)
            {
                let view_h = self.ly_bottom - self.ly_top;
                let origin = self.ly_top + view_h / 2 - self.scroll_px.round() as i32;
                let click_off = my - origin;
                for (i, top) in self.tops.iter().enumerate() {
                    let bot = self.tops.get(i + 1).copied().unwrap_or(self.total_h);
                    if click_off >= *top && click_off < bot {
                        let shift =
                            ctl.track_shift_ms(&tr.id) + ctl.hud_config().lyrics_shift_ms as i64;
                        let target_ms = (cand.lines[i].time_ms as i64 + shift).max(0) as u64;
                        ctl.seek_to(target_ms);
                        self.manual_offset_px = 0.0;
                        return ScreenRoute::Stay;
                    }
                }
            }

            // Lyrics control strip clicks
            if ctl.player_config().show_lyrics
                && self.ly_bottom - self.ly_top > 28
                && self.handle_lyrics_strip_click(mx, my, ctl, tr)
            {
                return ScreenRoute::Stay;
            }

            // Progress / Seek bar
            if !ctl.is_idle()
                && mx >= self.rx - 3
                && mx <= self.rx + self.rw + 3
                && my >= self.by - 8
                && my <= self.by + 12
            {
                self.dragging_seek = true;
                self.seek_frac = ((mx - self.rx) as f32 / self.rw.max(1) as f32).clamp(0.0, 1.0);
                return ScreenRoute::Stay;
            }

            // Transport buttons
            let mid_x = self.rx + self.rw / 2;
            let cy = self.cy;
            if Rect::new(mid_x - 38, cy - 11, 22, 22).hit(mx, my) {
                ctl.prev();
            } else if Rect::new(mid_x - 13, cy - 13, 26, 26).hit(mx, my) {
                ctl.toggle_pause();
            } else if Rect::new(mid_x + 16, cy - 11, 22, 22).hit(mx, my) {
                ctl.next();
            } else if Rect::new(mid_x - 66, cy - 9, 24, 18).hit(mx, my) {
                ctl.seek_by(-10_000);
            } else if Rect::new(mid_x + 42, cy - 9, 24, 18).hit(mx, my) {
                ctl.seek_by(10_000);
            } else if self.shuffle_rect.hit(mx, my) {
                ctl.toggle_shuffle();
            } else if self.repeat_rect.hit(mx, my) {
                ctl.cycle_repeat();
            } else if ctl.can_like(Some(tr)) && Rect::new(mid_x - 90, cy - 9, 18, 18).hit(mx, my) {
                ctl.toggle_like(tr.clone());
            } else if ctl.wave_shown() && Rect::new(mid_x + 70, cy - 9, 18, 18).hit(mx, my) {
                ctl.wave_dislike();
            }
        }

        ScreenRoute::Stay
    }

    fn handle_lyrics_strip_click(
        &mut self,
        mx: i32,
        my: i32,
        ctl: &PlayerController,
        tr: &Track,
    ) -> bool {
        if self.strip_minus_rect.hit(mx, my) {
            ctl.add_track_shift_ms(&tr.id, -100);
            return true;
        }
        if self.strip_plus_rect.hit(mx, my) {
            ctl.add_track_shift_ms(&tr.id, 100);
            return true;
        }
        if self.strip_reset_rect.hit(mx, my) {
            ctl.reset_track_shift_ms(&tr.id);
            return true;
        }
        if self.strip_mode_rect.hit(mx, my) {
            ctl.cycle_lyrics_source_mode();
            return true;
        }
        if self.strip_dsp_rect.hit(mx, my) {
            ctl.toggle_dsp_auto_align();
            return true;
        }
        if self.strip_src_rect.hit(mx, my) {
            ctl.next_lyric_candidate();
            return true;
        }
        if self.strip_sync_rect.hit(mx, my) {
            ctl.sync_current_lyric_to_now();
            return true;
        }
        if self.strip_own_rect.hit(mx, my) {
            if lyrics_cache::has_custom(tr) {
                ctl.remove_custom_lyrics();
            } else {
                ctl.state.lock().unwrap().status_msg = i18n::get("status.lyrics_drop_hint");
            }
            return true;
        }

        false
    }

    pub fn mouse_drag(&mut self, mx: i32, _my: i32, ctl: &PlayerController) {
        if self.dragging_vol {
            ctl.set_volume(((mx - self.vol_bar_x) as f32 / VOL_BAR_W as f32).clamp(0.0, 1.0));
        }
        if self.dragging_seek {
            self.seek_frac = ((mx - self.rx) as f32 / self.rw.max(1) as f32).clamp(0.0, 1.0);
        }
    }

    pub fn mouse_up(&mut self, _mx: i32, _my: i32, _button: i32, ctl: &PlayerController) {
        self.dragging_vol = false;
        if self.dragging_seek {
            self.dragging_seek = false;
            if let Some(tr) = ctl.current_track() {
                let target = (self.seek_frac * tr.duration_ms.max(1) as f32).round() as u64;
                ctl.seek_to(target);
            }
        }
    }

    pub fn mouse_scroll(&mut self, mx: i32, my: i32, sy: f32, ctl: &PlayerController) {
        if self.mode != Mode::Main {
            return;
        }
        if mx >= self.lx
            && mx < self.lx + self.lw
            && my >= self.list_top
            && my < self.py + self.ph - 48
        {
            let max_p = self.max_page(ctl);
            if sy < 0.0 && self.page < max_p {
                self.set_page(ctl, self.page + 1);
            } else if sy > 0.0 && self.page > 0 {
                self.set_page(ctl, self.page - 1);
            }
        } else if mx >= self.rx
            && mx < self.rx + self.rw
            && my >= self.ly_top
            && my < self.ly_bottom
        {
            self.manual_offset_px = (self.manual_offset_px - sy * 20.0).clamp(-300.0, 300.0);
        }
    }

    pub fn key_pressed(
        &mut self,
        key: ScreenKey,
        _raw_key: &str,
        ctrl: bool,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
    ) -> (bool, ScreenRoute) {
        if key == ScreenKey::Escape {
            if self.mode == Mode::Picker {
                self.mode = Mode::Main;
                return (true, ScreenRoute::Stay);
            }
            return (true, ScreenRoute::CloseAll);
        }

        if self.mode == Mode::Login && self.token_box.focused {
            if key == ScreenKey::Enter {
                let val = self.token_box.value.clone();
                match ctl.active_platform() {
                    Platform::Yandex => {
                        ctl.submit_yandex_token(val);
                        self.mode = Mode::Main;
                        self.add_account_mode = false;
                    }
                    Platform::SoundCloud => {
                        ctl.link_soundcloud(val);
                        self.mode = Mode::Main;
                    }
                    Platform::Spotify => ctl.login_spotify(val),
                    Platform::YouTube => {
                        ctl.login_youtube(val);
                        self.mode = Mode::Main;
                    }
                    Platform::Local => {}
                }
                self.init(ctl, sw, sh);
                return (true, ScreenRoute::Stay);
            }
            if self.token_box.handle_key(key, ctrl) {
                return (true, ScreenRoute::Stay);
            }
        }

        if self.mode == Mode::Main && self.search_box.focused {
            if key == ScreenKey::Enter {
                self.queue_mode = false;
                self.page = 0;
                ctl.search(&self.search_box.value);
                return (true, ScreenRoute::Stay);
            }
            if self.search_box.handle_key(key, ctrl) {
                return (true, ScreenRoute::Stay);
            }
        }

        if key == ScreenKey::Space && !self.search_box.focused && !self.token_box.focused {
            ctl.toggle_pause();
            return (true, ScreenRoute::Stay);
        }

        (false, ScreenRoute::Stay)
    }

    pub fn char_typed(&mut self, ch: char) -> bool {
        if self.mode == Mode::Login {
            self.token_box.insert_char(ch)
        } else if self.mode == Mode::Main {
            self.search_box.insert_char(ch)
        } else {
            false
        }
    }

    pub fn paste_text(&mut self, txt: &str) -> bool {
        if self.mode == Mode::Login {
            self.token_box.insert_str(txt)
        } else if self.mode == Mode::Main {
            self.search_box.insert_str(txt)
        } else {
            false
        }
    }

    pub fn on_files_drop(&mut self, paths: &[PathBuf], ctl: &PlayerController) -> bool {
        if paths.is_empty() {
            return false;
        }
        let all_lyrics = paths.iter().all(|p| {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            matches!(ext.as_str(), "lrc" | "ttml" | "xml" | "txt")
        });
        if all_lyrics && ctl.current_track().is_some() {
            match ctl.import_lyrics_file(&paths[0]) {
                Ok(msg) => ctl.state.lock().unwrap().status_msg = msg,
                Err(e) => ctl.state.lock().unwrap().error_msg = e,
            }
            return true;
        }
        let added = ctl.add_local_paths(paths);
        if added > 0 {
            self.mode = Mode::Main;
            self.add_account_mode = false;
            self.set_queue_mode(ctl, false);
            self.set_page(ctl, 0);
            return true;
        }
        false
    }
}

fn tab_key(t: Tab) -> &'static str {
    match t {
        Tab::Search => "player.albums_history",
        Tab::Liked => "player.likes",
        Tab::Wave => "player.wave",
    }
}

fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn smooth_step(cur: f32, target: bool) -> f32 {
    let dst = if target { 1.0 } else { 0.0 };
    cur + (dst - cur) * 0.25
}

fn wave_palette_at(t: f64) -> u32 {
    let n = WAVE_PALETTE.len();
    let nf = n as f64;
    let wrapped = t - (t / nf).floor() * nf;
    let i = (wrapped as usize) % n;
    let next = (i + 1) % n;
    let frac = (wrapped - i as f64) as f32;
    PlayerTheme::mix(WAVE_PALETTE[i], WAVE_PALETTE[next], frac)
}

fn draw_equalizer(g: &mut DrawList, x: i32, y: i32, h: i32, playing: bool, secs: f64, accent: u32) {
    for i in 0..3 {
        let k = if playing {
            0.5 + 0.5 * (secs * (6.0 + i as f64 * 2.1) + i as f64 * 1.9).sin()
        } else {
            0.2
        };
        let bh = 2 + (k * (h - 2).max(1) as f64).round() as i32;
        g.fill(
            (x + i * 3) as f32,
            (y + h - bh) as f32,
            (x + i * 3 + 2) as f32,
            (y + h) as f32,
            accent,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_pill_btn(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    r: Rect,
    label: &str,
    active: bool,
    th: &PlayerTheme,
    mx: i32,
    my: i32,
) {
    let hov = r.hit(mx, my);
    let bg = if active {
        if hov { th.accent_hover } else { th.accent }
    } else if hov {
        th.hover
    } else {
        th.track_bg
    };
    let rad = (th.radius + 1).min(r.h / 2);
    shapes::fill(g, r.x, r.y, r.w, r.h, rad, bg);
    let fit = metrics.trim_to_width(label, (r.w - 4).max(4) as f32);
    let tw = metrics.width(&fit);
    g.text(
        fit,
        r.x as f32 + (r.w as f32 - tw) / 2.0,
        r.y as f32 + (r.h as f32 - metrics.line_height()) / 2.0,
        if active || hov {
            PlayerTheme::TEXT_MAIN
        } else {
            PlayerTheme::TEXT_MID
        },
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    #[test]
    fn pager_buttons_do_not_overlap_mute_and_page_persists_across_reopen() {
        let ctl = PlayerController::new(false);
        ctl.select_platform(Platform::Local);
        {
            let mut st = ctl.state.lock().unwrap();
            st.liked = (0..45)
                .map(|i| Track {
                    id: format!("id-{i}"),
                    title: format!("Track {i}"),
                    artist: "Artist".to_owned(),
                    duration_ms: 120_000,
                    provider: "local".to_owned(),
                    ..Default::default()
                })
                .collect();
        }

        let mut scr = PlayerScreenState::new();
        scr.init(&ctl, 1280, 720);
        let (prev_r, next_r, _) = scr.pager_rects();
        let mute_r = scr.mute_rect();
        assert!(
            prev_r.y + prev_r.h <= mute_r.y,
            "prev button {:?} overlaps mute button {:?}",
            prev_r,
            mute_r
        );
        assert!(
            next_r.y + next_r.h <= mute_r.y,
            "next button {:?} overlaps mute button {:?}",
            next_r,
            mute_r
        );

        // Advance two pages using the > button
        scr.mouse_down(next_r.x + 2, next_r.y + 2, 0, &ctl, 1280, 720);
        scr.mouse_down(next_r.x + 2, next_r.y + 2, 0, &ctl, 1280, 720);
        assert_eq!(scr.page, 2);

        // Reopen player (simulating closing & reopening with M)
        let mut reopened = PlayerScreenState::new();
        reopened.init(&ctl, 1280, 720);
        assert_eq!(reopened.page, 2);

        // Switching tab resets page cleanly back to 0
        ctl.select_tab(Tab::Search);
        reopened.init(&ctl, 1280, 720);
        assert_eq!(reopened.page, 0);
    }

    #[test]
    fn spotify_login_keeps_cleared_input_and_allows_switching_platform() {
        let ctl = PlayerController::new(false);
        ctl.spotify.logout();
        ctl.spotify
            .set_client_id("3bc21d91572d488a9e9b337fb7fa7415");
        ctl.select_platform(Platform::Spotify);

        let mut scr = PlayerScreenState::new();
        let covers = crate::ui::cover::CoverArtManager::new();
        let metrics = crate::ui::MonoMetrics;
        let mut dl = DrawList::default();
        scr.render(&mut dl, &metrics, &ctl, &covers, 1280, 720, 0, 0, 0.016);
        assert_eq!(scr.mode, Mode::Login);
        assert_eq!(scr.token_box.value, "3bc21d91572d488a9e9b337fb7fa7415");

        // Clearing the Client ID box stays empty even after init/render runs again
        scr.token_box.set_value("");
        scr.render(&mut dl, &metrics, &ctl, &covers, 1280, 720, 0, 0, 0.016);
        assert_eq!(scr.token_box.value, "");

        // Clicking top-left platform chip switches to Picker and persists across render
        let chip = scr.chip_rect;
        scr.mouse_down(chip.x + 4, chip.y + 4, 0, &ctl, 1280, 720);
        assert_eq!(scr.mode, Mode::Picker);
        scr.render(&mut dl, &metrics, &ctl, &covers, 1280, 720, 0, 0, 0.016);
        assert_eq!(scr.mode, Mode::Picker);
    }

    #[test]
    fn logout_clears_auth_for_all_account_platforms() {
        let ctl = PlayerController::new(false);
        {
            let mut ts = ctl.tokens.lock().unwrap();
            ts.yandex_token = "y0_tok".to_owned();
            ts.yandex_accounts = vec![crate::model::Account {
                label: "ya".to_owned(),
                token: "y0_tok".to_owned(),
            }];
            ts.soundcloud_user = "123".to_owned();
            ts.soundcloud_oauth = "sc-oauth-token-123456".to_owned();
            ts.spotify_access_token = "sp-tok".to_owned();
            ts.spotify_user = "sp".to_owned();
            ts.youtube_cookie = "SAPISID=abc; SID=def".to_owned();
            ts.youtube_user = "yt".to_owned();
        }
        for plat in [
            Platform::Yandex,
            Platform::SoundCloud,
            Platform::Spotify,
            Platform::YouTube,
        ] {
            assert!(ctl.has_auth(plat), "{plat:?} should start signed in");
            ctl.select_platform(plat);
            ctl.logout_active();
            assert!(
                !ctl.has_auth(plat),
                "{plat:?} should be signed out after logout"
            );
        }
    }
}
