use crate::i18n;
use serde::{Deserialize, Serialize};

pub const MOD_NAME: &str = "Overtone: Lyrics PRO MAX SUPER ULTRA MEGA GIGA PLUS";
pub const MOD_SHORT_NAME: &str = "Overtone";
pub const MOD_ID: &str = "overtone_lyrics";
pub const MOD_AUTHORS: &str = "cosmo-jar, chifir4ik, PabJlo";
pub const MOD_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub cover_url: Option<String>,
    pub provider: String,
}

impl Track {
    pub fn platform(&self) -> Platform {
        Platform::by_id(&self.provider)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTiming {
    pub time_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
    pub words: Vec<WordTiming>,
}

impl LyricLine {
    pub const UNSYNCED: u64 = u64::MAX;

    pub fn new(time_ms: u64, text: impl Into<String>) -> Self {
        Self {
            time_ms,
            text: text.into(),
            words: Vec::new(),
        }
    }

    pub fn with_words(time_ms: u64, text: impl Into<String>, words: Vec<WordTiming>) -> Self {
        Self {
            time_ms,
            text: text.into(),
            words,
        }
    }

    pub fn is_unsynced(&self) -> bool {
        self.time_ms == Self::UNSYNCED
    }

    pub fn is_synced(&self) -> bool {
        !self.is_unsynced()
    }

    pub fn has_word_timings(&self) -> bool {
        !self.words.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricSourceKind {
    Custom,
    Platform,
    Amll,
    LrcLib,
    NetEase,
    QqMusic,
    KuGou,
    Musixmatch,
    BetterLyrics,
    AutoDsp,
}

impl LyricSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Platform => "Platform",
            Self::Amll => "AMLL",
            Self::LrcLib => "LRCLIB",
            Self::NetEase => "NetEase",
            Self::QqMusic => "QQ Music",
            Self::KuGou => "KuGou",
            Self::Musixmatch => "Musixmatch",
            Self::BetterLyrics => "BetterLyrics",
            Self::AutoDsp => "Auto-DSP",
        }
    }

    pub fn is_external(self) -> bool {
        !matches!(self, Self::Platform | Self::Custom)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LyricCandidate {
    pub source: LyricSourceKind,
    pub lines: Vec<LyricLine>,
    pub score: f32,
    pub has_word_timings: bool,
    pub is_auto_dsp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Artist,
    Album,
    Playlist,
    Station,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicEntity {
    pub kind: EntityKind,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
}

impl MusicEntity {
    pub fn mine() -> Self {
        Self {
            kind: EntityKind::Playlist,
            id: "mine".to_owned(),
            title: String::new(),
            subtitle: String::new(),
            cover_url: None,
        }
    }

    pub fn is_mine(&self) -> bool {
        self.kind == EntityKind::Playlist && self.id == "mine"
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub source: MusicEntity,
    pub tracks: Vec<Track>,
    pub entities: Vec<MusicEntity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Yandex,
    SoundCloud,
    Local,
    Spotify,
    YouTube,
}

impl Platform {
    pub const ALL: [Platform; 5] = [
        Platform::Yandex,
        Platform::SoundCloud,
        Platform::YouTube,
        Platform::Local,
        Platform::Spotify,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Yandex => "yandex",
            Self::SoundCloud => "soundcloud",
            Self::Local => "local",
            Self::Spotify => "spotify",
            Self::YouTube => "youtube",
        }
    }

    pub fn rgb(self) -> u32 {
        match self {
            Self::Yandex => 0xFFCC00,
            Self::SoundCloud => 0xFF5500,
            Self::Local => 0x34D399,
            Self::Spotify => 0x1DB954,
            Self::YouTube => 0xFF0000,
        }
    }

    pub fn needs_login(self) -> bool {
        matches!(self, Self::Yandex | Self::Spotify)
    }

    pub fn title(self) -> String {
        i18n::get(&format!("platform.{}", self.id()))
    }

    pub fn by_id(id: &str) -> Self {
        Self::by_id_or_none(id).unwrap_or(Self::Yandex)
    }

    pub fn by_id_or_none(id: &str) -> Option<Self> {
        match id {
            "yandex" => Some(Self::Yandex),
            "soundcloud" => Some(Self::SoundCloud),
            "local" => Some(Self::Local),
            "spotify" => Some(Self::Spotify),
            "youtube" => Some(Self::YouTube),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub label: String,
    pub token: String,
}

/// Normalizes YouTube/SoundCloud uploader titles such as "Artist - Track [Official Video]".
pub fn split_track_name(raw_title: &str, uploader: &str) -> (String, String) {
    let title = strip_video_noise(raw_title);
    let artist = clean_uploader(uploader);
    for sep in [" - ", " – ", " — "] {
        if let Some(i) = title.find(sep) {
            let left = &title[..i];
            let left_chars = left.chars().count();
            if left_chars > 0 && left_chars <= 60 {
                let right = title[i + sep.len()..].trim();
                if !right.is_empty() {
                    return (left.trim().to_owned(), right.to_owned());
                }
            }
        }
    }
    (artist, title)
}

pub fn clean_uploader(name: &str) -> String {
    let mut s = name.trim();
    if let Some(stripped) = s.strip_suffix(" - Topic") {
        s = stripped;
    }
    if s.len() > 4 && s.to_ascii_uppercase().ends_with("VEVO") {
        s = &s[..s.len() - 4];
    }
    s.trim().to_owned()
}

fn strip_video_noise(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '(' || c == '[' {
            let close = if c == '(' { ')' } else { ']' };
            if let Some(rel) = chars[i + 1..].iter().position(|&ch| ch == close) {
                let inner: String = chars[i + 1..i + 1 + rel].iter().collect();
                let low = inner.trim().to_ascii_lowercase();
                let is_noise = [
                    "official",
                    "lyric",
                    "lyrics",
                    "audio",
                    "video",
                    "music video",
                    "visualizer",
                    "visualiser",
                    "hd",
                    "hq",
                    "4k",
                    "clip",
                    "клип",
                    "премьера",
                    "текст",
                ]
                .iter()
                .any(|kw| low.starts_with(kw));
                if is_noise {
                    i += rel + 2;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_cleans_uploader_titles() {
        let (a, t) = split_track_name(
            "алёна швец - вино и сигареты (Official Video)",
            "алёна швецVEVO",
        );
        assert_eq!(a, "алёна швец");
        assert_eq!(t, "вино и сигареты");

        let (a2, t2) = split_track_name("Night Sky [4K]", "Artist - Topic");
        assert_eq!(a2, "Artist");
        assert_eq!(t2, "Night Sky");
    }
}
