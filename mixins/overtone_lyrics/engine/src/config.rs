use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::model::{Account, Platform};

static DATA_ROOT: OnceLock<RwLock<PathBuf>> = OnceLock::new();

fn default_data_dir() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join("overtone_test_data")
    }
    #[cfg(not(test))]
    {
        PathBuf::from("overtone_data")
    }
}

pub fn set_data_dir(dir: PathBuf) {
    let _ = fs::create_dir_all(&dir);
    let lock = DATA_ROOT.get_or_init(|| RwLock::new(default_data_dir()));
    if let Ok(mut g) = lock.write() {
        *g = dir;
    }
}

pub fn data_dir() -> PathBuf {
    let lock = DATA_ROOT.get_or_init(|| RwLock::new(default_data_dir()));
    lock.read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| default_data_dir())
}

pub fn atomic_write_bytes(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Card,
    Classic,
    Bar,
    Compact,
    Hotbar,
    Vanilla,
}

impl Layout {
    pub const ALL: [Self; 6] = [
        Self::Card,
        Self::Classic,
        Self::Bar,
        Self::Compact,
        Self::Hotbar,
        Self::Vanilla,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Card => "opt.layout.card",
            Self::Classic => "opt.layout.classic",
            Self::Bar => "opt.layout.bar",
            Self::Compact => "opt.layout.compact",
            Self::Hotbar => "opt.layout.hotbar",
            Self::Vanilla => "opt.layout.vanilla",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowMode {
    Always,
    Seconds15,
    Seconds5,
}

impl ShowMode {
    pub const ALL: [Self; 3] = [Self::Always, Self::Seconds15, Self::Seconds5];
    pub fn key(self) -> &'static str {
        match self {
            Self::Always => "opt.showmode.always",
            Self::Seconds15 => "opt.showmode.seconds_15",
            Self::Seconds5 => "opt.showmode.seconds_5",
        }
    }
    pub fn duration_ms(self) -> u64 {
        match self {
            Self::Always => 0,
            Self::Seconds15 => 15_000,
            Self::Seconds5 => 5_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    MidLeft,
    Center,
    MidRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::MidLeft,
        Self::Center,
        Self::MidRight,
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::TopLeft => "opt.anchor.top_left",
            Self::TopCenter => "opt.anchor.top_center",
            Self::TopRight => "opt.anchor.top_right",
            Self::MidLeft => "opt.anchor.mid_left",
            Self::Center => "opt.anchor.center",
            Self::MidRight => "opt.anchor.mid_right",
            Self::BottomLeft => "opt.anchor.bottom_left",
            Self::BottomCenter => "opt.anchor.bottom_center",
            Self::BottomRight => "opt.anchor.bottom_right",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Raspberry,
    Yellow,
    Green,
    Blue,
    Purple,
    Orange,
    White,
}

impl Accent {
    pub const ALL: [Self; 7] = [
        Self::Raspberry,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Orange,
        Self::White,
    ];
    pub fn rgb(self) -> u32 {
        match self {
            Self::Raspberry => 0xE6397A,
            Self::Yellow => 0xFFCC00,
            Self::Green => 0x34D399,
            Self::Blue => 0x3B82F6,
            Self::Purple => 0xA855F7,
            Self::Orange => 0xFF6B2B,
            Self::White => 0xFFFFFF,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Raspberry => "opt.accent.raspberry",
            Self::Yellow => "opt.accent.yellow",
            Self::Green => "opt.accent.green",
            Self::Blue => "opt.accent.blue",
            Self::Purple => "opt.accent.purple",
            Self::Orange => "opt.accent.orange",
            Self::White => "opt.accent.white",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsMode {
    Off,
    Subtitle,
    Karaoke,
}

impl LyricsMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Subtitle, Self::Karaoke];
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "opt.lyricsmode.off",
            Self::Subtitle => "opt.lyricsmode.subtitle",
            Self::Karaoke => "opt.lyricsmode.karaoke",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsEnter {
    Slide,
    Rise,
    Pop,
    Type,
}

impl LyricsEnter {
    pub const ALL: [Self; 4] = [Self::Slide, Self::Rise, Self::Pop, Self::Type];
    pub fn key(self) -> &'static str {
        match self {
            Self::Slide => "opt.lyricsenter.slide",
            Self::Rise => "opt.lyricsenter.rise",
            Self::Pop => "opt.lyricsenter.pop",
            Self::Type => "opt.lyricsenter.type",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldMode {
    Follow,
    Fixed,
}

impl WorldMode {
    pub const ALL: [Self; 2] = [Self::Follow, Self::Fixed];
    pub fn key(self) -> &'static str {
        match self {
            Self::Follow => "opt.worldmode.follow",
            Self::Fixed => "opt.worldmode.fixed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowStyle {
    Screen,
    World,
}

impl FollowStyle {
    pub const ALL: [Self; 2] = [Self::Screen, Self::World];
    pub fn key(self) -> &'static str {
        match self {
            Self::Screen => "opt.followstyle.screen",
            Self::World => "opt.followstyle.world",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldLayout {
    Line,
    Scatter,
}

impl WorldLayout {
    pub const ALL: [Self; 2] = [Self::Line, Self::Scatter];
    pub fn key(self) -> &'static str {
        match self {
            Self::Line => "opt.worldlayout.line",
            Self::Scatter => "opt.worldlayout.scatter",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldAnim {
    Fly,
    Rise,
    Drop,
    Pop,
    Wave,
    Type,
    Spin,
    Glitch,
}

impl WorldAnim {
    pub const ALL: [Self; 8] = [
        Self::Fly,
        Self::Rise,
        Self::Drop,
        Self::Pop,
        Self::Wave,
        Self::Type,
        Self::Spin,
        Self::Glitch,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Fly => "opt.worldanim.fly",
            Self::Rise => "opt.worldanim.rise",
            Self::Drop => "opt.worldanim.drop",
            Self::Pop => "opt.worldanim.pop",
            Self::Wave => "opt.worldanim.wave",
            Self::Type => "opt.worldanim.type",
            Self::Spin => "opt.worldanim.spin",
            Self::Glitch => "opt.worldanim.glitch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldUnit {
    Words,
    Letters,
}

impl WorldUnit {
    pub const ALL: [Self; 2] = [Self::Words, Self::Letters];
    pub fn key(self) -> &'static str {
        match self {
            Self::Words => "opt.worldunit.words",
            Self::Letters => "opt.worldunit.letters",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldColor {
    Classic,
    Accent,
    Gradient,
    Rainbow,
    White,
}

impl WorldColor {
    pub const ALL: [Self; 5] = [
        Self::Classic,
        Self::Accent,
        Self::Gradient,
        Self::Rainbow,
        Self::White,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Classic => "opt.worldcolor.classic",
            Self::Accent => "opt.worldcolor.accent",
            Self::Gradient => "opt.worldcolor.gradient",
            Self::Rainbow => "opt.worldcolor.rainbow",
            Self::White => "opt.worldcolor.white",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldGlow {
    Off,
    Sung,
    All,
}

impl WorldGlow {
    pub const ALL: [Self; 3] = [Self::Off, Self::Sung, Self::All];
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "opt.worldglow.off",
            Self::Sung => "opt.worldglow.sung",
            Self::All => "opt.worldglow.all",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldExit {
    Fade,
    Fall,
    Shatter,
    Float,
    Wind,
    Shrink,
    Sink,
}

impl WorldExit {
    pub const ALL: [Self; 7] = [
        Self::Fade,
        Self::Fall,
        Self::Shatter,
        Self::Float,
        Self::Wind,
        Self::Shrink,
        Self::Sink,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Fade => "opt.worldexit.fade",
            Self::Fall => "opt.worldexit.fall",
            Self::Shatter => "opt.worldexit.shatter",
            Self::Float => "opt.worldexit.float",
            Self::Wind => "opt.worldexit.wind",
            Self::Shrink => "opt.worldexit.shrink",
            Self::Sink => "opt.worldexit.sink",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeStyle {
    Bar,
    Compact,
    Vertical,
}

impl VolumeStyle {
    pub const ALL: [Self; 3] = [Self::Bar, Self::Compact, Self::Vertical];
    pub fn key(self) -> &'static str {
        match self {
            Self::Bar => "opt.volumestyle.bar",
            Self::Compact => "opt.volumestyle.compact",
            Self::Vertical => "opt.volumestyle.vertical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VizSource {
    Auto,
    Mod,
    System,
}

impl VizSource {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Mod, Self::System];
    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "opt.vizsource.auto",
            Self::Mod => "opt.vizsource.mod",
            Self::System => "opt.vizsource.system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VizStyle {
    Bars,
    Blocks,
    Mirror,
    Wave,
}

impl VizStyle {
    pub const ALL: [Self; 4] = [Self::Bars, Self::Blocks, Self::Mirror, Self::Wave];
    pub fn key(self) -> &'static str {
        match self {
            Self::Bars => "opt.vizstyle.bars",
            Self::Blocks => "opt.vizstyle.blocks",
            Self::Mirror => "opt.vizstyle.mirror",
            Self::Wave => "opt.vizstyle.wave",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VizDirection {
    Auto,
    Up,
    Down,
}

impl VizDirection {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Up, Self::Down];
    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "opt.vizdirection.auto",
            Self::Up => "opt.vizdirection.up",
            Self::Down => "opt.vizdirection.down",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VizColor {
    Accent,
    Gradient,
    Rainbow,
    White,
}

impl VizColor {
    pub const ALL: [Self; 4] = [Self::Accent, Self::Gradient, Self::Rainbow, Self::White];
    pub fn key(self) -> &'static str {
        match self {
            Self::Accent => "opt.vizcolor.accent",
            Self::Gradient => "opt.vizcolor.gradient",
            Self::Rainbow => "opt.vizcolor.rainbow",
            Self::White => "opt.vizcolor.white",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CineShots {
    Mix,
    Orbit,
    Low,
    High,
    Dolly,
    Crane,
    Track,
}

impl CineShots {
    pub const ALL: [Self; 7] = [
        Self::Mix,
        Self::Orbit,
        Self::Low,
        Self::High,
        Self::Dolly,
        Self::Crane,
        Self::Track,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Mix => "opt.cineshot.mix",
            Self::Orbit => "opt.cineshot.orbit",
            Self::Low => "opt.cineshot.low",
            Self::High => "opt.cineshot.high",
            Self::Dolly => "opt.cineshot.dolly",
            Self::Crane => "opt.cineshot.crane",
            Self::Track => "opt.cineshot.track",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CineCuts {
    Smooth,
    Beat,
}

impl CineCuts {
    pub const ALL: [Self; 2] = [Self::Smooth, Self::Beat];
    pub fn key(self) -> &'static str {
        match self {
            Self::Smooth => "opt.cinecuts.smooth",
            Self::Beat => "opt.cinecuts.beat",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowAnim {
    Zoom,
    Slide,
    Pop,
    Unfold,
    Off,
}

impl WindowAnim {
    pub const ALL: [Self; 5] = [Self::Zoom, Self::Slide, Self::Pop, Self::Unfold, Self::Off];
    pub fn key(self) -> &'static str {
        match self {
            Self::Zoom => "opt.windowanim.zoom",
            Self::Slide => "opt.windowanim.slide",
            Self::Pop => "opt.windowanim.pop",
            Self::Unfold => "opt.windowanim.unfold",
            Self::Off => "opt.windowanim.off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsSourceMode {
    ExternalFirst,
    ExternalOnly,
    PlatformFirst,
    PlatformOnly,
}

impl LyricsSourceMode {
    pub const ALL: [Self; 4] = [
        Self::ExternalFirst,
        Self::ExternalOnly,
        Self::PlatformFirst,
        Self::PlatformOnly,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::ExternalFirst => "opt.lyricssourcemode.external_first",
            Self::ExternalOnly => "opt.lyricssourcemode.external_only",
            Self::PlatformFirst => "opt.lyricssourcemode.platform_first",
            Self::PlatformOnly => "opt.lyricssourcemode.platform_only",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::ExternalFirst => Self::ExternalOnly,
            Self::ExternalOnly => Self::PlatformFirst,
            Self::PlatformFirst => Self::PlatformOnly,
            Self::PlatformOnly => Self::ExternalFirst,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HudConfig {
    pub master: bool,
    pub hide_with_gui: bool,
    pub hide_on_tab: bool,

    pub enabled: bool,
    pub layout: Layout,
    pub show_mode: ShowMode,
    pub show_on_song_start: bool,
    pub show_on_song_end: bool,
    pub accent: Accent,
    pub custom_accent: i32,
    pub scale: f32,
    pub background_opacity: i32,
    pub show_cover: bool,
    pub show_timestamps: bool,
    pub show_waveform: bool,
    pub anchor: Anchor,
    pub offset_x: i32,
    pub offset_y: i32,

    pub lyrics_mode: LyricsMode,
    pub card_lyrics: bool,
    pub lyrics_word_karaoke: bool,
    pub lyrics_enter: LyricsEnter,
    pub lyrics_exit: WorldExit,
    pub lyrics_linger: i32,
    pub lyrics_letter_ms: i32,
    pub lyrics_shift_ms: i32,
    pub lyrics_scale: f32,
    pub lyrics_width: i32,
    pub lyrics_lines: i32,
    pub lyrics_background: i32,
    pub lyrics_accent: bool,
    pub lyrics_shadow: bool,
    pub lyrics_anchor: Anchor,
    pub lyrics_offset_x: i32,
    pub lyrics_offset_y: i32,

    pub world_lyrics: bool,
    pub lyrics_with_world: bool,
    pub world_mode: WorldMode,
    pub world_follow_style: FollowStyle,
    pub world_layout: WorldLayout,
    pub world_scale: f32,
    pub world_distance: f32,
    pub world_height: f32,
    pub world_spread: f32,
    pub world_see_through: bool,
    pub world_shadow: bool,
    pub world_anim: WorldAnim,
    pub world_unit: WorldUnit,
    pub world_exit: WorldExit,
    pub world_linger: i32,
    pub world_letter_ms: i32,
    pub world_anim_speed: i32,
    pub world_bounce: i32,
    pub world_color: WorldColor,
    pub world_accent: Accent,
    pub world_accent2: Accent,
    pub world_glow: WorldGlow,
    pub world_glow_strength: i32,
    pub world_glow_size: i32,
    pub world_enhanced_glow: bool,

    pub volume_enabled: bool,
    pub volume_style: VolumeStyle,
    pub volume_seconds: i32,
    pub volume_percent: bool,
    pub volume_scale: f32,
    pub volume_background: i32,
    pub volume_anchor: Anchor,
    pub volume_offset_x: i32,
    pub volume_offset_y: i32,

    pub viz_enabled: bool,
    pub viz_source: VizSource,
    pub viz_style: VizStyle,
    pub viz_stereo: bool,
    pub viz_direction: VizDirection,
    pub viz_color: VizColor,
    pub viz_peaks: bool,
    pub viz_hide_idle: bool,
    pub viz_auto_sens: bool,
    pub viz_sensitivity: i32,
    pub viz_smoothing: i32,
    pub viz_bars: i32,
    pub viz_bar_width: i32,
    pub viz_gap: i32,
    pub viz_height: i32,
    pub viz_scale: f32,
    pub viz_background: i32,
    pub viz_opacity: i32,
    pub viz_anchor: Anchor,
    pub viz_offset_x: i32,
    pub viz_offset_y: i32,

    pub cine_shots: CineShots,
    pub cine_speed: i32,
    pub cine_distance: f32,
    pub cine_height: f32,
    pub cine_shot_sec: i32,
    pub cine_fov: i32,
    pub cine_music: bool,
    pub cine_music_strength: i32,
    pub cine_cuts: CineCuts,
    pub cine_bars: bool,
    pub cine_keep_music_hud: bool,
    pub cine_exit_on_move: bool,
}

impl Default for HudConfig {
    fn default() -> Self {
        Self {
            master: true,
            hide_with_gui: false,
            hide_on_tab: true,

            enabled: true,
            layout: Layout::Card,
            show_mode: ShowMode::Always,
            show_on_song_start: true,
            show_on_song_end: false,
            accent: Accent::Raspberry,
            custom_accent: -1,
            scale: 1.0,
            background_opacity: 75,
            show_cover: true,
            show_timestamps: true,
            show_waveform: true,
            anchor: Anchor::TopLeft,
            offset_x: 10,
            offset_y: 10,

            lyrics_mode: LyricsMode::Subtitle,
            card_lyrics: false,
            lyrics_word_karaoke: true,
            lyrics_enter: LyricsEnter::Slide,
            lyrics_exit: WorldExit::Fade,
            lyrics_linger: 4,
            lyrics_letter_ms: 25,
            lyrics_shift_ms: 0,
            lyrics_scale: 1.0,
            lyrics_width: 340,
            lyrics_lines: 5,
            lyrics_background: 55,
            lyrics_accent: true,
            lyrics_shadow: true,
            lyrics_anchor: Anchor::BottomCenter,
            lyrics_offset_x: 0,
            lyrics_offset_y: 52,

            world_lyrics: false,
            lyrics_with_world: false,
            world_mode: WorldMode::Follow,
            world_follow_style: FollowStyle::Screen,
            world_layout: WorldLayout::Line,
            world_scale: 1.2,
            world_distance: 8.0,
            world_height: 0.4,
            world_spread: 45.0,
            world_see_through: false,
            world_shadow: true,
            world_anim: WorldAnim::Fly,
            world_unit: WorldUnit::Letters,
            world_exit: WorldExit::Fade,
            world_linger: 5,
            world_letter_ms: 35,
            world_anim_speed: 100,
            world_bounce: 100,
            world_color: WorldColor::Classic,
            world_accent: Accent::Raspberry,
            world_accent2: Accent::Purple,
            world_glow: WorldGlow::Sung,
            world_glow_strength: 65,
            world_glow_size: 130,
            world_enhanced_glow: true,

            volume_enabled: true,
            volume_style: VolumeStyle::Bar,
            volume_seconds: 3,
            volume_percent: true,
            volume_scale: 1.0,
            volume_background: 75,
            volume_anchor: Anchor::TopCenter,
            volume_offset_x: 0,
            volume_offset_y: 14,

            viz_enabled: false,
            viz_source: VizSource::Auto,
            viz_style: VizStyle::Bars,
            viz_stereo: false,
            viz_direction: VizDirection::Auto,
            viz_color: VizColor::Accent,
            viz_peaks: true,
            viz_hide_idle: true,
            viz_auto_sens: true,
            viz_sensitivity: 100,
            viz_smoothing: 65,
            viz_bars: 24,
            viz_bar_width: 4,
            viz_gap: 2,
            viz_height: 44,
            viz_scale: 1.0,
            viz_background: 0,
            viz_opacity: 85,
            viz_anchor: Anchor::BottomLeft,
            viz_offset_x: 12,
            viz_offset_y: 12,

            cine_shots: CineShots::Mix,
            cine_speed: 100,
            cine_distance: 5.5,
            cine_height: 1.4,
            cine_shot_sec: 7,
            cine_fov: 0,
            cine_music: true,
            cine_music_strength: 100,
            cine_cuts: CineCuts::Smooth,
            cine_bars: true,
            cine_keep_music_hud: true,
            cine_exit_on_move: true,
        }
    }
}

impl HudConfig {
    pub fn accent_rgb(&self) -> u32 {
        if self.custom_accent >= 0 {
            (self.custom_accent as u32) & 0xFF_FFFF
        } else {
            self.accent.rgb()
        }
    }

    pub fn sanitize(&mut self) {
        self.scale = self.scale.clamp(0.5, 2.0);
        self.background_opacity = self.background_opacity.clamp(0, 100);
        self.offset_x = self.offset_x.clamp(-100, 400);
        self.offset_y = self.offset_y.clamp(-100, 400);
        self.lyrics_linger = self.lyrics_linger.clamp(1, 15);
        self.lyrics_letter_ms = self.lyrics_letter_ms.clamp(0, 120);
        self.lyrics_shift_ms = self.lyrics_shift_ms.clamp(-3000, 3000);
        self.lyrics_scale = self.lyrics_scale.clamp(0.5, 2.0);
        self.lyrics_width = self.lyrics_width.clamp(200, 600);
        self.lyrics_lines = self.lyrics_lines.clamp(3, 9);
        self.lyrics_background = self.lyrics_background.clamp(0, 100);
        self.lyrics_offset_x = self.lyrics_offset_x.clamp(-100, 400);
        self.lyrics_offset_y = self.lyrics_offset_y.clamp(-100, 400);

        self.world_scale = self.world_scale.clamp(0.5, 2.5);
        self.world_distance = self.world_distance.clamp(3.0, 30.0);
        self.world_height = self.world_height.clamp(-1.5, 3.0);
        self.world_spread = self.world_spread.clamp(15.0, 90.0);
        self.world_linger = self.world_linger.clamp(1, 20);
        self.world_letter_ms = self.world_letter_ms.clamp(5, 150);
        self.world_anim_speed = self.world_anim_speed.clamp(25, 300);
        self.world_bounce = self.world_bounce.clamp(0, 300);
        self.world_glow_strength = self.world_glow_strength.clamp(0, 100);
        self.world_glow_size = self.world_glow_size.clamp(50, 250);

        self.volume_seconds = self.volume_seconds.clamp(1, 10);
        self.volume_scale = self.volume_scale.clamp(0.5, 2.0);
        self.volume_background = self.volume_background.clamp(0, 100);
        self.volume_offset_x = self.volume_offset_x.clamp(-100, 400);
        self.volume_offset_y = self.volume_offset_y.clamp(-100, 400);

        self.viz_sensitivity = self.viz_sensitivity.clamp(10, 400);
        self.viz_smoothing = self.viz_smoothing.clamp(0, 95);
        self.viz_bars = self.viz_bars.clamp(4, 96);
        self.viz_bar_width = self.viz_bar_width.clamp(1, 10);
        self.viz_gap = self.viz_gap.clamp(0, 6);
        self.viz_height = self.viz_height.clamp(8, 160);
        self.viz_scale = self.viz_scale.clamp(0.5, 2.0);
        self.viz_background = self.viz_background.clamp(0, 100);
        self.viz_opacity = self.viz_opacity.clamp(5, 100);
        self.viz_offset_x = self.viz_offset_x.clamp(-100, 400);
        self.viz_offset_y = self.viz_offset_y.clamp(-100, 400);

        self.cine_speed = self.cine_speed.clamp(25, 300);
        self.cine_distance = self.cine_distance.clamp(2.0, 14.0);
        self.cine_height = self.cine_height.clamp(-1.0, 5.0);
        self.cine_shot_sec = self.cine_shot_sec.clamp(2, 20);
        self.cine_fov = self.cine_fov.clamp(-30, 30);
        self.cine_music_strength = self.cine_music_strength.clamp(0, 200);
    }

    pub fn load() -> Self {
        let p = data_dir().join("hud.json");
        let mut cfg: Self = fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        cfg.sanitize();
        cfg
    }

    pub fn save(&self) {
        let mut copy = self.clone();
        copy.sanitize();
        if let Ok(json) = serde_json::to_string_pretty(&copy) {
            let _ = atomic_write_bytes(&data_dir().join("hud.json"), json.as_bytes());
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerConfig {
    pub sync_accent: bool,
    pub accent: Accent,
    pub custom_accent: i32,
    pub window_scale: i32,
    pub corner_radius: i32,
    pub window_anim: WindowAnim,
    pub tint: i32,
    pub window_opacity: i32,
    pub dim: i32,
    pub show_cover: bool,
    pub cover_px: i32,
    pub show_lyrics: bool,
    pub accent_text: bool,

    pub use_game_volume: bool,
    pub audio_device: String,
    pub duck_when_inactive: bool,
    pub prefetch: bool,
    pub prefetch_count: i32,
    pub disk_cache: bool,
    pub online_lyrics: bool,
    pub amll_lyrics: bool,
    pub kugou_lyrics: bool,
    pub discord_presence: bool,

    // PRO MAX Lyrics extensions
    pub lyrics_source_mode: LyricsSourceMode,
    pub netease_lyrics: bool,
    pub qq_lyrics: bool,
    pub dsp_auto_align: bool,
    pub better_lyrics: bool,
    pub musixmatch_token: String,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            sync_accent: true,
            accent: Accent::Raspberry,
            custom_accent: -1,
            window_scale: 100,
            corner_radius: 10,
            window_anim: WindowAnim::Zoom,
            tint: 1,
            window_opacity: 92,
            dim: 55,
            show_cover: true,
            cover_px: 88,
            show_lyrics: true,
            accent_text: false,

            use_game_volume: false,
            audio_device: String::new(),
            duck_when_inactive: false,
            prefetch: true,
            prefetch_count: 2,
            disk_cache: true,
            online_lyrics: true,
            amll_lyrics: true,
            kugou_lyrics: true,
            discord_presence: true,

            lyrics_source_mode: LyricsSourceMode::ExternalFirst,
            netease_lyrics: true,
            qq_lyrics: true,
            dsp_auto_align: true,
            better_lyrics: false,
            musixmatch_token: String::new(),
        }
    }
}

impl PlayerConfig {
    pub fn resolved_accent_rgb(&self, hud: &HudConfig) -> u32 {
        if self.sync_accent {
            hud.accent_rgb()
        } else if self.custom_accent >= 0 {
            (self.custom_accent as u32) & 0xFF_FFFF
        } else {
            self.accent.rgb()
        }
    }

    pub fn sanitize(&mut self) {
        self.window_scale = self.window_scale.clamp(70, 150);
        self.corner_radius = self.corner_radius.clamp(0, 16);
        self.tint = self.tint.clamp(0, 3);
        self.window_opacity = self.window_opacity.clamp(20, 100);
        self.dim = self.dim.clamp(0, 100);
        self.cover_px = self.cover_px.clamp(40, 140);
        self.prefetch_count = self.prefetch_count.clamp(1, 8);
    }

    pub fn load() -> Self {
        let p = data_dir().join("player.json");
        let mut cfg: Self = fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        cfg.sanitize();
        cfg
    }

    pub fn save(&self) {
        let mut copy = self.clone();
        copy.sanitize();
        if let Ok(json) = serde_json::to_string_pretty(&copy) {
            let _ = atomic_write_bytes(&data_dir().join("player.json"), json.as_bytes());
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    pub platform: Platform,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: u8,
    pub position_ms: u64,
    pub index: usize,
    pub queue: Vec<crate::model::Track>,
    pub track_shifts: std::collections::HashMap<String, i64>,
    pub query: String,
    pub help_seen: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            platform: Platform::Yandex,
            volume: 0.7,
            shuffle: false,
            repeat: 0,
            position_ms: 0,
            index: 0,
            queue: Vec::new(),
            track_shifts: std::collections::HashMap::new(),
            query: String::new(),
            help_seen: false,
        }
    }
}

impl SessionState {
    pub fn load() -> Self {
        let p = data_dir().join("session.json");
        let mut s: Self = fs::read_to_string(p)
            .ok()
            .and_then(|txt| serde_json::from_str(&txt).ok())
            .unwrap_or_default();
        s.volume = s.volume.clamp(0.0, 1.0);
        s.repeat = s.repeat.min(2);
        s
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = atomic_write_bytes(&data_dir().join("session.json"), json.as_bytes());
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenStore {
    pub yandex_token: String,
    pub yandex_accounts: Vec<Account>,
    pub soundcloud_client_id: String,
    pub soundcloud_oauth: String,
    pub soundcloud_user: String,
    pub spotify_client_id: String,
    pub spotify_access_token: String,
    pub spotify_refresh_token: String,
    pub spotify_expires_at_ms: i64,
    pub spotify_user: String,
    pub youtube_cookie: String,
    pub youtube_user: String,
}

impl TokenStore {
    pub fn load() -> Self {
        let p = data_dir().join("tokens.json");
        fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = atomic_write_bytes(&data_dir().join("tokens.json"), json.as_bytes());
        }
    }

    pub fn add_yandex_account(&mut self, label: String, token: String) {
        self.yandex_token = token.clone();
        self.yandex_accounts.retain(|a| a.token != token);
        self.yandex_accounts.insert(0, Account { label, token });
        if self.yandex_accounts.len() > 16 {
            self.yandex_accounts.truncate(16);
        }
        self.save();
    }

    pub fn remove_yandex_by_token(&mut self, token: &str) {
        self.yandex_accounts.retain(|a| a.token != token);
        if self.yandex_token == token {
            self.yandex_token = self
                .yandex_accounts
                .first()
                .map(|a| a.token.clone())
                .unwrap_or_default();
        }
        self.save();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    Open,
    Prev,
    Next,
    Pause,
    VolUp,
    VolDown,
    ToggleHud,
    HudSettings,
    WorldLyrics,
    Cinematic,
}

impl HotkeyAction {
    pub const ALL: [Self; 10] = [
        Self::Open,
        Self::Prev,
        Self::Next,
        Self::Pause,
        Self::VolUp,
        Self::VolDown,
        Self::ToggleHud,
        Self::HudSettings,
        Self::WorldLyrics,
        Self::Cinematic,
    ];

    pub fn default_key(self) -> &'static str {
        match self {
            Self::Open => "key.keyboard.m",
            Self::Prev => "key.keyboard.left.bracket",
            Self::Next => "key.keyboard.right.bracket",
            Self::Pause => "key.keyboard.semicolon",
            Self::VolUp => "key.keyboard.equal",
            Self::VolDown => "key.keyboard.minus",
            Self::ToggleHud => "key.keyboard.h",
            Self::HudSettings => "key.keyboard.j",
            Self::WorldLyrics => "key.keyboard.o",
            Self::Cinematic => "key.keyboard.k",
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Open => "key.musicplayer.open",
            Self::Prev => "key.musicplayer.prev",
            Self::Next => "key.musicplayer.next",
            Self::Pause => "key.musicplayer.pause",
            Self::VolUp => "key.musicplayer.vol_up",
            Self::VolDown => "key.musicplayer.vol_down",
            Self::ToggleHud => "key.musicplayer.toggle_hud",
            Self::HudSettings => "key.musicplayer.hud_settings",
            Self::WorldLyrics => "key.musicplayer.world_lyrics",
            Self::Cinematic => "key.musicplayer.cinematic",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub open: String,
    pub prev: String,
    pub next: String,
    pub pause: String,
    pub vol_up: String,
    pub vol_down: String,
    pub toggle_hud: String,
    pub hud_settings: String,
    pub world_lyrics: String,
    pub cinematic: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            open: HotkeyAction::Open.default_key().to_owned(),
            prev: HotkeyAction::Prev.default_key().to_owned(),
            next: HotkeyAction::Next.default_key().to_owned(),
            pause: HotkeyAction::Pause.default_key().to_owned(),
            vol_up: HotkeyAction::VolUp.default_key().to_owned(),
            vol_down: HotkeyAction::VolDown.default_key().to_owned(),
            toggle_hud: HotkeyAction::ToggleHud.default_key().to_owned(),
            hud_settings: HotkeyAction::HudSettings.default_key().to_owned(),
            world_lyrics: HotkeyAction::WorldLyrics.default_key().to_owned(),
            cinematic: HotkeyAction::Cinematic.default_key().to_owned(),
        }
    }
}

impl HotkeyConfig {
    pub fn key_of(&self, action: HotkeyAction) -> &str {
        match action {
            HotkeyAction::Open => &self.open,
            HotkeyAction::Prev => &self.prev,
            HotkeyAction::Next => &self.next,
            HotkeyAction::Pause => &self.pause,
            HotkeyAction::VolUp => &self.vol_up,
            HotkeyAction::VolDown => &self.vol_down,
            HotkeyAction::ToggleHud => &self.toggle_hud,
            HotkeyAction::HudSettings => &self.hud_settings,
            HotkeyAction::WorldLyrics => &self.world_lyrics,
            HotkeyAction::Cinematic => &self.cinematic,
        }
    }

    pub fn set(&mut self, action: HotkeyAction, key: String) {
        let slot = match action {
            HotkeyAction::Open => &mut self.open,
            HotkeyAction::Prev => &mut self.prev,
            HotkeyAction::Next => &mut self.next,
            HotkeyAction::Pause => &mut self.pause,
            HotkeyAction::VolUp => &mut self.vol_up,
            HotkeyAction::VolDown => &mut self.vol_down,
            HotkeyAction::ToggleHud => &mut self.toggle_hud,
            HotkeyAction::HudSettings => &mut self.hud_settings,
            HotkeyAction::WorldLyrics => &mut self.world_lyrics,
            HotkeyAction::Cinematic => &mut self.cinematic,
        };
        *slot = key;
        self.save();
    }

    pub fn display_name(key: &str) -> String {
        if key.is_empty() {
            return crate::i18n::get("keys.unbound");
        }
        let short = key.strip_prefix("key.keyboard.").unwrap_or(key);
        match short {
            "left.bracket" => "[".to_owned(),
            "right.bracket" => "]".to_owned(),
            "semicolon" => ";".to_owned(),
            "equal" => "=".to_owned(),
            "minus" => "-".to_owned(),
            other => other.to_ascii_uppercase(),
        }
    }

    pub fn load() -> Self {
        let p = data_dir().join("hotkeys.json");
        fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = atomic_write_bytes(&data_dir().join("hotkeys.json"), json.as_bytes());
        }
    }
}
