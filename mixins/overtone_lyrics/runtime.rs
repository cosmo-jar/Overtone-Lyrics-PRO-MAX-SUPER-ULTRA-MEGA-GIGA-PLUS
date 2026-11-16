// Horizon 26.3 runtime glue for `Overtone: Lyrics PRO MAX SUPER ULTRA MEGA GIGA PLUS` (`overtone_lyrics`).
// Connects Horizon's native hook points to the pure-Rust `overtone_lyrics_engine` crate.

use crate::game::{native_hooks::NativeMod, native_world_draw::NativeWorldDraw, Game};
use horizon_client::{
    clipboard::{Clipboard, SystemClipboard},
    extras_hud::Corners,
    hud::{rgba, HudBatch, MOD_IMAGE_SPRITE},
    mod_api::NativeScreenView,
    music::Music,
    text::Text,
};
use overtone_lyrics_engine::{
    cinema::CinematicController,
    config,
    controller::{self, PlayerController},
    i18n::{self, Language},
    local,
    model::Platform,
    ui::{
        cover::{CoverArtManager, ModImageAtlas},
        hud::HudState,
        screens::{config::ConfigTab, ScreenKey, ScreenSideEffect, ScreenStack},
        shapes,
        world_lyrics::{WorldBlockProbe, WorldLyricsState},
        DrawCmd, DrawList, PlayerTheme, TexSlot, TextMetrics,
    },
};
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use winit::event::KeyEvent;

pub const ID: &str = "overtone_lyrics";
pub const NAME: &str = "Overtone: Lyrics PRO MAX SUPER ULTRA MEGA GIGA PLUS";
pub const VERSION: &str = "1.0.0";
pub const AUTHORS: &str = "cosmo-jar, chifir4ik, PabJlo";
pub const VOLUME_STEP: f32 = 0.05;

