use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{atomic_write_bytes, data_dir};
use crate::lyrics::parsers::{parse_lrc, parse_ttml};
use crate::model::{LyricLine, LyricSourceKind, Track};
use crate::net::md5_hex;

const SYNCED_TTL_MS: u64 = 14 * 24 * 3600 * 1000;
const PLAIN_TTL_MS: u64 = 3 * 24 * 3600 * 1000;
const DSP_CACHE_VERSION: u32 = 4;

#[derive(Serialize, Deserialize)]
struct CachedEntry {
    updated_ms: u64,
    source: LyricSourceKind,
    lines: Vec<LyricLine>,
    #[serde(default)]
    dsp_version: u32,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn cache_dir() -> PathBuf {
    data_dir().join("lyrics_cache")
}

fn custom_dir() -> PathBuf {
    data_dir().join("custom_lyrics")
}

pub fn track_key(t: &Track) -> String {
    let raw = format!(
        "{}|{}|{}|{}",
        t.provider.trim().to_ascii_lowercase(),
        t.id.trim(),
        t.artist.trim().to_lowercase(),
        t.title.trim().to_lowercase()
    );
    md5_hex(raw.as_bytes())
}

pub fn get_cached(t: &Track) -> Option<(LyricSourceKind, Vec<LyricLine>)> {
    let path = cache_dir().join(format!("{}.json", track_key(t)));
    let text = fs::read_to_string(path).ok()?;
    let entry: CachedEntry = serde_json::from_str(&text).ok()?;
    if entry.source == LyricSourceKind::AutoDsp && entry.dsp_version < DSP_CACHE_VERSION {
        return None;
    }
    let ttl = if entry.lines.first().is_some_and(|l| !l.is_unsynced()) {
        SYNCED_TTL_MS
    } else {
        PLAIN_TTL_MS
    };
    if now_ms().saturating_sub(entry.updated_ms) > ttl {
        return None;
    }
    Some((entry.source, entry.lines))
}

pub fn put_cached(t: &Track, source: LyricSourceKind, lines: &[LyricLine]) {
    if lines.is_empty() || lines.first().is_some_and(LyricLine::is_unsynced) {
        return;
    }
    let entry = CachedEntry {
        updated_ms: now_ms(),
        source,
        lines: lines.to_vec(),
        dsp_version: if source == LyricSourceKind::AutoDsp {
            DSP_CACHE_VERSION
        } else {
            0
        },
    };
    if let Ok(json) = serde_json::to_vec(&entry) {
        let _ = atomic_write_bytes(&cache_dir().join(format!("{}.json", track_key(t))), &json);
    }
}

pub fn has_custom(t: &Track) -> bool {
    custom_dir().join(format!("{}.json", track_key(t))).exists()
}

pub fn load_custom(t: &Track) -> Option<Vec<LyricLine>> {
    let path = custom_dir().join(format!("{}.json", track_key(t)));
    let text = fs::read_to_string(path).ok()?;
    let lines: Vec<LyricLine> = serde_json::from_str(&text).ok()?;
    if lines.is_empty() { None } else { Some(lines) }
}

pub fn save_custom(t: &Track, lines: &[LyricLine]) -> Result<(), String> {
    if lines.is_empty() {
        return Err("Empty lyrics".to_owned());
    }
    let json = serde_json::to_vec_pretty(lines).map_err(|e| e.to_string())?;
    atomic_write_bytes(&custom_dir().join(format!("{}.json", track_key(t))), &json)
        .map_err(|e| e.to_string())
}

pub fn remove_custom(t: &Track) {
    let _ = fs::remove_file(custom_dir().join(format!("{}.json", track_key(t))));
}

/// Imports dropped `.lrc` / `.ttml` / `.txt` file into custom lyrics store.
/// Plain `.txt` without timestamps will be aligned via DSP if PCM is supplied, or rejected with error.
pub fn parse_dropped_lyrics_file(path: &Path) -> Result<(Vec<LyricLine>, Vec<String>), String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    match ext.as_str() {
        "lrc" => {
            let lines = parse_lrc(&content);
            if lines.is_empty() {
                Err("No timed lines in .lrc".to_owned())
            } else {
                Ok((lines, Vec::new()))
            }
        }
        "ttml" | "xml" => {
            let lines = parse_ttml(&content);
            if lines.is_empty() {
                Err("No timed lines in .ttml".to_owned())
            } else {
                Ok((lines, Vec::new()))
            }
        }
        "txt" => {
            let timed = parse_lrc(&content);
            if !timed.is_empty() {
                Ok((timed, Vec::new()))
            } else {
                let plain = crate::lyrics::parsers::parse_plain_lines(&content);
                if plain.is_empty() {
                    Err("Empty .txt file".to_owned())
                } else {
                    Ok((Vec::new(), plain))
                }
            }
        }
        _ => Err(format!("Unsupported lyrics extension: .{ext}")),
    }
}
