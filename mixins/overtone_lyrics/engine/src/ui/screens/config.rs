use crate::cinema::CinematicController;
use crate::config::{
    Accent, Anchor, CineCuts, CineShots, FollowStyle, HotkeyAction, HotkeyConfig, HudConfig,
    Layout, LyricsEnter, LyricsMode, PlayerConfig, ShowMode, VizColor, VizDirection, VizSource,
    VizStyle, VolumeStyle, WindowAnim, WorldAnim, WorldColor, WorldExit, WorldGlow, WorldLayout,
    WorldMode, WorldUnit,
};
use crate::controller::PlayerController;
use crate::i18n;
use crate::ui::screens::{PickerTarget, ScreenKey, ScreenRoute, TextInput};
use crate::ui::world_lyrics::WorldLyricsState;
use crate::ui::{DrawList, PlayerTheme, TextMetrics, shapes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTab {
    Widget,
    Lyrics,
    World,
    Volume,
    Visualizer,
    Cinema,
    Player,
    Keys,
    General,
}

impl ConfigTab {
    pub const ALL: [Self; 9] = [
        Self::Widget,
        Self::Lyrics,
        Self::World,
        Self::Volume,
        Self::Visualizer,
        Self::Cinema,
        Self::Player,
        Self::Keys,
        Self::General,
    ];

    pub fn title(self) -> String {
        let k = match self {
            Self::Widget => "tab.widget",
            Self::Lyrics => "tab.lyrics",
            Self::World => "tab.world",
            Self::Volume => "tab.volume",
            Self::Visualizer => "tab.visualizer",
            Self::Cinema => "tab.cinema",
            Self::Player => "tab.player",
            Self::Keys => "tab.keys",
            Self::General => "tab.general",
        };
        i18n::get(k)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigAction {
    SpawnWorldDemo,
    ToggleCinemaPreview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingId {
    // Player tab
    SyncAccent,
    PlayerAccent,
    PlayerCustomColor,
    WindowScale,
    CornerRadius,
    WindowAnim,
    Tint,
    WindowOpacity,
    Dim,
    ShowCover,
    CoverPx,
    ShowLyrics,
    AccentText,
    DuckInactive,
    Prefetch,
    PrefetchCount,
    DiskCache,
    DiscordPresence,
    // HUD tab
    Master,
    HideWithGui,
    HideOnTab,
    WidgetEnabled,
    Layout,
    ShowMode,
    ShowOnSongStart,
    ShowOnSongEnd,
    HudAccent,
    HudCustomColor,
    HudScale,
    BgOpacity,
    HudCover,
    HudTimestamps,
    HudWaveform,
    HudAnchor,
    HudOffsetX,
    HudOffsetY,
    // Volume tab
    VolEnabled,
    VolStyle,
    VolSeconds,
    VolPercent,
    VolScale,
    VolBg,
    VolAnchor,
    VolOffsetX,
    VolOffsetY,
    // Lyrics tab (2D + PRO MAX sources)
    LyricsMode,
    CardLyrics,
    WordKaraoke,
    LyricsEnter,
    LyricsExit,
    LyricsLinger,
    LyricsLetterMs,
    LyricsShiftMs,
    LyricsScale,
    LyricsWidth,
    LyricsLines,
    LyricsBg,
    LyricsAccent,
    LyricsShadow,
    LyricsAnchor,
    LyricsOffsetX,
    LyricsOffsetY,
    LyricsSourceMode,
    OnlineLyrics,
    AmllLyrics,
    NeteaseLyrics,
    QqLyrics,
    KugouLyrics,
    BetterLyrics,
    DspAutoAlign,
    MusixmatchToken,
    // World 3D tab
    WorldLyrics,
    LyricsWithWorld,
    WorldMode,
    FollowStyle,
    WorldLayout,
    WorldScale,
    WorldDistance,
    WorldHeight,
    WorldSpread,
    WorldSeeThrough,
    WorldShadow,
    WorldAnim,
    WorldUnit,
    WorldExit,
    WorldLinger,
    WorldLetterMs,
    WorldAnimSpeed,
    WorldBounce,
    WorldColor,
    WorldAccent,
    WorldAccent2,
    WorldGlow,
    WorldEnhancedGlow,
    WorldGlowStrength,
    WorldGlowSize,
    WorldDemoBtn,
    // Visualizer tab
    VizEnabled,
    VizSource,
    VizStyle,
    VizStereo,
    VizDirection,
    VizColor,
    VizPeaks,
    VizHideIdle,
    VizAutoSens,
    VizSensitivity,
    VizSmoothing,
    VizBars,
    VizBarWidth,
    VizGap,
    VizHeight,
    VizScale,
    VizBg,
    VizOpacity,
    VizAnchor,
    VizOffsetX,
    VizOffsetY,
    // Cinema tab
    CineShots,
    CineSpeed,
    CineDistance,
    CineHeight,
    CineShotSec,
    CineFov,
    CineMusic,
    CineMusicStrength,
    CineCuts,
    CineBars,
    CineKeepHud,
    CineExitOnMove,
    CinePreviewBtn,
    // Keys & General actions
    Hotkey(HotkeyAction),
    ResetPlayerBtn,
    ResetKeysBtn,
    ResetHudBtn,
    OpenFolderBtn,
    GuideBtn,
}

enum ControlKind {
    Toggle(bool),
    Enum(String),
    Slider {
        min: f32,
        max: f32,
        step: f32,
        val: f32,
        text: String,
    },
    Action(String),
    TextField,
}

struct Row {
    id: SettingId,
    label: String,
    hint: String,
    kind: ControlKind,
}

#[derive(Clone, Debug)]
pub struct HudConfigScreenState {
    pub tab: ConfigTab,
    scroll: f32,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    list_x: i32,
    list_y: i32,
    list_w: i32,
    list_h: i32,
    dragging_slider: Option<(SettingId, f32, f32, f32)>,
    capturing_hotkey: Option<HotkeyAction>,
    musixmatch_box: TextInput,
    tab_rects: Vec<(ConfigTab, [i32; 4])>,
    back_rect: [i32; 4],
    edit_rect: [i32; 4],
    defaults_rect: [i32; 4],
    world_peek_until: Option<std::time::Instant>,
}

impl HudConfigScreenState {
    pub fn new(tab: ConfigTab) -> Self {
        Self {
            tab,
            scroll: 0.0,
            px: 0,
            py: 0,
            pw: 580,
            ph: 320,
            list_x: 0,
            list_y: 0,
            list_w: 300,
            list_h: 220,
            dragging_slider: None,
            capturing_hotkey: None,
            musixmatch_box: TextInput::new("Musixmatch token…", 512),
            tab_rects: Vec::new(),
            back_rect: [0; 4],
            edit_rect: [0; 4],
            defaults_rect: [0; 4],
            world_peek_until: None,
        }
    }

    pub fn is_text_focused(&self) -> bool {
        self.musixmatch_box.focused
    }

    pub fn is_listening_hotkey(&self) -> bool {
        self.capturing_hotkey.is_some()
    }

    pub fn init(&mut self, ctl: &PlayerController, sw: i32, sh: i32) {
        self.pw = (sw - 16).clamp(320, 620);
        self.ph = (sh - 20).clamp(240, 350);
        self.px = (sw - self.pw) / 2;
        self.py = (sh - self.ph) / 2;
        self.musixmatch_box
            .set_value(ctl.player_config().musixmatch_token);
        let tab_y = self.py + 28;
        let avail_w = (self.pw - 28).max(180);
        let metrics = crate::ui::MonoMetrics;
        let natural: Vec<f32> = ConfigTab::ALL
            .iter()
            .map(|t| (metrics.width(&t.title()) + 12.0).max(34.0))
            .collect();
        let gap = 3;
        let single_need: f32 =
            natural.iter().sum::<f32>() + (ConfigTab::ALL.len() as i32 - 1) as f32 * gap as f32;
        self.tab_rects.clear();
        let two_rows = avail_w < 460 || single_need > avail_w as f32 * 1.18;
        if !two_rows {
            let gap_total = (ConfigTab::ALL.len() as i32 - 1) * gap;
            let scale = (avail_w - gap_total).max(90) as f32 / natural.iter().sum::<f32>().max(1.0);
            let mut x = self.px + 14;
            for (i, t) in ConfigTab::ALL.iter().enumerate() {
                let w = if i + 1 == ConfigTab::ALL.len() {
                    (self.px + 14 + avail_w - x).max(28)
                } else {
                    (natural[i] * scale).round() as i32
                };
                self.tab_rects.push((*t, [x, tab_y, w, 17]));
                x += w + gap;
            }
            self.list_y = self.py + 50;
        } else {
            let split = 5;
            for (row_idx, chunk) in [0..split, split..ConfigTab::ALL.len()].iter().enumerate() {
                let slice_nat = &natural[chunk.clone()];
                let gap_total = (chunk.len() as i32 - 1) * gap;
                let scale =
                    (avail_w - gap_total).max(60) as f32 / slice_nat.iter().sum::<f32>().max(1.0);
                let mut x = self.px + 14;
                let ry = tab_y + row_idx as i32 * 19;
                for (offset, idx) in chunk.clone().enumerate() {
                    let w = if offset + 1 == chunk.len() {
                        (self.px + 14 + avail_w - x).max(32)
                    } else {
                        (natural[idx] * scale).round() as i32
                    };
                    self.tab_rects.push((ConfigTab::ALL[idx], [x, ry, w, 16]));
                    x += w + gap;
                }
            }
            self.list_y = self.py + 69;
        }
        self.list_x = self.px + 14;
        self.list_w = self.pw - 28;
        self.list_h = self.py + self.ph - 32 - self.list_y;
        let by = self.py + self.ph - 26;
        self.back_rect = [self.px + 14, by, 72, 18];
        self.edit_rect = [self.px + self.pw - 14 - 160, by, 76, 18];
        self.defaults_rect = [self.px + self.pw - 14 - 78, by, 78, 18];
    }

    fn rows(&self, ctl: &PlayerController, cinema_active: bool) -> Vec<Row> {
        let p = ctl.player_config();
        let h = ctl.hud_config();
        let hk = ctl.hotkey_config();
        let mut r = Vec::new();
        macro_rules! tog {
            ($id:expr, $b:expr, $lk:expr) => {
                r.push(Row {
                    id: $id,
                    label: i18n::get(&format!("set.{}", $lk)),
                    hint: i18n::get(&format!("hint.{}", $lk)),
                    kind: ControlKind::Toggle($b),
                });
            };
        }
        macro_rules! en {
            ($id:expr, $val_key:expr, $lk:expr) => {
                r.push(Row {
                    id: $id,
                    label: i18n::get(&format!("set.{}", $lk)),
                    hint: i18n::get(&format!("hint.{}", $lk)),
                    kind: ControlKind::Enum(i18n::get($val_key)),
                });
            };
        }
        macro_rules! sl {
            ($id:expr, $lk:expr, $min:expr, $max:expr, $step:expr, $val:expr, $txt:expr) => {
                r.push(Row {
                    id: $id,
                    label: i18n::get(&format!("set.{}", $lk)),
                    hint: i18n::get(&format!("hint.{}", $lk)),
                    kind: ControlKind::Slider {
                        min: $min,
                        max: $max,
                        step: $step,
                        val: $val,
                        text: $txt,
                    },
                });
            };
        }

        match self.tab {
            ConfigTab::Widget => {
                tog!(SettingId::WidgetEnabled, h.enabled, "widget_enabled");
                en!(SettingId::Layout, h.layout.key(), "layout");
                en!(SettingId::ShowMode, h.show_mode.key(), "show_mode");
                tog!(
                    SettingId::ShowOnSongStart,
                    h.show_on_song_start,
                    "show_on_start"
                );
                tog!(SettingId::ShowOnSongEnd, h.show_on_song_end, "show_on_end");
                en!(SettingId::HudAccent, h.accent.key(), "accent");
                r.push(Row {
                    id: SettingId::HudCustomColor,
                    label: i18n::get("set.custom_accent"),
                    hint: i18n::get("hint.custom_accent"),
                    kind: ControlKind::Action(if h.custom_accent >= 0 {
                        format!("#{:06X}", h.custom_accent)
                    } else {
                        i18n::get("btn.pick")
                    }),
                });
                sl!(
                    SettingId::HudScale,
                    "scale",
                    0.5,
                    2.0,
                    0.05,
                    h.scale,
                    format!("{:.2}x", h.scale)
                );
                sl!(
                    SettingId::BgOpacity,
                    "background",
                    0.0,
                    100.0,
                    5.0,
                    h.background_opacity as f32,
                    format!("{}%", h.background_opacity)
                );
                tog!(SettingId::HudCover, h.show_cover, "cover");
                tog!(SettingId::HudTimestamps, h.show_timestamps, "time");
                tog!(SettingId::HudWaveform, h.show_waveform, "equalizer");
                en!(SettingId::HudAnchor, h.anchor.key(), "position");
                sl!(
                    SettingId::HudOffsetX,
                    "offset_x",
                    -100.0,
                    400.0,
                    1.0,
                    h.offset_x as f32,
                    format!("{}px", h.offset_x)
                );
                sl!(
                    SettingId::HudOffsetY,
                    "offset_y",
                    -100.0,
                    400.0,
                    1.0,
                    h.offset_y as f32,
                    format!("{}px", h.offset_y)
                );
            }
            ConfigTab::Lyrics => {
                // PRO MAX Cascade & DSP Auto-Aligner highlighted at top
                en!(
                    SettingId::LyricsSourceMode,
                    p.lyrics_source_mode.key(),
                    "lyrics_source_mode"
                );
                tog!(SettingId::DspAutoAlign, p.dsp_auto_align, "dsp_auto_align");
                en!(SettingId::LyricsMode, h.lyrics_mode.key(), "lyrics_mode");
                tog!(SettingId::CardLyrics, h.card_lyrics, "card_lyrics");
                tog!(
                    SettingId::WordKaraoke,
                    h.lyrics_word_karaoke,
                    "word_karaoke"
                );
                en!(SettingId::LyricsEnter, h.lyrics_enter.key(), "lyrics_enter");
                en!(SettingId::LyricsExit, h.lyrics_exit.key(), "lyrics_exit");
                sl!(
                    SettingId::LyricsLinger,
                    "world_linger",
                    1.0,
                    15.0,
                    1.0,
                    h.lyrics_linger as f32,
                    format!("{}s", h.lyrics_linger)
                );
                sl!(
                    SettingId::LyricsLetterMs,
                    "world_letter_ms",
                    0.0,
                    120.0,
                    5.0,
                    h.lyrics_letter_ms as f32,
                    format!("{}ms", h.lyrics_letter_ms)
                );
                sl!(
                    SettingId::LyricsShiftMs,
                    "shift",
                    -3000.0,
                    3000.0,
                    100.0,
                    h.lyrics_shift_ms as f32,
                    format!("{:+}ms", h.lyrics_shift_ms)
                );
                sl!(
                    SettingId::LyricsScale,
                    "scale",
                    0.5,
                    2.0,
                    0.05,
                    h.lyrics_scale,
                    format!("{:.2}x", h.lyrics_scale)
                );
                sl!(
                    SettingId::LyricsWidth,
                    "width",
                    200.0,
                    600.0,
                    10.0,
                    h.lyrics_width as f32,
                    format!("{}px", h.lyrics_width)
                );
                sl!(
                    SettingId::LyricsLines,
                    "lines",
                    3.0,
                    9.0,
                    2.0,
                    h.lyrics_lines as f32,
                    format!("{}", h.lyrics_lines)
                );
                sl!(
                    SettingId::LyricsBg,
                    "background",
                    0.0,
                    100.0,
                    5.0,
                    h.lyrics_background as f32,
                    format!("{}%", h.lyrics_background)
                );
                tog!(SettingId::LyricsAccent, h.lyrics_accent, "lyrics_accent");
                tog!(SettingId::LyricsShadow, h.lyrics_shadow, "text_shadow");
                en!(SettingId::LyricsAnchor, h.lyrics_anchor.key(), "position");
                sl!(
                    SettingId::LyricsOffsetX,
                    "offset_x",
                    -100.0,
                    400.0,
                    1.0,
                    h.lyrics_offset_x as f32,
                    format!("{}px", h.lyrics_offset_x)
                );
                sl!(
                    SettingId::LyricsOffsetY,
                    "offset_y",
                    -100.0,
                    400.0,
                    1.0,
                    h.lyrics_offset_y as f32,
                    format!("{}px", h.lyrics_offset_y)
                );
                tog!(SettingId::OnlineLyrics, p.online_lyrics, "online_lyrics");
                tog!(SettingId::AmllLyrics, p.amll_lyrics, "amll_lyrics");
                tog!(SettingId::NeteaseLyrics, p.netease_lyrics, "netease_lyrics");
                tog!(SettingId::QqLyrics, p.qq_lyrics, "qq_lyrics");
                tog!(SettingId::KugouLyrics, p.kugou_lyrics, "kugou_lyrics");
                tog!(SettingId::BetterLyrics, p.better_lyrics, "better_lyrics");
                r.push(Row {
                    id: SettingId::MusixmatchToken,
                    label: i18n::get("set.musixmatch_token"),
                    hint: i18n::get("hint.musixmatch_token"),
                    kind: ControlKind::TextField,
                });
            }
            ConfigTab::World => {
                tog!(SettingId::WorldLyrics, h.world_lyrics, "world_enabled");
                tog!(
                    SettingId::LyricsWithWorld,
                    h.lyrics_with_world,
                    "world_with_hud"
                );
                en!(SettingId::WorldMode, h.world_mode.key(), "world_mode");
                en!(SettingId::FollowStyle, h.world_follow_style.key(), "follow");
                en!(SettingId::WorldLayout, h.world_layout.key(), "word_layout");
                sl!(
                    SettingId::WorldScale,
                    "size",
                    0.5,
                    2.5,
                    0.05,
                    h.world_scale,
                    format!("{:.2}x", h.world_scale)
                );
                sl!(
                    SettingId::WorldDistance,
                    "distance",
                    3.0,
                    30.0,
                    0.5,
                    h.world_distance,
                    format!("{:.1}m", h.world_distance)
                );
                sl!(
                    SettingId::WorldHeight,
                    "height",
                    -1.5,
                    3.0,
                    0.1,
                    h.world_height,
                    format!("{:.1}m", h.world_height)
                );
                sl!(
                    SettingId::WorldSpread,
                    "world_spread",
                    15.0,
                    90.0,
                    5.0,
                    h.world_spread,
                    format!("{:.0}°", h.world_spread)
                );
                tog!(
                    SettingId::WorldSeeThrough,
                    h.world_see_through,
                    "see_through"
                );
                tog!(SettingId::WorldShadow, h.world_shadow, "text_shadow");
                en!(SettingId::WorldAnim, h.world_anim.key(), "world_anim");
                en!(SettingId::WorldUnit, h.world_unit.key(), "world_unit");
                en!(SettingId::WorldExit, h.world_exit.key(), "world_exit");
                sl!(
                    SettingId::WorldLinger,
                    "world_linger",
                    1.0,
                    20.0,
                    1.0,
                    h.world_linger as f32,
                    format!("{}s", h.world_linger)
                );
                sl!(
                    SettingId::WorldLetterMs,
                    "world_letter_ms",
                    5.0,
                    150.0,
                    5.0,
                    h.world_letter_ms as f32,
                    format!("{}ms", h.world_letter_ms)
                );
                sl!(
                    SettingId::WorldAnimSpeed,
                    "world_speed",
                    25.0,
                    300.0,
                    5.0,
                    h.world_anim_speed as f32,
                    format!("{}%", h.world_anim_speed)
                );
                sl!(
                    SettingId::WorldBounce,
                    "world_bounce",
                    0.0,
                    300.0,
                    10.0,
                    h.world_bounce as f32,
                    format!("{}%", h.world_bounce)
                );
                en!(SettingId::WorldColor, h.world_color.key(), "world_color");
                en!(SettingId::WorldAccent, h.world_accent.key(), "world_accent");
                en!(
                    SettingId::WorldAccent2,
                    h.world_accent2.key(),
                    "world_accent2"
                );
                en!(SettingId::WorldGlow, h.world_glow.key(), "world_glow");
                tog!(
                    SettingId::WorldEnhancedGlow,
                    h.world_enhanced_glow,
                    "world_enhanced_glow"
                );
                sl!(
                    SettingId::WorldGlowStrength,
                    "world_glow_strength",
                    0.0,
                    100.0,
                    5.0,
                    h.world_glow_strength as f32,
                    format!("{}%", h.world_glow_strength)
                );
                sl!(
                    SettingId::WorldGlowSize,
                    "world_glow_size",
                    50.0,
                    250.0,
                    10.0,
                    h.world_glow_size as f32,
                    format!("{}%", h.world_glow_size)
                );
                r.push(Row {
                    id: SettingId::WorldDemoBtn,
                    label: i18n::get("set.world_demo"),
                    hint: i18n::get("hint.demo"),
                    kind: ControlKind::Action(i18n::get("btn.demo")),
                });
            }
            ConfigTab::Volume => {
                tog!(SettingId::VolEnabled, h.volume_enabled, "volume_enabled");
                en!(SettingId::VolStyle, h.volume_style.key(), "volume_style");
                sl!(
                    SettingId::VolSeconds,
                    "seconds",
                    1.0,
                    10.0,
                    1.0,
                    h.volume_seconds as f32,
                    format!("{}s", h.volume_seconds)
                );
                tog!(SettingId::VolPercent, h.volume_percent, "percent");
                sl!(
                    SettingId::VolScale,
                    "scale",
                    0.5,
                    2.0,
                    0.05,
                    h.volume_scale,
                    format!("{:.2}x", h.volume_scale)
                );
                sl!(
                    SettingId::VolBg,
                    "background",
                    0.0,
                    100.0,
                    5.0,
                    h.volume_background as f32,
                    format!("{}%", h.volume_background)
                );
                en!(SettingId::VolAnchor, h.volume_anchor.key(), "position");
                sl!(
                    SettingId::VolOffsetX,
                    "offset_x",
                    -100.0,
                    400.0,
                    1.0,
                    h.volume_offset_x as f32,
                    format!("{}px", h.volume_offset_x)
                );
                sl!(
                    SettingId::VolOffsetY,
                    "offset_y",
                    -100.0,
                    400.0,
                    1.0,
                    h.volume_offset_y as f32,
                    format!("{}px", h.volume_offset_y)
                );
            }
            ConfigTab::Visualizer => {
                tog!(SettingId::VizEnabled, h.viz_enabled, "viz_enabled");
                en!(SettingId::VizSource, h.viz_source.key(), "viz_source");
                en!(SettingId::VizStyle, h.viz_style.key(), "viz_style");
                tog!(SettingId::VizStereo, h.viz_stereo, "viz_stereo");
                en!(
                    SettingId::VizDirection,
                    h.viz_direction.key(),
                    "viz_direction"
                );
                en!(SettingId::VizColor, h.viz_color.key(), "viz_color");
                tog!(SettingId::VizPeaks, h.viz_peaks, "viz_peaks");
                tog!(SettingId::VizHideIdle, h.viz_hide_idle, "viz_hide_idle");
                tog!(SettingId::VizAutoSens, h.viz_auto_sens, "viz_autosens");
                sl!(
                    SettingId::VizSensitivity,
                    "viz_sensitivity",
                    10.0,
                    400.0,
                    10.0,
                    h.viz_sensitivity as f32,
                    format!("{}%", h.viz_sensitivity)
                );
                sl!(
                    SettingId::VizSmoothing,
                    "viz_smoothing",
                    0.0,
                    95.0,
                    5.0,
                    h.viz_smoothing as f32,
                    format!("{}%", h.viz_smoothing)
                );
                sl!(
                    SettingId::VizBars,
                    "viz_bars",
                    4.0,
                    96.0,
                    2.0,
                    h.viz_bars as f32,
                    format!("{}", h.viz_bars)
                );
                sl!(
                    SettingId::VizBarWidth,
                    "viz_bar_width",
                    1.0,
                    10.0,
                    1.0,
                    h.viz_bar_width as f32,
                    format!("{}px", h.viz_bar_width)
                );
                sl!(
                    SettingId::VizGap,
                    "viz_gap",
                    0.0,
                    6.0,
                    1.0,
                    h.viz_gap as f32,
                    format!("{}px", h.viz_gap)
                );
                sl!(
                    SettingId::VizHeight,
                    "viz_height",
                    8.0,
                    160.0,
                    4.0,
                    h.viz_height as f32,
                    format!("{}px", h.viz_height)
                );
                sl!(
                    SettingId::VizScale,
                    "scale",
                    0.5,
                    2.0,
                    0.05,
                    h.viz_scale,
                    format!("{:.2}x", h.viz_scale)
                );
                sl!(
                    SettingId::VizBg,
                    "background",
                    0.0,
                    100.0,
                    5.0,
                    h.viz_background as f32,
                    format!("{}%", h.viz_background)
                );
                sl!(
                    SettingId::VizOpacity,
                    "viz_opacity",
                    5.0,
                    100.0,
                    5.0,
                    h.viz_opacity as f32,
                    format!("{}%", h.viz_opacity)
                );
                en!(SettingId::VizAnchor, h.viz_anchor.key(), "position");
                sl!(
                    SettingId::VizOffsetX,
                    "offset_x",
                    -100.0,
                    400.0,
                    1.0,
                    h.viz_offset_x as f32,
                    format!("{}px", h.viz_offset_x)
                );
                sl!(
                    SettingId::VizOffsetY,
                    "offset_y",
                    -100.0,
                    400.0,
                    1.0,
                    h.viz_offset_y as f32,
                    format!("{}px", h.viz_offset_y)
                );
            }
            ConfigTab::Cinema => {
                r.push(Row {
                    id: SettingId::CinePreviewBtn,
                    label: i18n::get("set.cine_preview"),
                    hint: i18n::get("hint.cine_preview"),
                    kind: ControlKind::Action(if cinema_active {
                        i18n::get("btn.stop")
                    } else {
                        i18n::get("btn.start")
                    }),
                });
                en!(SettingId::CineShots, h.cine_shots.key(), "cine_shots");
                sl!(
                    SettingId::CineSpeed,
                    "cine_speed",
                    25.0,
                    300.0,
                    5.0,
                    h.cine_speed as f32,
                    format!("{}%", h.cine_speed)
                );
                sl!(
                    SettingId::CineDistance,
                    "cine_distance",
                    2.0,
                    14.0,
                    0.5,
                    h.cine_distance,
                    format!("{:.1}m", h.cine_distance)
                );
                sl!(
                    SettingId::CineHeight,
                    "cine_height",
                    -1.0,
                    5.0,
                    0.2,
                    h.cine_height,
                    format!("{:.1}m", h.cine_height)
                );
                sl!(
                    SettingId::CineShotSec,
                    "cine_shot_sec",
                    2.0,
                    20.0,
                    1.0,
                    h.cine_shot_sec as f32,
                    format!("{}s", h.cine_shot_sec)
                );
                sl!(
                    SettingId::CineFov,
                    "cine_fov",
                    -30.0,
                    30.0,
                    2.0,
                    h.cine_fov as f32,
                    format!("{:+}°", h.cine_fov)
                );
                tog!(SettingId::CineMusic, h.cine_music, "cine_music");
                sl!(
                    SettingId::CineMusicStrength,
                    "cine_music_strength",
                    0.0,
                    200.0,
                    10.0,
                    h.cine_music_strength as f32,
                    format!("{}%", h.cine_music_strength)
                );
                en!(SettingId::CineCuts, h.cine_cuts.key(), "cine_cuts");
                tog!(SettingId::CineBars, h.cine_bars, "cine_bars");
                tog!(
                    SettingId::CineKeepHud,
                    h.cine_keep_music_hud,
                    "cine_keep_music_hud"
                );
                tog!(
                    SettingId::CineExitOnMove,
                    h.cine_exit_on_move,
                    "cine_exit_on_move"
                );
            }
            ConfigTab::Player => {
                tog!(SettingId::SyncAccent, p.sync_accent, "sync_accent");
                en!(SettingId::PlayerAccent, p.accent.key(), "accent");
                r.push(Row {
                    id: SettingId::PlayerCustomColor,
                    label: i18n::get("set.custom_accent"),
                    hint: i18n::get("hint.custom_accent"),
                    kind: ControlKind::Action(if p.custom_accent >= 0 {
                        format!("#{:06X}", p.custom_accent)
                    } else {
                        i18n::get("btn.pick")
                    }),
                });
                sl!(
                    SettingId::WindowScale,
                    "window_size",
                    70.0,
                    150.0,
                    5.0,
                    p.window_scale as f32,
                    format!("{}%", p.window_scale)
                );
                sl!(
                    SettingId::CornerRadius,
                    "corner",
                    0.0,
                    16.0,
                    1.0,
                    p.corner_radius as f32,
                    format!("{}px", p.corner_radius)
                );
                en!(SettingId::WindowAnim, p.window_anim.key(), "window_anim");
                sl!(
                    SettingId::Tint,
                    "tint",
                    0.0,
                    3.0,
                    1.0,
                    p.tint as f32,
                    format!("{}%", p.tint * 5)
                );
                sl!(
                    SettingId::WindowOpacity,
                    "opacity",
                    20.0,
                    100.0,
                    5.0,
                    p.window_opacity as f32,
                    format!("{}%", p.window_opacity)
                );
                sl!(
                    SettingId::Dim,
                    "dim",
                    0.0,
                    100.0,
                    5.0,
                    p.dim as f32,
                    format!("{}%", p.dim)
                );
                tog!(SettingId::ShowCover, p.show_cover, "cover");
                sl!(
                    SettingId::CoverPx,
                    "cover_size",
                    40.0,
                    140.0,
                    4.0,
                    p.cover_px as f32,
                    format!("{}px", p.cover_px)
                );
                tog!(SettingId::ShowLyrics, p.show_lyrics, "lyrics_in_player");
                tog!(SettingId::AccentText, p.accent_text, "accent_title");
                r.push(Row {
                    id: SettingId::ResetPlayerBtn,
                    label: i18n::get("btn.reset_player"),
                    hint: i18n::get("hint.reset_player"),
                    kind: ControlKind::Action(i18n::get("btn.reset")),
                });
            }
            ConfigTab::Keys => {
                for act in HotkeyAction::ALL {
                    let lbl = if self.capturing_hotkey == Some(act) {
                        i18n::get("keys.press")
                    } else {
                        HotkeyConfig::display_name(hk.key_of(act))
                    };
                    r.push(Row {
                        id: SettingId::Hotkey(act),
                        label: i18n::get(act.label_key()),
                        hint: i18n::get("hint.key"),
                        kind: ControlKind::Action(lbl),
                    });
                }
                r.push(Row {
                    id: SettingId::ResetKeysBtn,
                    label: i18n::get("btn.reset_keys"),
                    hint: i18n::get("hint.reset_keys"),
                    kind: ControlKind::Action(i18n::get("btn.reset")),
                });
            }
            ConfigTab::General => {
                tog!(SettingId::Master, h.master, "master");
                tog!(SettingId::HideWithGui, h.hide_with_gui, "hide_with_gui");
                tog!(SettingId::HideOnTab, h.hide_on_tab, "hide_on_tab");
                tog!(SettingId::DuckInactive, p.duck_when_inactive, "duck");
                tog!(SettingId::Prefetch, p.prefetch, "prefetch");
                sl!(
                    SettingId::PrefetchCount,
                    "prefetch_count",
                    1.0,
                    8.0,
                    1.0,
                    p.prefetch_count as f32,
                    format!("{}", p.prefetch_count)
                );
                tog!(SettingId::DiskCache, p.disk_cache, "disk_cache");
                tog!(SettingId::DiscordPresence, p.discord_presence, "discord");
                r.push(Row {
                    id: SettingId::OpenFolderBtn,
                    label: i18n::get("btn.open_folder"),
                    hint: i18n::get("hint.open_folder"),
                    kind: ControlKind::Action(i18n::get("btn.open_folder")),
                });
                r.push(Row {
                    id: SettingId::GuideBtn,
                    label: i18n::get("btn.guide"),
                    hint: i18n::get("hint.guide"),
                    kind: ControlKind::Action(i18n::get("btn.guide_short")),
                });
                r.push(Row {
                    id: SettingId::ResetHudBtn,
                    label: i18n::get("btn.reset_hud"),
                    hint: i18n::get("hint.reset_hud"),
                    kind: ControlKind::Action(i18n::get("btn.reset")),
                });
            }
        }
        r
    }

    fn apply_click(&mut self, id: SettingId, ctl: &PlayerController) -> ScreenRoute {
        match id {
            SettingId::PlayerCustomColor => {
                let p = ctl.player_config();
                let h = ctl.hud_config();
                return ScreenRoute::OpenPicker {
                    initial_rgb: p.resolved_accent_rgb(&h),
                    target: PickerTarget::PlayerAccent,
                };
            }
            SettingId::HudCustomColor => {
                let h = ctl.hud_config();
                return ScreenRoute::OpenPicker {
                    initial_rgb: h.accent_rgb(),
                    target: PickerTarget::HudAccent,
                };
            }
            SettingId::SyncAccent => ctl.update_player_config(|p| p.sync_accent = !p.sync_accent),
            SettingId::PlayerAccent => ctl.update_player_config(|p| {
                p.accent = cycle(&Accent::ALL, p.accent);
                p.custom_accent = -1;
            }),
            SettingId::WindowAnim => {
                ctl.update_player_config(|p| p.window_anim = cycle(&WindowAnim::ALL, p.window_anim))
            }
            SettingId::ShowCover => ctl.update_player_config(|p| p.show_cover = !p.show_cover),
            SettingId::ShowLyrics => ctl.update_player_config(|p| p.show_lyrics = !p.show_lyrics),
            SettingId::AccentText => ctl.update_player_config(|p| p.accent_text = !p.accent_text),
            SettingId::DuckInactive => {
                ctl.update_player_config(|p| p.duck_when_inactive = !p.duck_when_inactive)
            }
            SettingId::Prefetch => ctl.update_player_config(|p| p.prefetch = !p.prefetch),
            SettingId::DiskCache => ctl.update_player_config(|p| p.disk_cache = !p.disk_cache),
            SettingId::DiscordPresence => {
                ctl.update_player_config(|p| p.discord_presence = !p.discord_presence)
            }
            SettingId::LyricsSourceMode => {
                ctl.cycle_lyrics_source_mode();
            }
            SettingId::OnlineLyrics => {
                ctl.update_player_config(|p| p.online_lyrics = !p.online_lyrics)
            }
            SettingId::AmllLyrics => ctl.update_player_config(|p| p.amll_lyrics = !p.amll_lyrics),
            SettingId::NeteaseLyrics => {
                ctl.update_player_config(|p| p.netease_lyrics = !p.netease_lyrics)
            }
            SettingId::QqLyrics => ctl.update_player_config(|p| p.qq_lyrics = !p.qq_lyrics),
            SettingId::KugouLyrics => {
                ctl.update_player_config(|p| p.kugou_lyrics = !p.kugou_lyrics)
            }
            SettingId::BetterLyrics => {
                ctl.update_player_config(|p| p.better_lyrics = !p.better_lyrics)
            }
            SettingId::DspAutoAlign => {
                ctl.update_player_config(|p| p.dsp_auto_align = !p.dsp_auto_align)
            }
            SettingId::Hotkey(act) => {
                self.capturing_hotkey = Some(act);
            }
            // HudConfig
            SettingId::Master => {
                ctl.toggle_hud_master();
            }
            SettingId::HideWithGui => ctl.update_hud_config(|h| h.hide_with_gui = !h.hide_with_gui),
            SettingId::HideOnTab => ctl.update_hud_config(|h| h.hide_on_tab = !h.hide_on_tab),
            SettingId::WidgetEnabled => ctl.update_hud_config(|h| h.enabled = !h.enabled),
            SettingId::Layout => {
                ctl.update_hud_config(|h| h.layout = cycle(&Layout::ALL, h.layout))
            }
            SettingId::ShowMode => {
                ctl.update_hud_config(|h| h.show_mode = cycle(&ShowMode::ALL, h.show_mode))
            }
            SettingId::ShowOnSongStart => {
                ctl.update_hud_config(|h| h.show_on_song_start = !h.show_on_song_start)
            }
            SettingId::ShowOnSongEnd => {
                ctl.update_hud_config(|h| h.show_on_song_end = !h.show_on_song_end)
            }
            SettingId::HudAccent => ctl.update_hud_config(|h| {
                h.accent = cycle(&Accent::ALL, h.accent);
                h.custom_accent = -1;
            }),
            SettingId::HudCover => ctl.update_hud_config(|h| h.show_cover = !h.show_cover),
            SettingId::HudTimestamps => {
                ctl.update_hud_config(|h| h.show_timestamps = !h.show_timestamps)
            }
            SettingId::HudWaveform => ctl.update_hud_config(|h| h.show_waveform = !h.show_waveform),
            SettingId::HudAnchor => {
                ctl.update_hud_config(|h| h.anchor = cycle(&Anchor::ALL, h.anchor))
            }
            SettingId::VolEnabled => {
                ctl.update_hud_config(|h| h.volume_enabled = !h.volume_enabled)
            }
            SettingId::VolStyle => {
                ctl.update_hud_config(|h| h.volume_style = cycle(&VolumeStyle::ALL, h.volume_style))
            }
            SettingId::VolPercent => {
                ctl.update_hud_config(|h| h.volume_percent = !h.volume_percent)
            }
            SettingId::VolAnchor => {
                ctl.update_hud_config(|h| h.volume_anchor = cycle(&Anchor::ALL, h.volume_anchor))
            }
            SettingId::LyricsMode => {
                ctl.update_hud_config(|h| h.lyrics_mode = cycle(&LyricsMode::ALL, h.lyrics_mode))
            }
            SettingId::CardLyrics => ctl.update_hud_config(|h| h.card_lyrics = !h.card_lyrics),
            SettingId::WordKaraoke => {
                ctl.update_hud_config(|h| h.lyrics_word_karaoke = !h.lyrics_word_karaoke)
            }
            SettingId::LyricsEnter => {
                ctl.update_hud_config(|h| h.lyrics_enter = cycle(&LyricsEnter::ALL, h.lyrics_enter))
            }
            SettingId::LyricsExit => {
                ctl.update_hud_config(|h| h.lyrics_exit = cycle(&WorldExit::ALL, h.lyrics_exit))
            }
            SettingId::LyricsAccent => {
                ctl.update_hud_config(|h| h.lyrics_accent = !h.lyrics_accent)
            }
            SettingId::LyricsShadow => {
                ctl.update_hud_config(|h| h.lyrics_shadow = !h.lyrics_shadow)
            }
            SettingId::LyricsAnchor => {
                ctl.update_hud_config(|h| h.lyrics_anchor = cycle(&Anchor::ALL, h.lyrics_anchor))
            }
            SettingId::WorldLyrics => {
                ctl.toggle_world_lyrics();
            }
            SettingId::LyricsWithWorld => {
                ctl.update_hud_config(|h| h.lyrics_with_world = !h.lyrics_with_world)
            }
            SettingId::WorldMode => {
                ctl.update_hud_config(|h| h.world_mode = cycle(&WorldMode::ALL, h.world_mode))
            }
            SettingId::FollowStyle => ctl.update_hud_config(|h| {
                h.world_follow_style = cycle(&FollowStyle::ALL, h.world_follow_style)
            }),
            SettingId::WorldLayout => {
                ctl.update_hud_config(|h| h.world_layout = cycle(&WorldLayout::ALL, h.world_layout))
            }
            SettingId::WorldSeeThrough => {
                ctl.update_hud_config(|h| h.world_see_through = !h.world_see_through)
            }
            SettingId::WorldShadow => ctl.update_hud_config(|h| h.world_shadow = !h.world_shadow),
            SettingId::WorldAnim => {
                ctl.update_hud_config(|h| h.world_anim = cycle(&WorldAnim::ALL, h.world_anim))
            }
            SettingId::WorldUnit => {
                ctl.update_hud_config(|h| h.world_unit = cycle(&WorldUnit::ALL, h.world_unit))
            }
            SettingId::WorldExit => {
                ctl.update_hud_config(|h| h.world_exit = cycle(&WorldExit::ALL, h.world_exit))
            }
            SettingId::WorldColor => {
                ctl.update_hud_config(|h| h.world_color = cycle(&WorldColor::ALL, h.world_color))
            }
            SettingId::WorldAccent => {
                ctl.update_hud_config(|h| h.world_accent = cycle(&Accent::ALL, h.world_accent))
            }
            SettingId::WorldAccent2 => {
                ctl.update_hud_config(|h| h.world_accent2 = cycle(&Accent::ALL, h.world_accent2))
            }
            SettingId::WorldGlow => {
                ctl.update_hud_config(|h| h.world_glow = cycle(&WorldGlow::ALL, h.world_glow))
            }
            SettingId::WorldEnhancedGlow => {
                ctl.update_hud_config(|h| h.world_enhanced_glow = !h.world_enhanced_glow)
            }
            SettingId::VizEnabled => ctl.update_hud_config(|h| h.viz_enabled = !h.viz_enabled),
            SettingId::VizSource => {
                ctl.update_hud_config(|h| h.viz_source = cycle(&VizSource::ALL, h.viz_source))
            }
            SettingId::VizStyle => {
                ctl.update_hud_config(|h| h.viz_style = cycle(&VizStyle::ALL, h.viz_style))
            }
            SettingId::VizStereo => ctl.update_hud_config(|h| h.viz_stereo = !h.viz_stereo),
            SettingId::VizDirection => ctl.update_hud_config(|h| {
                h.viz_direction = cycle(&VizDirection::ALL, h.viz_direction)
            }),
            SettingId::VizColor => {
                ctl.update_hud_config(|h| h.viz_color = cycle(&VizColor::ALL, h.viz_color))
            }
            SettingId::VizPeaks => ctl.update_hud_config(|h| h.viz_peaks = !h.viz_peaks),
            SettingId::VizHideIdle => ctl.update_hud_config(|h| h.viz_hide_idle = !h.viz_hide_idle),
            SettingId::VizAutoSens => ctl.update_hud_config(|h| h.viz_auto_sens = !h.viz_auto_sens),
            SettingId::VizAnchor => {
                ctl.update_hud_config(|h| h.viz_anchor = cycle(&Anchor::ALL, h.viz_anchor))
            }
            SettingId::CineShots => {
                ctl.update_hud_config(|h| h.cine_shots = cycle(&CineShots::ALL, h.cine_shots))
            }
            SettingId::CineMusic => ctl.update_hud_config(|h| h.cine_music = !h.cine_music),
            SettingId::CineCuts => {
                ctl.update_hud_config(|h| h.cine_cuts = cycle(&CineCuts::ALL, h.cine_cuts))
            }
            SettingId::CineBars => ctl.update_hud_config(|h| h.cine_bars = !h.cine_bars),
            SettingId::CineKeepHud => {
                ctl.update_hud_config(|h| h.cine_keep_music_hud = !h.cine_keep_music_hud)
            }
            SettingId::CineExitOnMove => {
                ctl.update_hud_config(|h| h.cine_exit_on_move = !h.cine_exit_on_move)
            }
            SettingId::ResetPlayerBtn => {
                ctl.update_player_config(|p| *p = PlayerConfig::default());
            }
            SettingId::ResetKeysBtn => {
                self.capturing_hotkey = None;
                ctl.update_hotkey_config(|hk| *hk = HotkeyConfig::default());
            }
            SettingId::ResetHudBtn => {
                ctl.update_hud_config(|h| *h = HudConfig::default());
            }
            SettingId::OpenFolderBtn => {
                ctl.open_local_folder();
            }
            SettingId::GuideBtn => {
                return ScreenRoute::OpenHelp;
            }
            _ => {}
        }
        ScreenRoute::Stay
    }

    fn apply_slider(
        &self,
        id: SettingId,
        min: f32,
        max: f32,
        step: f32,
        mx: i32,
        ctl: &PlayerController,
    ) {
        let sx = self.list_x + self.list_w - 132;
        let sw = 80;
        let frac = ((mx - sx) as f32 / sw as f32).clamp(0.0, 1.0);
        let raw = min + frac * (max - min);
        let snapped = ((raw / step).round() * step).clamp(min, max);
        match id {
            SettingId::WindowScale => {
                ctl.update_player_config(|p| p.window_scale = snapped.round() as i32)
            }
            SettingId::CornerRadius => {
                ctl.update_player_config(|p| p.corner_radius = snapped.round() as i32)
            }
            SettingId::Tint => ctl.update_player_config(|p| p.tint = snapped.round() as i32),
            SettingId::WindowOpacity => {
                ctl.update_player_config(|p| p.window_opacity = snapped.round() as i32)
            }
            SettingId::Dim => ctl.update_player_config(|p| p.dim = snapped.round() as i32),
            SettingId::CoverPx => ctl.update_player_config(|p| p.cover_px = snapped.round() as i32),
            SettingId::PrefetchCount => {
                ctl.update_player_config(|p| p.prefetch_count = snapped.round() as i32)
            }
            SettingId::HudScale => ctl.update_hud_config(|h| h.scale = snapped),
            SettingId::BgOpacity => {
                ctl.update_hud_config(|h| h.background_opacity = snapped.round() as i32)
            }
            SettingId::HudOffsetX => ctl.update_hud_config(|h| h.offset_x = snapped.round() as i32),
            SettingId::HudOffsetY => ctl.update_hud_config(|h| h.offset_y = snapped.round() as i32),
            SettingId::VolSeconds => {
                ctl.update_hud_config(|h| h.volume_seconds = snapped.round() as i32)
            }
            SettingId::VolScale => ctl.update_hud_config(|h| h.volume_scale = snapped),
            SettingId::VolBg => {
                ctl.update_hud_config(|h| h.volume_background = snapped.round() as i32)
            }
            SettingId::VolOffsetX => {
                ctl.update_hud_config(|h| h.volume_offset_x = snapped.round() as i32)
            }
            SettingId::VolOffsetY => {
                ctl.update_hud_config(|h| h.volume_offset_y = snapped.round() as i32)
            }
            SettingId::LyricsLinger => {
                ctl.update_hud_config(|h| h.lyrics_linger = snapped.round() as i32)
            }
            SettingId::LyricsLetterMs => {
                ctl.update_hud_config(|h| h.lyrics_letter_ms = snapped.round() as i32)
            }
            SettingId::LyricsShiftMs => {
                ctl.update_hud_config(|h| h.lyrics_shift_ms = snapped.round() as i32)
            }
            SettingId::LyricsScale => ctl.update_hud_config(|h| h.lyrics_scale = snapped),
            SettingId::LyricsWidth => {
                ctl.update_hud_config(|h| h.lyrics_width = snapped.round() as i32)
            }
            SettingId::LyricsLines => {
                ctl.update_hud_config(|h| h.lyrics_lines = snapped.round() as i32)
            }
            SettingId::LyricsBg => {
                ctl.update_hud_config(|h| h.lyrics_background = snapped.round() as i32)
            }
            SettingId::LyricsOffsetX => {
                ctl.update_hud_config(|h| h.lyrics_offset_x = snapped.round() as i32)
            }
            SettingId::LyricsOffsetY => {
                ctl.update_hud_config(|h| h.lyrics_offset_y = snapped.round() as i32)
            }
            SettingId::WorldScale => ctl.update_hud_config(|h| h.world_scale = snapped),
            SettingId::WorldDistance => ctl.update_hud_config(|h| h.world_distance = snapped),
            SettingId::WorldHeight => ctl.update_hud_config(|h| h.world_height = snapped),
            SettingId::WorldSpread => ctl.update_hud_config(|h| h.world_spread = snapped),
            SettingId::WorldLinger => {
                ctl.update_hud_config(|h| h.world_linger = snapped.round() as i32)
            }
            SettingId::WorldLetterMs => {
                ctl.update_hud_config(|h| h.world_letter_ms = snapped.round() as i32)
            }
            SettingId::WorldAnimSpeed => {
                ctl.update_hud_config(|h| h.world_anim_speed = snapped.round() as i32)
            }
            SettingId::WorldBounce => {
                ctl.update_hud_config(|h| h.world_bounce = snapped.round() as i32)
            }
            SettingId::WorldGlowStrength => {
                ctl.update_hud_config(|h| h.world_glow_strength = snapped.round() as i32)
            }
            SettingId::WorldGlowSize => {
                ctl.update_hud_config(|h| h.world_glow_size = snapped.round() as i32)
            }
            SettingId::VizSensitivity => {
                ctl.update_hud_config(|h| h.viz_sensitivity = snapped.round() as i32)
            }
            SettingId::VizSmoothing => {
                ctl.update_hud_config(|h| h.viz_smoothing = snapped.round() as i32)
            }
            SettingId::VizBars => ctl.update_hud_config(|h| h.viz_bars = snapped.round() as i32),
            SettingId::VizBarWidth => {
                ctl.update_hud_config(|h| h.viz_bar_width = snapped.round() as i32)
            }
            SettingId::VizGap => ctl.update_hud_config(|h| h.viz_gap = snapped.round() as i32),
            SettingId::VizHeight => {
                ctl.update_hud_config(|h| h.viz_height = snapped.round() as i32)
            }
            SettingId::VizScale => ctl.update_hud_config(|h| h.viz_scale = snapped),
            SettingId::VizBg => {
                ctl.update_hud_config(|h| h.viz_background = snapped.round() as i32)
            }
            SettingId::VizOpacity => {
                ctl.update_hud_config(|h| h.viz_opacity = snapped.round() as i32)
            }
            SettingId::VizOffsetX => {
                ctl.update_hud_config(|h| h.viz_offset_x = snapped.round() as i32)
            }
            SettingId::VizOffsetY => {
                ctl.update_hud_config(|h| h.viz_offset_y = snapped.round() as i32)
            }
            SettingId::CineSpeed => {
                ctl.update_hud_config(|h| h.cine_speed = snapped.round() as i32)
            }
            SettingId::CineDistance => ctl.update_hud_config(|h| h.cine_distance = snapped),
            SettingId::CineHeight => ctl.update_hud_config(|h| h.cine_height = snapped),
            SettingId::CineShotSec => {
                ctl.update_hud_config(|h| h.cine_shot_sec = snapped.round() as i32)
            }
            SettingId::CineFov => ctl.update_hud_config(|h| h.cine_fov = snapped.round() as i32),
            SettingId::CineMusicStrength => {
                ctl.update_hud_config(|h| h.cine_music_strength = snapped.round() as i32)
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        g: &mut DrawList,
        metrics: &dyn TextMetrics,
        ctl: &PlayerController,
        sw: i32,
        sh: i32,
        mx: i32,
        my: i32,
        _dt: f32,
        cinema: &CinematicController,
    ) {
        let th = PlayerTheme::from_config(&ctl.player_config(), &ctl.hud_config());
        if let Some(until) = self.world_peek_until {
            if std::time::Instant::now() < until {
                let banner = i18n::get("world.demo_peek");
                let fit = metrics.ellipsize(&banner, (sw - 40).max(40) as f32);
                let tw = metrics.width(&fit);
                let bw = (tw.round() as i32 + 24).min(sw - 20);
                let bx = (sw - bw) / 2;
                let by = 14;
                shapes::fill(g, bx, by, bw, 20, 10, 0xCC11_1116);
                g.text(
                    fit,
                    (sw as f32 - tw) / 2.0,
                    (by + 6) as f32,
                    th.accent,
                    false,
                );
                return;
            }
            self.world_peek_until = None;
        }
        g.fill(0.0, 0.0, sw as f32, sh as f32, th.dim);
        th.window(g, self.px, self.py, self.pw, self.ph);

        g.text(
            format!("⚙ {}", i18n::get("settings")),
            (self.px + 14) as f32,
            (self.py + 9) as f32,
            th.accent,
            false,
        );

        for (t, r) in &self.tab_rects {
            let active = *t == self.tab;
            let hov = in_rect(*r, mx, my);
            let bg = if active {
                th.accent
            } else if hov {
                th.card_hover
            } else {
                th.btn_bg
            };
            shapes::fill(g, r[0], r[1], r[2], r[3], 4, bg);
            let title = metrics.ellipsize(&t.title(), (r[2] - 4).max(16) as f32);
            let tw = metrics.width(&title);
            g.text(
                title,
                r[0] as f32 + (r[2] as f32 - tw) / 2.0,
                r[1] as f32 + ((r[3] as f32 - metrics.line_height()) / 2.0).max(2.0),
                PlayerTheme::TEXT_MAIN,
                false,
            );
        }

        let rows = self.rows(ctl, cinema.active());
        let row_h = 24;
        let max_scroll = ((rows.len() as i32 * row_h) - self.list_h).max(0) as f32;
        self.scroll = self.scroll.clamp(0.0, max_scroll);

        g.push_scissor(
            self.list_x as f32,
            self.list_y as f32,
            self.list_w as f32,
            self.list_h as f32,
        );
        let mut hovered_hint: Option<String> = None;
        for (i, row) in rows.iter().enumerate() {
            let ry = self.list_y + i as i32 * row_h - self.scroll.round() as i32;
            if ry + row_h < self.list_y || ry > self.list_y + self.list_h {
                continue;
            }
            let bg = if mx >= self.list_x
                && mx < self.list_x + self.list_w
                && my >= ry
                && my < ry + row_h - 2
                && my >= self.list_y
                && my < self.list_y + self.list_h
            {
                if !row.hint.is_empty() {
                    hovered_hint = Some(row.hint.clone());
                }
                th.card_hover
            } else {
                th.card
            };
            shapes::fill(g, self.list_x, ry, self.list_w, row_h - 2, 4, bg);
            let lbl = metrics.ellipsize(&row.label, (self.list_w - 150).max(60) as f32);
            g.text(
                lbl,
                (self.list_x + 8) as f32,
                (ry + 7) as f32,
                PlayerTheme::TEXT_MAIN,
                false,
            );

            let rx = self.list_x + self.list_w - 132;
            match &row.kind {
                ControlKind::Toggle(on) => {
                    let bx = self.list_x + self.list_w - 40;
                    shapes::fill(
                        g,
                        bx,
                        ry + 5,
                        30,
                        12,
                        6,
                        if *on { th.accent } else { th.track_bg },
                    );
                    PlayerTheme::circle(
                        g,
                        if *on { bx + 24 } else { bx + 6 },
                        ry + 11,
                        4,
                        PlayerTheme::TEXT_MAIN,
                    );
                }
                ControlKind::Enum(txt) | ControlKind::Action(txt) => {
                    let bw = 122;
                    shapes::fill(g, rx, ry + 3, bw, 16, 4, th.btn_bg);
                    let clipped = metrics.ellipsize(txt, (bw - 10) as f32);
                    let tw = metrics.width(&clipped);
                    g.text(
                        clipped,
                        rx as f32 + (bw as f32 - tw) / 2.0,
                        (ry + 7) as f32,
                        PlayerTheme::TEXT_MAIN,
                        false,
                    );
                }
                ControlKind::Slider {
                    min,
                    max,
                    val,
                    text,
                    ..
                } => {
                    let sw_b = 80;
                    let sx = rx;
                    let sy = ry + 9;
                    let frac = ((*val - *min) / (*max - *min).max(0.001)).clamp(0.0, 1.0);
                    shapes::fill(g, sx, sy, sw_b, 4, 2, th.track_bg);
                    let fw = ((sw_b as f32) * frac).round() as i32;
                    if fw > 0 {
                        shapes::fill(g, sx, sy, fw, 4, 2, th.accent);
                    }
                    PlayerTheme::circle(g, sx + fw, sy + 2, 4, PlayerTheme::TEXT_MAIN);
                    g.text(
                        text,
                        (sx + sw_b + 6) as f32,
                        (ry + 7) as f32,
                        PlayerTheme::TEXT_MID,
                        false,
                    );
                }
                ControlKind::TextField => {
                    self.musixmatch_box.set_bounds(rx, ry + 2, 122, 18);
                    self.musixmatch_box.render(
                        g,
                        metrics,
                        th.radius,
                        th.accent,
                        th.track_bg,
                        th.field_bg,
                        PlayerTheme::TEXT_MAIN,
                        PlayerTheme::TEXT_DIM,
                    );
                }
            }
        }
        g.pop_scissor();

        if let Some(hint) = hovered_hint {
            let trimmed = metrics.ellipsize(&hint, (self.pw - 260).max(80) as f32);
            g.text(
                trimmed,
                (self.px + 94) as f32,
                (self.py + self.ph - 22) as f32,
                PlayerTheme::TEXT_DIM,
                false,
            );
        }

        draw_cfg_btn(
            g,
            metrics,
            self.back_rect,
            &format!("‹ {}", i18n::get("btn.back")),
            false,
            &th,
            mx,
            my,
        );
        draw_cfg_btn(
            g,
            metrics,
            self.edit_rect,
            &i18n::get("edit.button"),
            false,
            &th,
            mx,
            my,
        );
        draw_cfg_btn(
            g,
            metrics,
            self.defaults_rect,
            &i18n::get("btn.reset"),
            false,
            &th,
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
        if self
            .world_peek_until
            .take()
            .is_some_and(|u| std::time::Instant::now() < u)
        {
            return ScreenRoute::Stay;
        }
        if in_rect(self.back_rect, mx, my) {
            return ScreenRoute::Pop;
        }
        if in_rect(self.edit_rect, mx, my) {
            return ScreenRoute::OpenEdit;
        }
        if self.is_world_demo_row(mx, my, ctl) == Some(ConfigAction::SpawnWorldDemo) {
            self.world_peek_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(3200));
            return ScreenRoute::Stay;
        }
        if in_rect(self.defaults_rect, mx, my) {
            match self.tab {
                ConfigTab::Player => ctl.update_player_config(|p| *p = PlayerConfig::default()),
                ConfigTab::Keys => {
                    self.capturing_hotkey = None;
                    ctl.update_hotkey_config(|hk| *hk = HotkeyConfig::default());
                }
                _ => ctl.update_hud_config(|h| *h = HudConfig::default()),
            }
            return ScreenRoute::Stay;
        }
        for (t, r) in &self.tab_rects {
            if in_rect(*r, mx, my) {
                self.tab = *t;
                self.scroll = 0.0;
                self.capturing_hotkey = None;
                return ScreenRoute::Stay;
            }
        }
        if mx < self.list_x
            || mx >= self.list_x + self.list_w
            || my < self.list_y
            || my >= self.list_y + self.list_h
        {
            self.musixmatch_box.focused = false;
            return ScreenRoute::Stay;
        }
        let rows = self.rows(ctl, false);
        let row_h = 24;
        let idx = ((my - self.list_y + self.scroll.round() as i32) / row_h) as usize;
        if let Some(row) = rows.get(idx) {
            self.musixmatch_box.focused = matches!(row.id, SettingId::MusixmatchToken);
            if matches!(row.id, SettingId::MusixmatchToken) {
                return ScreenRoute::Stay;
            }
            if let ControlKind::Slider { min, max, step, .. } = row.kind {
                self.dragging_slider = Some((row.id, min, max, step));
                self.apply_slider(row.id, min, max, step, mx, ctl);
                return ScreenRoute::Stay;
            }
            return self.apply_click(row.id, ctl);
        }
        ScreenRoute::Stay
    }

    pub fn mouse_drag(&mut self, mx: i32, _my: i32, ctl: &PlayerController) {
        if let Some((id, min, max, step)) = self.dragging_slider {
            self.apply_slider(id, min, max, step, mx, ctl);
        }
    }

    pub fn mouse_up(&mut self) {
        self.dragging_slider = None;
    }

    pub fn mouse_scroll(&mut self, mx: i32, my: i32, sy: f32, _sw: i32, _sh: i32) {
        if mx >= self.list_x
            && mx < self.list_x + self.list_w
            && my >= self.list_y
            && my < self.list_y + self.list_h
        {
            self.scroll = (self.scroll - sy * 24.0).max(0.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn key_pressed(
        &mut self,
        key: ScreenKey,
        raw_key: &str,
        ctrl: bool,
        ctl: &PlayerController,
        world_lyrics: &mut WorldLyricsState,
        cinema: &mut CinematicController,
        metrics: &dyn TextMetrics,
        _sw: i32,
        _sh: i32,
    ) -> (bool, ScreenRoute) {
        if self
            .world_peek_until
            .take()
            .is_some_and(|u| std::time::Instant::now() < u)
        {
            return (true, ScreenRoute::Stay);
        }
        if let Some(act) = self.capturing_hotkey.take() {
            if key != ScreenKey::Escape && !raw_key.is_empty() {
                ctl.update_hotkey_config(|hk| hk.set(act, raw_key.to_owned()));
            }
            return (true, ScreenRoute::Stay);
        }
        if key == ScreenKey::Escape {
            return (true, ScreenRoute::Pop);
        }
        if self.musixmatch_box.handle_key(key, ctrl) {
            let val = self.musixmatch_box.value.clone();
            ctl.update_player_config(|p| p.musixmatch_token = val);
            return (true, ScreenRoute::Stay);
        }
        let _ = (world_lyrics, cinema, metrics);
        (false, ScreenRoute::Stay)
    }

    pub fn char_typed(&mut self, ch: char, ctl: &PlayerController) -> bool {
        if self.musixmatch_box.insert_char(ch) {
            let val = self.musixmatch_box.value.clone();
            ctl.update_player_config(|p| p.musixmatch_token = val);
            return true;
        }
        false
    }

    pub fn paste_text(&mut self, txt: &str, ctl: &PlayerController) -> bool {
        if self.musixmatch_box.insert_str(txt) {
            let val = self.musixmatch_box.value.clone();
            ctl.update_player_config(|p| p.musixmatch_token = val);
            return true;
        }
        false
    }

    pub fn is_world_demo_row(
        &self,
        mx: i32,
        my: i32,
        ctl: &PlayerController,
    ) -> Option<ConfigAction> {
        if self
            .world_peek_until
            .is_some_and(|u| std::time::Instant::now() < u)
        {
            return None;
        }
        if mx < self.list_x
            || mx >= self.list_x + self.list_w
            || my < self.list_y
            || my >= self.list_y + self.list_h
        {
            return None;
        }
        let rows = self.rows(ctl, false);
        let idx = ((my - self.list_y + self.scroll.round() as i32) / 24) as usize;
        match rows.get(idx)?.id {
            SettingId::WorldDemoBtn => Some(ConfigAction::SpawnWorldDemo),
            SettingId::CinePreviewBtn => Some(ConfigAction::ToggleCinemaPreview),
            _ => None,
        }
    }
}

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T) -> T {
    let idx = all.iter().position(|&v| v == cur).unwrap_or(0);
    all[(idx + 1) % all.len()]
}

fn in_rect(r: [i32; 4], mx: i32, my: i32) -> bool {
    mx >= r[0] && mx < r[0] + r[2] && my >= r[1] && my < r[1] + r[3]
}

#[allow(clippy::too_many_arguments)]
fn draw_cfg_btn(
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
