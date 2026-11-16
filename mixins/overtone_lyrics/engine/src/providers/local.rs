//! Local folder music provider (`LocalLibrary` wrapper), ported 1:1 from `LocalMusicProvider.java`.

use std::fs;
use std::path::PathBuf;

use crate::audio::AudioFormat;
use crate::local::{AudioCache, AudioKind, LocalLibrary, sidecar_lyrics_text};
use crate::lyrics::parsers::{parse_lrc, parse_plain_lines};
use crate::model::{EntityKind, MusicEntity, Page, Platform, Track};
use crate::providers::{MusicProvider, PlatformLyrics, PlaybackSource, SearchResult};

const MAX_LOCAL_FILE_BYTES: u64 = 400 << 20;

#[derive(Clone)]
pub struct LocalMusicProvider {
    pub library: LocalLibrary,
}

impl Default for LocalMusicProvider {
    fn default() -> Self {
        Self::new(LocalLibrary::new())
    }
}

impl LocalMusicProvider {
    pub fn new(library: LocalLibrary) -> Self {
        Self { library }
    }

    pub fn add_paths(&self, paths: &[PathBuf]) -> usize {
        self.library.add_paths(paths)
    }

    pub fn open_folder(&self) {
        let dir = LocalLibrary::default_folder();
        let _ = fs::create_dir_all(&dir);
        let _ = open::that(dir);
    }

    pub fn rescan(&self) {
        self.library.rescan();
    }
}

impl MusicProvider for LocalMusicProvider {
    fn platform(&self) -> Platform {
        Platform::Local
    }

    fn has_auth(&self) -> bool {
        true
    }

    fn search(&self, query: &str) -> Result<SearchResult, String> {
        Ok(SearchResult {
            tracks: self.library.search(query),
            entities: Vec::new(),
        })
    }

    fn liked_tracks(&self) -> Result<Vec<Track>, String> {
        Ok(self.library.search(""))
    }

    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String> {
        Ok(Vec::new())
    }

    fn open(&self, entity: &MusicEntity) -> Result<Page, String> {
        if entity.is_mine() && entity.kind == EntityKind::Playlist {
            return Ok(Page {
                source: entity.clone(),
                tracks: self.liked_tracks()?,
                entities: Vec::new(),
            });
        }
        Err("Not supported for local tracks".to_owned())
    }

    fn fetch_lyrics(&self, track: &Track) -> PlatformLyrics {
        let Some(path) = self.library.path_of(&track.id) else {
            return PlatformLyrics::default();
        };
        let raw = sidecar_lyrics_text(&path);
        if raw.trim().is_empty() {
            return PlatformLyrics::default();
        }
        let synced = parse_lrc(&raw);
        if !synced.is_empty() {
            return PlatformLyrics {
                synced,
                plain: Vec::new(),
            };
        }
        PlatformLyrics {
            synced: Vec::new(),
            plain: parse_plain_lines(&raw),
        }
    }

    fn set_liked(&self, _track: &Track, _liked: bool) -> Result<(), String> {
        Ok(())
    }

    fn load_audio(&self, track: &Track) -> Result<PlaybackSource, String> {
        let path = self
            .library
            .path_of(&track.id)
            .ok_or_else(|| "File not found in local index".to_owned())?;
        let meta = fs::metadata(&path).map_err(|_| "Local audio file missing".to_owned())?;
        if !meta.is_file() {
            return Err("Local audio path is not a file".to_owned());
        }
        if meta.len() > MAX_LOCAL_FILE_BYTES {
            return Err("File too large (400 MB limit)".to_owned());
        }
        let bytes = fs::read(&path).map_err(|e| format!("Failed to read local file: {e}"))?;
        let format = AudioFormat::detect(&bytes)
            .or_else(|| match AudioKind::from_path(&path)? {
                AudioKind::Mp3 => Some(AudioFormat::Mp3),
                AudioKind::Flac => Some(AudioFormat::Flac),
                AudioKind::Ogg => Some(AudioFormat::Ogg),
                AudioKind::Mp4 => Some(AudioFormat::Aac),
                AudioKind::Pcm => Some(AudioFormat::Pcm),
            })
            .ok_or_else(|| "Unsupported audio container".to_owned())?;
        let _ = AudioCache::shared();
        Ok(PlaybackSource::Bytes {
            format,
            data: bytes,
        })
    }
}