/// Vanilla-compatible key ids mirroring `Hotkeys.java`.
pub const KEY_SPECS: [(&str, &str, &str); 10] = [
    ("key.musicplayer.open", "Open Player", "key.keyboard.m"),
    (
        "key.musicplayer.toggle_hud",
        "Toggle HUD",
        "key.keyboard.unknown",
    ),
    (
        "key.musicplayer.hud_settings",
        "HUD Settings",
        "key.keyboard.k",
    ),
    (
        "key.musicplayer.prev",
        "Previous Track",
        "key.keyboard.left.bracket",
    ),
    (
        "key.musicplayer.next",
        "Next Track",
        "key.keyboard.right.bracket",
    ),
    (
        "key.musicplayer.pause",
        "Play / Pause",
        "key.keyboard.semicolon",
    ),
    ("key.musicplayer.vol_up", "Volume Up", "key.keyboard.equal"),
    (
        "key.musicplayer.vol_down",
        "Volume Down",
        "key.keyboard.minus",
    ),
    (
        "key.musicplayer.world_lyrics",
        "Toggle World Lyrics",
        "key.keyboard.j",
    ),
    (
        "key.musicplayer.cinematic",
        "Cinematic Camera",
        "key.keyboard.o",
    ),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingScreen {
    None,
    Player,
    Config(ConfigTab),
    Edit,
    Help,
}

struct Runtime {
    initialized: bool,
    ctl: PlayerController,
    covers: CoverArtManager,
    atlas: ModImageAtlas,
    hud: HudState,
    world: WorldLyricsState,
    cinema: CinematicController,
    screens: ScreenStack,
    clipboard: SystemClipboard,
    pending_close: bool,
    pending_open: PendingScreen,
    overlay_host_id: Option<String>,
    last_vanilla_screen: Option<String>,
    swallow_char_until: Option<std::time::Instant>,
    modifiers: u32,
    tab_down: bool,
    mouse_btn: i32,
    last_lang: String,
    last_cover_offer_id: String,
    last_player_pos: Option<[f64; 3]>,
    saved_camera_type: Option<super::perspective::CameraType>,
}

static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

fn runtime() -> std::sync::MutexGuard<'static, Runtime> {
    let lock = RUNTIME.get_or_init(|| {
        Mutex::new(Runtime {
            initialized: false,
            ctl: PlayerController::new(false),
            covers: CoverArtManager::new(),
            atlas: ModImageAtlas::new(),
            hud: HudState::new(),
            world: WorldLyricsState::new(),
            cinema: CinematicController::new(),
            screens: ScreenStack::new(),
            clipboard: SystemClipboard::default(),
            pending_close: false,
            pending_open: PendingScreen::None,
            overlay_host_id: None,
            last_vanilla_screen: None,
            swallow_char_until: None,
            modifiers: 0,
            tab_down: false,
            mouse_btn: 0,
            last_lang: String::new(),
            last_cover_offer_id: String::new(),
            last_player_pos: None,
            saved_camera_type: None,
        })
    });
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

fn sync_cinema_camera_type(game: &mut Game<'_>, r: &mut Runtime) {
    if r.cinema.active() {
        if game.camera_type.first_person() {
            if r.saved_camera_type.is_none() {
                r.saved_camera_type = Some(game.camera_type);
            }
            game.camera_type = super::perspective::CameraType::ThirdPersonBack;
        }
    } else if let Some(prev) = r.saved_camera_type.take() {
        game.camera_type = prev;
    }
}

struct HorizonMetrics<'a, 'g> {
    game: &'a Game<'g>,
}

impl TextMetrics for HorizonMetrics<'_, '_> {
    fn width(&self, text: &str) -> f32 {
        self.game.native_text_width(text)
    }

    fn line_height(&self) -> f32 {
        9.0
    }
}

struct HorizonBlockProbe<'a, 'g> {
    game: &'a Game<'g>,
}

impl WorldBlockProbe for HorizonBlockProbe<'_, '_> {
    fn ground_below(&self, x: f64, y: f64, z: f64) -> f64 {
        let bx = x.floor() as i32;
        let bz = z.floor() as i32;
        let top_y = y.floor() as i32;
        let fx = x - bx as f64;
        let fz = z - bz as f64;
        for by in (top_y - 6..=top_y).rev() {
            let mut best = f64::NEG_INFINITY;
            for b in self.game.native_block_collision([bx, by, bz]) {
                if fx >= b[0] - 0.05 && fx <= b[3] + 0.05 && fz >= b[2] - 0.05 && fz <= b[5] + 0.05
                {
                    let top = by as f64 + b[4];
                    if top <= y + 0.15 {
                        best = best.max(top);
                    }
                }
            }
            if best.is_finite() {
                return best;
            }
        }
        -1e9
    }

    fn is_translucent_occluded(&self, cam: [f64; 3], target: [f64; 3]) -> bool {
        let dx = target[0] - cam[0];
        let dy = target[1] - cam[1];
        let dz = target[2] - cam[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 0.35 {
            return false;
        }
        let steps = (len * 3.0).clamp(4.0, 48.0) as i32;
        for s in 1..steps {
            let t = s as f64 / steps as f64;
            let px = cam[0] + dx * t;
            let py = cam[1] + dy * t;
            let pz = cam[2] + dz * t;
            let bx = px.floor() as i32;
            let by = py.floor() as i32;
            let bz = pz.floor() as i32;
            let fx = px - bx as f64;
            let fy = py - by as f64;
            let fz = pz - bz as f64;
            for box_aabb in self.game.native_block_collision([bx, by, bz]) {
                if fx >= box_aabb[0]
                    && fx <= box_aabb[3]
                    && fy >= box_aabb[1]
                    && fy <= box_aabb[4]
                    && fz >= box_aabb[2]
                    && fz <= box_aabb[5]
                {
                    return true;
                }
            }
        }
        false
    }
}

fn ensure_init(rt: &mut Runtime, lang_code: &str) {
    horizon_client::gfx::native_render::set_mods_built_in(true);
    if rt.initialized {
        if rt.last_lang != lang_code {
            rt.last_lang = lang_code.to_owned();
            i18n::set_language(Language::from_code(lang_code));
        }
        return;
    }
    if let Ok(dir) = Game::native_config_dir() {
        config::set_data_dir(dir.join("overtone"));
    }
    i18n::set_language(Language::from_code(lang_code));
    rt.last_lang = lang_code.to_owned();
    rt.ctl = PlayerController::new(true);
    rt.initialized = true;
}

pub(super) fn on_mods_list(mods: &mut Vec<NativeMod>) {
    mods.push(NativeMod {
        id: ID.to_owned(),
        title: format!("{NAME} {VERSION}"),
        description: format!(
            "Authors: {AUTHORS} — Strict 1:1 port of Overtone (Fabric 26.3) with multi-source synchronized lyrics cascade (LRCLIB, AMLL, NetEase, QQ, KuGou), lightweight DSP aligner, 6 HUD styles, Cinema camera, and 3D world lyrics with enhanced dual-ring halo glow."
        ),
        keys: KEY_SPECS
            .iter()
            .map(|(k, label, def)| [(*k).to_owned(), (*label).to_owned(), (*def).to_owned()])
            .collect(),
    });
}

pub(super) fn on_settings(game: &mut Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    if mod_id != ID {
        return;
    }
    let mut rt = runtime();
    let lang = game.settings.language.clone();
    ensure_init(&mut rt, &lang);
    let hcfg = rt.ctl.hud_config();
    let on_s = i18n::get("state.on");
    let off_s = i18n::get("state.off");
    buttons.push(i18n::get("modmenu.open_player"));
    buttons.push(i18n::get("modmenu.hud_config"));
    buttons.push(i18n::get("modmenu.edit_hud"));
    buttons.push(i18n::get("modmenu.help"));
    buttons.push(format!(
        "{}: {}",
        i18n::get("modmenu.hud_overlay"),
        if hcfg.master { &on_s } else { &off_s }
    ));
    buttons.push(format!(
        "{}: {}",
        i18n::get("modmenu.world_lyrics"),
        if hcfg.world_lyrics { &on_s } else { &off_s }
    ));
    buttons.push(format!(
        "{}: {}",
        i18n::get("modmenu.world_glow"),
        if hcfg.world_enhanced_glow {
            &on_s
        } else {
            &off_s
        }
    ));
    buttons.push(format!(
        "{}: {}",
        i18n::get("modmenu.cinema"),
        if rt.cinema.active() { &on_s } else { &off_s }
    ));
}

pub(super) fn on_settings_click(game: &mut Game<'_>, mod_id: &str, button: usize) {
    if mod_id != ID {
        return;
    }
    let mut rt = runtime();
    let lang = game.settings.language.clone();
    ensure_init(&mut rt, &lang);
    match button {
        0 => rt.pending_open = PendingScreen::Player,
        1 => rt.pending_open = PendingScreen::Config(ConfigTab::Player),
        2 => rt.pending_open = PendingScreen::Edit,
        3 => rt.pending_open = PendingScreen::Help,
        4 => {
            rt.ctl.toggle_hud_master();
        }
        5 => {
            let on = rt.ctl.toggle_world_lyrics();
            let metrics = HorizonMetrics { game };
            let msg = i18n::raw(if on {
                "toast.world_on"
            } else {
                "toast.world_off"
            });
            rt.world.toast(&metrics, &msg);
        }
        6 => {
            rt.ctl
                .update_hud_config(|c| c.world_enhanced_glow = !c.world_enhanced_glow);
        }
        7 => {
            let yaw = game.native_player().map_or(0.0, |p| p.yaw);
            let hcfg = rt.ctl.hud_config();
            rt.cinema.toggle(yaw, &hcfg);
            sync_cinema_camera_type(game, &mut rt);
        }
        _ => {}
    }
}

pub(super) fn on_client_start(game: &mut Game<'_>) {
    horizon_client::gfx::native_render::set_mods_built_in(true);
    let mut rt = runtime();
    let lang = game.settings.language.clone();
    ensure_init(&mut rt, &lang);
}

pub(super) fn on_client_stopping(_game: &Game<'_>) {
    if let Some(lock) = RUNTIME.get() {
        if let Ok(rt) = lock.lock() {
            if rt.initialized {
                rt.ctl.shutdown();
            }
        }
    }
}

fn apply_side_effects(r: &mut Runtime, effs: Vec<ScreenSideEffect>) {
    for eff in effs {
        match eff {
            ScreenSideEffect::OpenUrl(url) => controller::open_browser_url(&url),
            ScreenSideEffect::CopyClipboard(txt) => r.clipboard.set(&txt),
            ScreenSideEffect::CloseHostScreen => {
                if r.overlay_host_id.is_some() {
                    r.overlay_host_id = None;
                } else {
                    r.pending_close = true;
                }
            }
        }
    }
}

fn open_pending_screen(r: &mut Runtime, req: PendingScreen, sw: i32, sh: i32) {
    let ctl = r.ctl.clone();
    match req {
        PendingScreen::Player => r.screens.open_player(&ctl, sw, sh),
        PendingScreen::Config(tab) => r.screens.open_config(tab, &ctl, sw, sh),
        PendingScreen::Edit => {
            r.screens.open_config(ConfigTab::Widget, &ctl, sw, sh);
            let _ = r.screens.poll_side_effects();
            r.screens.open_edit_direct(&ctl, sw, sh);
        }
        PendingScreen::Help => {
            r.screens.open_help_direct(sw, sh);
        }
        PendingScreen::None => {}
    }
}

fn is_our_screen_active(game: &Game<'_>, rt: &Runtime, screen_id: &str) -> bool {
    (game.native_mod_screen() == Some(ID) && screen_id == "mod")
        || (rt.screens.is_open() && rt.overlay_host_id.as_deref() == Some(screen_id))
}

fn menu_icon_rect(screen_id: &str, sw: i32, sh: i32) -> Option<[i32; 4]> {
    match screen_id {
        "title" => {
            let cx = sw / 2;
            let y = sh / 4 + 48;
            Some([cx + 38, y + 72, 20, 20])
        }
        "paused" => {
            let x0 = ((sw as f32 - 212.0) * 0.5).floor() as i32;
            let y0 = ((sh as f32 - 166.0) * 0.25).floor() as i32;
            Some([x0 + 216, y0 + 50, 20, 20])
        }
        _ => None,
    }
}

fn render_menu_icon_button(
    g: &mut DrawList,
    metrics: &dyn TextMetrics,
    r: [i32; 4],
    th: &PlayerTheme,
    playing: bool,
    mx: i32,
    my: i32,
) {
    let hov = mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3];
    let bg = if hov { 0xD8_24242C } else { 0xB0_121216 };
    let border = if hov || playing {
        th.accent
    } else {
        0x55_FFFFFF
    };
    shapes::fill(g, r[0], r[1], r[2], r[3], 5, bg);
    shapes::ring(g, r[0], r[1], r[2], r[3], 5, border);
    let note_col = if hov || playing {
        th.accent
    } else {
        PlayerTheme::TEXT_MAIN
    };
    let nw = metrics.width("♪");
    g.text(
        "♪",
        r[0] as f32 + (r[2] as f32 - nw) * 0.5,
        r[1] as f32 + (r[3] as f32 - metrics.line_height()) * 0.5,
        note_col,
        true,
    );
    if playing {
        shapes::fill(g, r[0] + r[2] - 5, r[1] + 2, 3, 3, 1, th.accent);
    }
    if hov {
        let tip = "Overtone [M]";
        let tw = metrics.width(tip).ceil() as i32 + 10;
        let tx = (r[0] + (r[2] - tw) / 2).max(4);
        let ty = r[1] + r[3] + 4;
        shapes::fill(g, tx, ty, tw, 15, 4, 0xE6_141418);
        shapes::ring(g, tx, ty, tw, 15, 4, th.accent);
        g.text(
            tip,
            (tx + 5) as f32,
            (ty + 3) as f32,
            PlayerTheme::TEXT_MAIN,
            false,
        );
    }
}

pub(super) fn on_frame(game: &mut Game<'_>, elapsed: Duration) {
    let (sw_f, sh_f) = game.gui_size();
    let sw = (sw_f.round() as i32).max(1);
    let sh = (sh_f.round() as i32).max(1);

    let (open_req, want_close, upload) = {
        let mut rt = runtime();
        let lang = game.settings.language.clone();
        ensure_init(&mut rt, &lang);
        let r = &mut *rt;
        if r.last_lang != lang {
            r.last_lang = lang.clone();
            i18n::set_language(Language::from_code(&lang));
        }

        let music_vol = game.settings.final_volume("music");
        let focused = game.focused;
        let dev = game.settings.sound_device.clone();
        r.ctl.set_game_state(music_vol, focused, &dev);

        let dt = elapsed.as_secs_f32().clamp(0.0001, 0.25);
        r.ctl.tick(dt);

        let player_opt = game.native_player();
        let moving = match (player_opt.as_ref(), r.last_player_pos) {
            (Some(p), Some(prev)) => {
                let d2 =
                    (p.x - prev[0]).powi(2) + (p.y - prev[1]).powi(2) + (p.z - prev[2]).powi(2);
                d2 > 0.0004
            }
            _ => false,
        };
        if let Some(p) = player_opt.as_ref() {
            r.last_player_pos = Some([p.x, p.y, p.z]);
        }
        let no_screen =
            game.native_mod_screen().is_none() && game.native_playing() && !r.screens.is_open();
        let hcfg = r.ctl.hud_config();
        r.cinema
            .tick(&hcfg, player_opt.is_some(), no_screen, moving);
        sync_cinema_camera_type(game, r);

        // Offer cover downloads for the current track and sync atlas buffer.
        if let Some(track) = r.ctl.current_track() {
            if r.last_cover_offer_id != track.id {
                r.last_cover_offer_id = track.id.clone();
                r.covers.offer_hud(&track.id, track.cover_url.as_deref());
                r.covers.offer_big(&track.id, track.cover_url.as_deref());
            }
            r.atlas.sync_covers(&r.covers, &track.id);
        }

        if r.overlay_host_id.is_some() && game.native_playing() {
            r.screens.close_all();
            let _ = r.screens.poll_side_effects();
            r.overlay_host_id = None;
        } else if game.native_mod_screen() == Some(ID) || r.overlay_host_id.is_some() {
            let ctl = r.ctl.clone();
            let mut cinema = std::mem::take(&mut r.cinema);
            r.screens.tick(&ctl, &mut cinema, sw, sh);
            r.cinema = cinema;
            let effs = r.screens.poll_side_effects();
            apply_side_effects(r, effs);
            if !r.screens.is_open() {
                r.overlay_host_id = None;
            }
        } else if r.screens.is_open() {
            r.screens.close_all();
            let _ = r.screens.poll_side_effects();
        }

        let open_req = r.pending_open;
        r.pending_open = PendingScreen::None;
        let want_close = r.pending_close;
        r.pending_close = false;
        let upload = if r.atlas.dirty {
            r.atlas.dirty = false;
            Some((r.atlas.width, r.atlas.height, r.atlas.rgba.clone()))
        } else {
            None
        };
        (open_req, want_close, upload)
    };

    if let Some((w, h, rgba_buf)) = upload {
        game.native_mod_image(w, h, &rgba_buf);
    }
    if want_close {
        game.native_mod_screen_close(ID);
    }
    if open_req != PendingScreen::None {
        if game.native_mod_screen_open(ID) {
            let mut rt = runtime();
            rt.overlay_host_id = None;
            open_pending_screen(&mut rt, open_req, sw, sh);
        } else if !game.native_playing() {
            let mut rt = runtime();
            if let Some(host_id) = rt.last_vanilla_screen.clone() {
                rt.overlay_host_id = Some(host_id);
                open_pending_screen(&mut rt, open_req, sw, sh);
            }
        }
    }
}

pub(super) fn on_hud(game: &mut Game<'_>, batch: &mut HudBatch, _corners: &mut Corners) {
    if !game.native_on(ID) {
        return;
    }
    let (sw_f, sh_f) = game.gui_size();
    let sw = (sw_f.round() as i32).max(1);
    let sh = (sh_f.round() as i32).max(1);
    let in_world = game.native_player().is_some();

    let mut list = DrawList::new();
    {
        let mut rt = runtime();
        let lang = game.settings.language.clone();
        ensure_init(&mut rt, &lang);
        let gui_hidden =
            game.ui.hide_gui || game.native_mod_screen() == Some(ID) || rt.screens.is_open();
        let r = &mut *rt;
        let tab = r.tab_down;
        let cinema_active = r.cinema.active();
        let ctl = r.ctl.clone();
        let hcfg = ctl.hud_config();
        let bar_h = r.cinema.bar_height(&hcfg, sh);
        if bar_h > 0 {
            list.fill(0.0, 0.0, sw as f32, bar_h as f32, 0xFF00_0000);
            list.fill(0.0, (sh - bar_h) as f32, sw as f32, sh as f32, 0xFF00_0000);
        }
        let world_hides_hud = WorldLyricsState::hides_hud(&ctl, in_world);
        let mut hud = std::mem::take(&mut r.hud);
        let metrics = HorizonMetrics { game };
        hud.render_all(
            &mut list,
            &metrics,
            &ctl,
            &r.covers,
            sw,
            sh,
            0.016,
            false,
            tab,
            gui_hidden || !in_world,
            cinema_active,
            world_hides_hud,
        );
        r.hud = hud;
    }
    flush_draw_list(game, batch, &list);
}

pub(super) fn on_gui_render_post(
    game: &Game<'_>,
    batch: &mut HudBatch,
    gui_hidden: bool,
    focused: bool,
) {
    if !game.native_on(ID) || !focused || gui_hidden || game.native_playing() {
        return;
    }
    if game.native_mod_screen() == Some(ID) {
        return;
    }
    let (sw_f, sh_f) = game.gui_size();
    let sw = (sw_f.round() as i32).max(1);
    let sh = (sh_f.round() as i32).max(1);

    let mut list = DrawList::new();
    {
        let mut rt = runtime();
        let lang = game.settings.language.clone();
        ensure_init(&mut rt, &lang);
        let cfg = rt.ctl.hud_config();
        let ctl = rt.ctl.clone();
        let metrics = HorizonMetrics { game };
        let mut hud = std::mem::take(&mut rt.hud);
        hud.render_volume_only(&mut list, &metrics, &ctl, &cfg, sw, sh);
        rt.hud = hud;
    }
    flush_draw_list(game, batch, &list);
}

pub(super) fn on_world_submit(game: &Game<'_>, draw: &mut NativeWorldDraw) {
    if !game.native_on(ID) || (game.native_player().is_none() && game.native_camera().is_none()) {
        return;
    }
    horizon_client::gfx::native_render::set_mods_built_in(true);
    let cmds = {
        let mut rt = runtime();
        let lang = game.settings.language.clone();
        ensure_init(&mut rt, &lang);
        let cinema_active = rt.cinema.active();
        let fallback_dim = || match game.dim {
            crate::game::Dim::Overworld => "minecraft:overworld",
            crate::game::Dim::Nether => "minecraft:the_nether",
            crate::game::Dim::End => "minecraft:the_end",
        };
        let (cam_pos, cam_yaw, cam_pitch, dim_id) = if cinema_active {
            let (yaw, pitch) = game
                .posed_rotation
                .unwrap_or((game.view.y_rot, game.view.x_rot));
            (draw.camera, yaw, Some(pitch), fallback_dim().to_owned())
        } else if let Some(cam) = game.native_camera() {
            (cam.position, cam.yaw, None, cam.dimension)
        } else {
            let yaw = game.locator_camera.map_or(game.yaw, |(_, y, _, _)| y);
            (draw.camera, yaw, None, fallback_dim().to_owned())
        };
        let metrics = HorizonMetrics { game };
        let probe = HorizonBlockProbe { game };
        let mut world = std::mem::take(&mut rt.world);
        let ctl = rt.ctl.clone();
        let out = world.update_and_emit_view(
            &ctl,
            &metrics,
            &probe,
            cam_pos,
            cam_yaw,
            cam_pitch,
            &dim_id,
        );
        rt.world = world;
        out
    };

    for cmd in cmds {
        if (cmd.argb >> 24) < 4 || cmd.text.is_empty() {
            continue;
        }
        draw.push();
        if !draw.translate_world(cmd.origin) {
            draw.pop();
            continue;
        }
        draw.rotate_about([0.0, 1.0, 0.0], cmd.yaw.to_degrees(), [0.0; 3]);
        if cmd.flat_pitch != 0.0 {
            draw.rotate_about([1.0, 0.0, 0.0], cmd.flat_pitch.to_degrees(), [0.0; 3]);
        }
        draw.rotate_about([0.0, 0.0, 1.0], cmd.roll.to_degrees(), [0.0; 3]);
        draw.scale([cmd.base_scale, -cmd.base_scale, cmd.base_scale]);
        draw.translate(cmd.local_pos);
        draw.rotate_about([0.0, 1.0, 0.0], cmd.piece_rot_y.to_degrees(), [0.0; 3]);
        draw.rotate_about([0.0, 0.0, 1.0], cmd.piece_rot_z.to_degrees(), [0.0; 3]);
        draw.scale([cmd.piece_scale, cmd.piece_scale, 1.0]);
        draw.translate([-cmd.width / 2.0, -4.0, 0.0]);

        let label = Text::literal(&cmd.text);
        if let Some(glow) = cmd.glow {
            if glow.enhanced && glow.outer_alpha >= 4 {
                let outer_col = (u32::from(glow.outer_alpha) << 24) | (glow.rgb & 0x00FF_FFFF);
                let r = glow.radius * 1.85;
                for k in 0..8 {
                    let ang = (k as f32) * std::f32::consts::FRAC_PI_4;
                    draw.push();
                    draw.translate([ang.cos() * r, ang.sin() * r, -0.002]);
                    game.native_world_text(draw, &label, outer_col, None);
                    draw.pop();
                }
            }
            if glow.inner_alpha >= 4 {
                let inner_col = (u32::from(glow.inner_alpha) << 24) | (glow.rgb & 0x00FF_FFFF);
                let r = glow.radius;
                for k in 0..8 {
                    let ang = (k as f32) * std::f32::consts::FRAC_PI_4;
                    draw.push();
                    draw.translate([ang.cos() * r, ang.sin() * r, -0.001]);
                    game.native_world_text(draw, &label, inner_col, None);
                    draw.pop();
                }
            }
        }
        let outline = if cmd.shadow {
            let a = cmd.argb >> 24;
            Some((a << 24) | 0x0012_1218)
        } else {
            None
        };
        game.native_world_text(draw, &label, cmd.argb, outline);
        draw.pop();
    }
}

pub(super) fn on_render_hand(game: &Game<'_>, canceled: &mut bool) {
    if !game.native_on(ID) {
        return;
    }
    if runtime().cinema.active() {
        *canceled = true;
    }
}

pub(super) fn on_camera_pose(
    game: &Game<'_>,
    partial: f32,
    position: &mut [f64; 3],
    yaw: &mut f32,
    pitch: &mut f32,
) {
    if !game.native_on(ID) {
        return;
    }
    let Some(player) = game.native_player() else {
        return;
    };
    let mut rt = runtime();
    if !rt.cinema.active() {
        return;
    }
    let hcfg = rt.ctl.hud_config();
    let y_rad = f64::from(player.yaw).to_radians();
    let p_rad = f64::from(player.pitch).to_radians();
    let vdir = [
        -y_rad.sin() * p_rad.cos(),
        -p_rad.sin(),
        y_rad.cos() * p_rad.cos(),
    ];
    let partial = partial.clamp(0.0, 1.0);
    let eye_height =
        game.view.eye_height_old + (game.view.eye_height - game.view.eye_height_old) * partial;
    let eye_world = game.view.eye;
    let (player_pos, resolved_eye_h) = if eye_world[0].is_finite()
        && eye_world[1].is_finite()
        && eye_world[2].is_finite()
        && eye_height > 0.05
    {
        (
            [
                eye_world[0],
                eye_world[1] - f64::from(eye_height),
                eye_world[2],
            ],
            f64::from(eye_height),
        )
    } else {
        ([player.x, player.y, player.z], 1.62)
    };
    let start_eye = if rt.saved_camera_type.is_some() {
        eye_world
    } else {
        *position
    };
    let mut cinema = std::mem::take(&mut rt.cinema);
    let view_opt = cinema.frame(
        &hcfg,
        player_pos,
        resolved_eye_h,
        player.yaw,
        start_eye,
        vdir,
        70.0,
        |target, cand| {
            let probe = HorizonBlockProbe { game };
            clip_camera_ray(&probe, target, cand)
        },
    );
    rt.cinema = cinema;
    if let Some(view) = view_opt {
        *position = view.pos;
        *yaw = view.yaw;
        *pitch = view.pitch;
    } else if rt.saved_camera_type.is_some() {
        *position = eye_world;
        *yaw = game.view.eye_rotation[0];
        *pitch = game.view.eye_rotation[1];
    }
}

fn clip_camera_ray(
    probe: &HorizonBlockProbe<'_, '_>,
    target: [f64; 3],
    cand: [f64; 3],
) -> [f64; 3] {
    let dx = cand[0] - target[0];
    let dy = cand[1] - target[1];
    let dz = cand[2] - target[2];
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len < 0.25 {
        return cand;
    }
    let steps = (len * 4.0).clamp(4.0, 48.0) as i32;
    for s in 1..=steps {
        let t = s as f64 / steps as f64;
        let pt = [target[0] + dx * t, target[1] + dy * t, target[2] + dz * t];
        let bx = pt[0].floor() as i32;
        let by = pt[1].floor() as i32;
        let bz = pt[2].floor() as i32;
        if !probe.game.native_block_collision([bx, by, bz]).is_empty() {
            let mut lo = (s - 1) as f64 / steps as f64;
            let mut hi = t;
            for _ in 0..8 {
                let mid = 0.5 * (lo + hi);
                let mpt = [
                    target[0] + dx * mid,
                    target[1] + dy * mid,
                    target[2] + dz * mid,
                ];
                let mbx = mpt[0].floor() as i32;
                let mby = mpt[1].floor() as i32;
                let mbz = mpt[2].floor() as i32;
                if !probe.game.native_block_collision([mbx, mby, mbz]).is_empty() {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let safe = (lo * 0.96).max(0.0);
            return [
                target[0] + dx * safe,
                target[1] + dy * safe,
                target[2] + dz * safe,
            ];
        }
    }
    cand
}

pub(super) fn on_compute_fov(game: &Game<'_>, _partial: f32, _camera: [f64; 3], degrees: &mut f32) {
    if !game.native_on(ID) {
        return;
    }
    let rt = runtime();
    if rt.cinema.active() {
        *degrees = rt.cinema.fov(*degrees).clamp(15.0, 170.0);
    }
}

pub(super) fn on_music(game: &Game<'_>, music: &mut Option<Music>, volume: &mut f32) {
    if !game.native_on(ID) {
        return;
    }
    let rt = runtime();
    if rt.initialized && rt.ctl.is_playing() && rt.ctl.volume() > 0.01 {
        *music = None;
        *volume = 0.0;
    }
}

pub(super) fn on_input_key(_game: &mut Game<'_>, _event: &KeyEvent, modifiers: u32) {
    let mut rt = runtime();
    rt.modifiers = modifiers;
}

fn resolved_hotkey(game: &Game<'_>, hk: &config::HotkeyConfig, idx: usize) -> String {
    let act = config::HotkeyAction::ALL[idx];
    let local = hk.key_of(act);
    if local != act.default_key() {
        local.to_owned()
    } else {
        game.native_mod_key(ID, KEY_SPECS[idx].0, KEY_SPECS[idx].2)
    }
}

fn dispatch_transport_hotkey(rt: &mut Runtime, idx: usize) -> bool {
    match idx {
        3 => {
            if rt.ctl.active_platform() != Platform::Local || rt.ctl.current_track().is_some() {
                rt.ctl.prev();
            }
            true
        }
        4 => {
            if rt.ctl.active_platform() != Platform::Local || rt.ctl.current_track().is_some() {
                rt.ctl.next();
            }
            true
        }
        5 => {
            rt.ctl.toggle_pause();
            true
        }
        6 => {
            rt.ctl.step_volume(VOLUME_STEP);
            let v = rt.ctl.volume();
            rt.hud.notify_volume_changed(v);
            true
        }
        7 => {
            rt.ctl.step_volume(-VOLUME_STEP);
            let v = rt.ctl.volume();
            rt.hud.notify_volume_changed(v);
            true
        }
        _ => false,
    }
}

pub(super) fn on_key(game: &mut Game<'_>, key: &str, pressed: bool) {
    if !game.native_on(ID) {
        return;
    }
    {
        let mut rt = runtime();
        if key == "key.keyboard.tab" {
            rt.tab_down = pressed;
        }
    }
    if !pressed {
        return;
    }
    if game.native_mod_screen() == Some(ID) || runtime().screens.is_open() {
        return;
    }

    let playing = game.native_playing();
    let mut rt = runtime();
    let lang = game.settings.language.clone();
    ensure_init(&mut rt, &lang);
    let hk = rt.ctl.hotkey_config();
    let Some(idx) = (0..10).position(|i| {
        let b = resolved_hotkey(game, &hk, i);
        !b.is_empty() && b != "key.keyboard.unknown" && b == key
    }) else {
        return;
    };

    let is_transport = matches!(idx, 3..=7);
    if !playing && !is_transport {
        return;
    }

    if dispatch_transport_hotkey(&mut rt, idx) {
        return;
    }
    match idx {
        0 => rt.pending_open = PendingScreen::Player,
        1 => {
            rt.ctl.toggle_hud_master();
        }
        2 => rt.pending_open = PendingScreen::Config(ConfigTab::Widget),
        8 => {
            let on = rt.ctl.toggle_world_lyrics();
            let metrics = HorizonMetrics { game };
            let msg = i18n::raw(if on {
                "toast.world_on"
            } else {
                "toast.world_off"
            });
            rt.world.toast(&metrics, &msg);
        }
        9 => {
            let yaw = game.native_player().map_or(0.0, |p| p.yaw);
            let hcfg = rt.ctl.hud_config();
            rt.cinema.toggle(yaw, &hcfg);
            sync_cinema_camera_type(game, &mut rt);
        }
        _ => {}
    }
}

pub(super) fn on_wheel(game: &mut Game<'_>, steps: f32, claimed: &mut bool) {
    if !game.native_on(ID) || steps == 0.0 || !game.native_playing() {
        return;
    }
    let mut rt = runtime();
    let alt_down = (rt.modifiers & 4) != 0;
    if !alt_down {
        return;
    }
    rt.ctl.step_volume(steps.signum() * VOLUME_STEP);
    let v = rt.ctl.volume();
    rt.hud.notify_volume_changed(v);
    *claimed = true;
}

pub(super) fn on_screen_render_post(
    game: &Game<'_>,
    batch: &mut HudBatch,
    screen: &NativeScreenView<'_>,
) {
    if !game.native_on(ID) {
        return;
    }
    let sw = (screen.rect[2].round() as i32).max(1);
    let sh = (screen.rect[3].round() as i32).max(1);
    let mx = screen.mouse[0].round() as i32;
    let my = screen.mouse[1].round() as i32;

    let mut list = DrawList::new();
    {
        let mut rt = runtime();
        let lang = game.settings.language.clone();
        ensure_init(&mut rt, &lang);
        if screen.id != "mod" {
            rt.last_vanilla_screen = Some(screen.id.to_owned());
        }
        if is_our_screen_active(game, &rt, screen.id) {
            let r = &mut *rt;
            let ctl = r.ctl.clone();
            let mut atlas = std::mem::replace(&mut r.atlas, ModImageAtlas::new());
            let mut world = std::mem::take(&mut r.world);
            let mut cinema = std::mem::take(&mut r.cinema);
            let metrics = HorizonMetrics { game };
            r.screens.render(
                &mut list,
                &metrics,
                &ctl,
                &r.covers,
                &mut atlas,
                &mut world,
                &mut cinema,
                sw,
                sh,
                mx,
                my,
                0.016,
            );
            r.atlas = atlas;
            r.world = world;
            r.cinema = cinema;
        } else if !rt.screens.is_open() {
            if let Some(btn_rect) = menu_icon_rect(screen.id, sw, sh) {
                let th = PlayerTheme::from_config(&rt.ctl.player_config(), &rt.ctl.hud_config());
                let playing = rt.ctl.is_playing();
                let metrics = HorizonMetrics { game };
                render_menu_icon_button(&mut list, &metrics, btn_rect, &th, playing, mx, my);
            }
        }
    }
    if !list.cmds.is_empty() {
        flush_draw_list(game, batch, &list);
    }
}

pub(super) fn on_screen_mouse_pressed(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    button: i32,
    _modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let sw = (screen.rect[2].round() as i32).max(1);
    let sh = (screen.rect[3].round() as i32).max(1);
    let mx = screen.mouse[0].round() as i32;
    let my = screen.mouse[1].round() as i32;

    let mut rt = runtime();
    if is_our_screen_active(game, &rt, screen.id) {
        let r = &mut *rt;
        r.mouse_btn = button;
        let ctl = r.ctl.clone();
        let mut world = std::mem::take(&mut r.world);
        let mut cinema = std::mem::take(&mut r.cinema);
        let metrics = HorizonMetrics { game };
        r.screens.mouse_down(
            mx,
            my,
            button,
            &ctl,
            &mut world,
            &mut cinema,
            &metrics,
            sw,
            sh,
        );
        r.world = world;
        r.cinema = cinema;
        let effs = r.screens.poll_side_effects();
        apply_side_effects(r, effs);
        *canceled = true;
    } else if !rt.screens.is_open() && button == 0 {
        if let Some(btn) = menu_icon_rect(screen.id, sw, sh) {
            if mx >= btn[0] && mx < btn[0] + btn[2] && my >= btn[1] && my < btn[1] + btn[3] {
                let lang = game.settings.language.clone();
                ensure_init(&mut rt, &lang);
                rt.overlay_host_id = Some(screen.id.to_owned());
                open_pending_screen(&mut rt, PendingScreen::Player, sw, sh);
                *canceled = true;
            }
        }
    }
}

pub(super) fn on_screen_mouse_released(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    button: i32,
    _modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let mut rt = runtime();
    if !is_our_screen_active(game, &rt, screen.id) {
        return;
    }
    let mx = screen.mouse[0].round() as i32;
    let my = screen.mouse[1].round() as i32;
    let ctl = rt.ctl.clone();
    rt.screens.mouse_up(mx, my, button, &ctl);
    *canceled = true;
}

pub(super) fn on_screen_mouse_dragged(
    game: &mut Game<'_>,
    screen: &str,
    mouse: [f32; 2],
    _button: i32,
    _delta: [f32; 2],
    _modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let (sw_f, sh_f) = game.gui_size();
    let sw = (sw_f.round() as i32).max(1);
    let sh = (sh_f.round() as i32).max(1);
    let mut rt = runtime();
    if !is_our_screen_active(game, &rt, screen) {
        return;
    }
    let ctl = rt.ctl.clone();
    rt.screens.mouse_drag(
        mouse[0].round() as i32,
        mouse[1].round() as i32,
        &ctl,
        sw,
        sh,
    );
    *canceled = true;
}

pub(super) fn on_screen_mouse_scrolled(
    game: &mut Game<'_>,
    screen: &str,
    mouse: [f32; 2],
    scroll: [f64; 2],
    _modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let (sw_f, sh_f) = game.gui_size();
    let sw = (sw_f.round() as i32).max(1);
    let sh = (sh_f.round() as i32).max(1);
    let mut rt = runtime();
    if !is_our_screen_active(game, &rt, screen) {
        return;
    }
    let ctl = rt.ctl.clone();
    rt.screens.mouse_scroll(
        mouse[0].round() as i32,
        mouse[1].round() as i32,
        scroll[1] as f32,
        &ctl,
        sw,
        sh,
    );
    *canceled = true;
}

pub(super) fn on_screen_key_pressed(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    key: &str,
    _repeat: bool,
    modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let sw = (screen.rect[2].round() as i32).max(1);
    let sh = (screen.rect[3].round() as i32).max(1);
    let ctrl = (modifiers & 2) != 0;
    let shift = (modifiers & 1) != 0;

    let mut rt = runtime();
    let lang = game.settings.language.clone();
    ensure_init(&mut rt, &lang);

    if is_our_screen_active(game, &rt, screen.id) {
        let hk = rt.ctl.hotkey_config();
        let open_key = resolved_hotkey(game, &hk, 0);
        if !rt.screens.is_text_input_focused()
            && !rt.screens.is_listening_hotkey()
            && !open_key.is_empty()
            && open_key != "key.keyboard.unknown"
            && key == open_key
        {
            let ctl = rt.ctl.clone();
            rt.screens.close_all_animated(&ctl);
            rt.swallow_char_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
            let effs = rt.screens.poll_side_effects();
            apply_side_effects(&mut rt, effs);
            *canceled = true;
            return;
        }

        let r = &mut *rt;
        if ctrl && key == "key.keyboard.v" {
            let txt = r.clipboard.get();
            if !txt.is_empty() {
                let ctl = r.ctl.clone();
                let _ = r.screens.paste_text(&txt, &ctl);
            }
            r.swallow_char_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
            *canceled = true;
            return;
        }

        let sk = map_screen_key(key);
        let ctl = r.ctl.clone();
        let mut world = std::mem::take(&mut r.world);
        let mut cinema = std::mem::take(&mut r.cinema);
        let metrics = HorizonMetrics { game };
        let _handled = r.screens.key_pressed(
            sk,
            key,
            ctrl,
            shift,
            &ctl,
            &mut world,
            &mut cinema,
            &metrics,
            sw,
            sh,
        );
        r.world = world;
        r.cinema = cinema;
        let effs = r.screens.poll_side_effects();
        apply_side_effects(r, effs);
        *canceled = true;
        return;
    }

    if !rt.screens.is_open()
        && matches!(
            screen.id,
            "title"
                | "paused"
                | "disconnected"
                | "options"
                | "world_select"
                | "multiplayer"
                | "server_links"
        )
    {
        let hk = rt.ctl.hotkey_config();
        let Some(idx) = (0..10).position(|i| {
            let b = resolved_hotkey(game, &hk, i);
            !b.is_empty() && b != "key.keyboard.unknown" && b == key
        }) else {
            return;
        };
        if idx == 0 {
            rt.overlay_host_id = Some(screen.id.to_owned());
            open_pending_screen(&mut rt, PendingScreen::Player, sw, sh);
            rt.swallow_char_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
            *canceled = true;
        } else if idx == 2 {
            rt.overlay_host_id = Some(screen.id.to_owned());
            open_pending_screen(&mut rt, PendingScreen::Config(ConfigTab::Widget), sw, sh);
            rt.swallow_char_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
            *canceled = true;
        } else if dispatch_transport_hotkey(&mut rt, idx) {
            *canceled = true;
        }
    }
}

pub(super) fn on_screen_key_released(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    _key: &str,
    _modifiers: u32,
    canceled: &mut bool,
) {
    if game.native_on(ID) && is_our_screen_active(game, &runtime(), screen.id) {
        *canceled = true;
    }
}

pub(super) fn on_screen_char_typed(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    ch: char,
    modifiers: u32,
    canceled: &mut bool,
) {
    if !game.native_on(ID) {
        return;
    }
    let mut rt = runtime();
    if rt
        .swallow_char_until
        .is_some_and(|t| std::time::Instant::now() <= t)
    {
        *canceled = true;
        return;
    }
    if !is_our_screen_active(game, &rt, screen.id) {
        return;
    }
    if (modifiers & 2) != 0 || ch.is_control() {
        *canceled = true;
        return;
    }
    let ctl = rt.ctl.clone();
    let _ = rt.screens.char_typed(ch, &ctl);
    *canceled = true;
}

pub(super) fn on_drop_file(game: &mut Game<'_>, path: &Path) {
    if !game.native_on(ID) {
        return;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let mut rt = runtime();
    if game.native_mod_screen() != Some(ID) && !rt.screens.is_open() {
        return;
    }
    let ctl = rt.ctl.clone();
    if matches!(ext.as_str(), "lrc" | "elrc" | "ttml" | "txt") {
        let _ = ctl.import_lyrics_file(path);
    } else if local::AudioKind::from_path(path).is_some() {
        let _ = rt.screens.on_files_drop(&[path.to_path_buf()], &ctl);
    }
}

fn map_screen_key(key: &str) -> ScreenKey {
    match key {
        "key.keyboard.escape" => ScreenKey::Escape,
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => ScreenKey::Enter,
        "key.keyboard.backspace" => ScreenKey::Backspace,
        "key.keyboard.delete" => ScreenKey::Delete,
        "key.keyboard.left" => ScreenKey::Left,
        "key.keyboard.right" => ScreenKey::Right,
        "key.keyboard.up" => ScreenKey::Up,
        "key.keyboard.down" => ScreenKey::Down,
        "key.keyboard.home" => ScreenKey::Home,
        "key.keyboard.end" => ScreenKey::End,
        "key.keyboard.space" => ScreenKey::Space,
        "key.keyboard.tab" => ScreenKey::Tab,
        _ => ScreenKey::Other,
    }
}

fn flush_draw_list(game: &Game<'_>, batch: &mut HudBatch, list: &DrawList) {
    let mut scissors: Vec<(usize, f32, f32, f32, f32)> = Vec::new();
    for cmd in &list.cmds {
        match cmd {
            DrawCmd::FillRect { x, y, w, h, argb } => {
                game.native_fill(batch, [*x, *y, *w, *h], *argb);
            }
            DrawCmd::Text {
                text,
                x,
                y,
                argb,
                shadow,
                scale,
            } => {
                let [a, r, g, b] = argb.to_be_bytes();
                let color = rgba(r, g, b, a);
                let start = batch.vertices.len();
                batch.text_styled(
                    &game.gui.atlas,
                    &Text::literal(text),
                    *x,
                    *y,
                    color,
                    *shadow,
                );
                if (*scale - 1.0).abs() > 0.001 {
                    batch.scale_since(start, *x, *y, *scale, *scale);
                }
            }
            DrawCmd::Tex {
                slot,
                x,
                y,
                w,
                h,
                alpha,
            } => {
                let Some(mut base) = game.gui.atlas.sprites.get(MOD_IMAGE_SPRITE).copied() else {
                    continue;
                };
                let src = match slot {
                    TexSlot::CoverHud => ModImageAtlas::HUD_RECT,
                    TexSlot::CoverBig => ModImageAtlas::BIG_RECT,
                    TexSlot::PickerSquare => ModImageAtlas::PICKER_SQ_RECT,
                    TexSlot::PickerHue => ModImageAtlas::PICKER_HUE_RECT,
                };
                let du = (base.u1 - base.u0) / base.w as f32;
                let dv = (base.v1 - base.v0) / base.h as f32;
                base.u0 += src[0] * du;
                base.v0 += src[1] * dv;
                base.u1 = base.u0 + src[2] * du;
                base.v1 = base.v0 + src[3] * dv;
                let a = (*alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                batch.rect_uv(*x, *y, *w, *h, base, rgba(255, 255, 255, a));
            }
            DrawCmd::PushScissor { x, y, w, h } => {
                scissors.push((batch.vertices.len(), *x, *y, *x + *w, *y + *h));
            }
            DrawCmd::PopScissor => {
                if let Some((start, x0, y0, x1, y1)) = scissors.pop() {
                    batch.clip_since(start, x0, y0, x1, y1);
                }
            }
        }
    }
    while let Some((start, x0, y0, x1, y1)) = scissors.pop() {
        batch.clip_since(start, x0, y0, x1, y1);
    }
}
