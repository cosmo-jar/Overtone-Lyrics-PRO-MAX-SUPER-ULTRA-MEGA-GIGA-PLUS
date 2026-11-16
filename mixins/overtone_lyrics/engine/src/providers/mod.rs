//! Music provider abstraction and shared helpers, ported 1:1 from `Overtone-main`.

pub mod local;
pub mod soundcloud;
pub mod spotify;
pub mod yandex;
pub mod youtube;

use std::sync::Arc;

use serde_json::Value;

use crate::audio::{AudioFormat, decode_full_pcm_for_dsp};
use crate::lyrics::AudioDspInput;
use crate::model::{LyricLine, MusicEntity, Page, Platform, Track};

#[derive(Clone, Debug, Default)]
pub struct SearchResult {
    pub tracks: Vec<Track>,
    pub entities: Vec<MusicEntity>,
}

#[derive(Clone, Debug)]
pub struct WaveStart {
    pub seed: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default)]
pub struct PlatformLyrics {
    pub synced: Vec<LyricLine>,
    pub plain: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum PlaybackSource {
    Bytes { format: AudioFormat, data: Vec<u8> },
    External,
}

impl PlaybackSource {
    pub fn decode_dsp_input(format: AudioFormat, data: &[u8]) -> Option<Arc<AudioDspInput>> {
        let (sample_rate, channels, pcm) = decode_full_pcm_for_dsp(format, data).ok()?;
        if pcm.is_empty() || channels == 0 || sample_rate == 0 {
            return None;
        }
        Some(Arc::new(AudioDspInput {
            pcm,
            channels,
            sample_rate,
        }))
    }
}

pub trait MusicProvider: Send + Sync {
    fn platform(&self) -> Platform;
    fn has_auth(&self) -> bool;
    fn search(&self, query: &str) -> Result<SearchResult, String>;
    fn liked_tracks(&self) -> Result<Vec<Track>, String>;
    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String>;
    fn open(&self, entity: &MusicEntity) -> Result<Page, String>;
    fn fetch_lyrics(&self, track: &Track) -> PlatformLyrics;
    fn set_liked(&self, track: &Track, liked: bool) -> Result<(), String>;
    fn load_audio(&self, track: &Track) -> Result<PlaybackSource, String>;
    fn on_queue_change(&self, upcoming_ids: &[String], current_id: Option<&str>) {
        let _ = (upcoming_ids, current_id);
    }
}

pub(crate) fn json_str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn json_id(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn json_u64(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::Number(n)) => n
            .as_u64()
            .unwrap_or_else(|| n.as_f64().map_or(0, |f| f.max(0.0) as u64)),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

pub(crate) fn yandex_cover_url(raw: &str, size: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let url = if let Some(rest) = s.strip_prefix("http://") {
        format!("https://{rest}")
    } else if s.starts_with("https://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    Some(url.replace("%%", size))
}
