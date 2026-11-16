//! Central playback, queue, wave, lyrics, and provider controller,
//! ported 1:1 from `PlayerController.java` with PRO MAX Lyrics + Level B DSP support.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::AudioEngine;
use crate::config::{HotkeyConfig, HudConfig, PlayerConfig, SessionState, TokenStore};
use crate::discord::{DiscordPresence, PresenceSnapshot};
use crate::i18n;
use crate::local::{AudioCache, LocalLibrary};
use crate::lyrics::{self, AudioDspInput, LyricsLookupResult, cache as lyrics_cache, dsp_aligner};
use crate::model::{LyricCandidate, MusicEntity, Page, Platform, Track};
use crate::providers::local::LocalMusicProvider;
use crate::providers::soundcloud::SoundCloudProvider;
use crate::providers::spotify::{END_WINDOW_MS, SpotifyProvider};
use crate::providers::yandex::{DeviceAuthSession, DevicePollResult, YandexMusicProvider};
use crate::providers::youtube::YouTubeProvider;
use crate::providers::{MusicProvider, PlaybackSource};
use crate::viz::{SystemCapture, VisualizerState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Search,
    Liked,
    Wave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off = 0,
    All = 1,
    One = 2,
}

impl RepeatMode {
    pub fn from_u8(v: u8) -> Self {
        match v % 3 {
            1 => Self::All,
            2 => Self::One,
            _ => Self::Off,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

pub struct ControllerState {
    pub platform: Platform,
    pub tab: Tab,
    pub search_query: String,
    pub search_tracks: Vec<Track>,
    pub search_entities: Vec<MusicEntity>,
    pub liked: Vec<Track>,
    pub playlists: Vec<MusicEntity>,
    pub stations: Vec<MusicEntity>,
    pub page_trail: Vec<Page>,

    pub wave_token: String,
    pub wave_title: String,
    pub wave_tracks: Vec<Track>,
    pub wave_fetching: bool,

    pub queue: Vec<Track>,
    pub user_queue: Vec<Track>,
    pub index: Option<usize>,
    pub history: Vec<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
    pub saved_unmuted_volume: f32,
    pub wave_view: bool,
    pub track_shifts: HashMap<String, i64>,
    pub help_seen: bool,
    pub list_page: usize,
    pub queue_mode: bool,

    pub liked_ids: HashSet<String>,
    pub busy: bool,
    pub loading_track: bool,
    pub status_msg: String,
    pub error_msg: String,

    pub volume_popup_timer: f32,
    pub game_music_volume: f32,
    pub game_active: bool,

    pub lyrics: Option<LyricsLookupResult>,
    pub lyrics_track_id: String,
    pub lyrics_loading: bool,
    pub active_audio_pcm: Option<(String, Arc<AudioDspInput>)>,

    pub browse_gen: u64,
    pub play_gen: u64,
    pub lyrics_gen: u64,
    pub rng_seed: u64,

    pub last_spotify_poll: Instant,
    pub last_session_save: Instant,
    pub session_dirty: bool,
    pub local_end_reported_id: String,
}

#[derive(Clone)]
pub struct PlayerController {
    pub player_cfg: Arc<Mutex<PlayerConfig>>,
    pub hud_cfg: Arc<Mutex<HudConfig>>,
    pub hotkey_cfg: Arc<Mutex<HotkeyConfig>>,
    pub tokens: Arc<Mutex<TokenStore>>,
    pub cache: AudioCache,
    pub local: LocalMusicProvider,
    pub yandex: YandexMusicProvider,
    pub soundcloud: SoundCloudProvider,
    pub spotify: SpotifyProvider,
    pub youtube: YouTubeProvider,
    pub audio: Arc<AudioEngine>,
    pub discord: DiscordPresence,
    pub viz: Arc<Mutex<VisualizerState>>,
    pub state: Arc<Mutex<ControllerState>>,
    pending_end: Arc<AtomicBool>,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self::new(false)
    }
}

impl PlayerController {
    pub fn new(start_background_threads: bool) -> Self {
        let player_cfg = Arc::new(Mutex::new(PlayerConfig::load()));
        let hud_cfg = Arc::new(Mutex::new(HudConfig::load()));
        let hotkey_cfg = Arc::new(Mutex::new(HotkeyConfig::load()));
        let tokens = Arc::new(Mutex::new(TokenStore::load()));
        let session = SessionState::load();

        let cache = AudioCache::default();
        let disk_on = player_cfg.lock().unwrap().disk_cache;
        let prefetch_on = player_cfg.lock().unwrap().prefetch;
        cache.set_disk_enabled(disk_on);

        let local = LocalMusicProvider::new(LocalLibrary::new());
        let yandex = YandexMusicProvider::new(Arc::clone(&tokens), cache.clone());
        yandex.set_prefetch_enabled(prefetch_on);
        let soundcloud = SoundCloudProvider::new(Arc::clone(&tokens), cache.clone());
        soundcloud.set_prefetch_enabled(prefetch_on);
        let spotify = SpotifyProvider::new(Arc::clone(&tokens));
        let youtube = YouTubeProvider::new(Arc::clone(&tokens));

        let audio = Arc::new(AudioEngine::new());
        audio.set_volume(session.volume);

        let discord = DiscordPresence::new();
        if start_background_threads {
            discord.start_thread();
        }

        let init_idx = if session.queue.is_empty() || session.index >= session.queue.len() {
            None
        } else {
            Some(session.index)
        };

        let state = Arc::new(Mutex::new(ControllerState {
            platform: session.platform,
            tab: Tab::Search,
            search_query: String::new(),
            search_tracks: Vec::new(),
            search_entities: Vec::new(),
            liked: Vec::new(),
            playlists: Vec::new(),
            stations: Vec::new(),
            page_trail: Vec::new(),

            wave_token: String::new(),
            wave_title: String::new(),
            wave_tracks: Vec::new(),
            wave_fetching: false,

            queue: session.queue,
            user_queue: Vec::new(),
            index: init_idx,
            history: Vec::new(),
            shuffle: session.shuffle,
            repeat: RepeatMode::from_u8(session.repeat),
            volume: session.volume,
            saved_unmuted_volume: if session.volume > 0.02 {
                session.volume
            } else {
                0.7
            },
            wave_view: false,
            track_shifts: session.track_shifts,
            help_seen: session.help_seen,
            list_page: 0,
            queue_mode: false,

            liked_ids: HashSet::new(),
            busy: false,
            loading_track: false,
            status_msg: String::new(),
            error_msg: String::new(),

            volume_popup_timer: 0.0,
            game_music_volume: 1.0,
            game_active: true,

            lyrics: None,
            lyrics_track_id: String::new(),
            lyrics_loading: false,
            active_audio_pcm: None,

            browse_gen: 1,
            play_gen: 1,
            lyrics_gen: 1,
            rng_seed: 0x517cc1b727220a95,

            last_spotify_poll: Instant::now() - Duration::from_secs(10),
            last_session_save: Instant::now(),
            session_dirty: false,
            local_end_reported_id: String::new(),
        }));

        Self {
            player_cfg,
            hud_cfg,
            hotkey_cfg,
            tokens,
            cache,
            local,
            yandex,
            soundcloud,
            spotify,
            youtube,
            audio,
            discord,
            viz: Arc::new(Mutex::new(VisualizerState::new())),
            state,
            pending_end: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn provider_of(&self, platform: Platform) -> &dyn MusicProvider {
        match platform {
            Platform::Yandex => &self.yandex,
            Platform::SoundCloud => &self.soundcloud,
            Platform::Local => &self.local,
            Platform::Spotify => &self.spotify,
            Platform::YouTube => &self.youtube,
        }
    }

    pub fn active_platform(&self) -> Platform {
        self.state.lock().unwrap().platform
    }

    pub fn has_auth(&self, platform: Platform) -> bool {
        self.provider_of(platform).has_auth()
    }

    pub fn current_track(&self) -> Option<Track> {
        let st = self.state.lock().unwrap();
        st.index.and_then(|i| st.queue.get(i).cloned())
    }

    pub fn is_playing(&self) -> bool {
        let cur = self.current_track();
        match cur.as_ref().map(Track::platform) {
            Some(Platform::Spotify) => self.spotify.remote_state().playing,
            Some(_) => self.audio.is_playing(),
            None => false,
        }
    }

    pub fn position_ms(&self) -> u64 {
        let Some(cur) = self.current_track() else {
            return 0;
        };
        if cur.platform() == Platform::Spotify {
            let r = self.spotify.remote_state();
            if r.playing {
                let elapsed = now_unix_ms().saturating_sub(r.updated_at_ms);
                (r.position_ms + elapsed).min(cur.duration_ms.max(r.duration_ms))
            } else {
                r.position_ms
            }
        } else {
            self.audio.position_ms()
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.current_track().map_or(0, |t| t.duration_ms)
    }

    pub fn save_player_config(&self) {
        let cfg = self.player_cfg.lock().unwrap().clone();
        cfg.save();
        self.cache.set_disk_enabled(cfg.disk_cache);
        self.yandex.set_prefetch_enabled(cfg.prefetch);
        self.soundcloud.set_prefetch_enabled(cfg.prefetch);
        let loop_needed = self.should_capture_system_audio(&cfg);
        SystemCapture::set_running(loop_needed);
    }

    pub fn save_hud_config(&self) {
        self.hud_cfg.lock().unwrap().save();
        let cfg = self.player_cfg.lock().unwrap().clone();
        SystemCapture::set_running(self.should_capture_system_audio(&cfg));
    }

    pub fn save_hotkey_config(&self) {
        self.hotkey_cfg.lock().unwrap().save();
    }

    pub fn save_session(&self) {
        let pos = self.position_ms();
        let st = self.state.lock().unwrap();
        let s = SessionState {
            platform: st.platform,
            volume: st.volume,
            shuffle: st.shuffle,
            repeat: st.repeat as u8,
            position_ms: pos,
            index: st.index.unwrap_or(0),
            queue: st.queue.iter().take(120).cloned().collect(),
            track_shifts: st.track_shifts.clone(),
            query: st.search_query.clone(),
            help_seen: st.help_seen,
        };
        drop(st);
        s.save();
    }

    pub fn select_platform(&self, platform: Platform) {
        if platform != Platform::Spotify {
            self.spotify.cancel_login();
        }
        {
            let mut st = self.state.lock().unwrap();
            if st.platform == platform {
                return;
            }
            st.platform = platform;
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.wave_view = false;
            st.search_tracks.clear();
            st.search_entities.clear();
            st.liked.clear();
            st.playlists.clear();
            st.stations.clear();
            st.status_msg.clear();
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            st.session_dirty = true;
            if platform != Platform::Yandex && st.tab == Tab::Wave {
                st.tab = Tab::Search;
            }
        }
        let cur_tab = self.state.lock().unwrap().tab;
        if cur_tab == Tab::Liked {
            self.load_liked();
        } else if cur_tab == Tab::Wave && platform == Platform::Yandex {
            self.load_wave_stations();
        }
    }

    pub fn select_tab(&self, tab: Tab) {
        let platform = {
            let mut st = self.state.lock().unwrap();
            st.tab = tab;
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.wave_view = false;
            st.error_msg.clear();
            st.platform
        };
        match tab {
            Tab::Liked => self.load_liked(),
            Tab::Wave if platform == Platform::Yandex => self.load_wave_stations(),
            _ => {}
        }
    }

    pub fn pop_page(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        let popped = st.page_trail.pop().is_some();
        if popped {
            st.list_page = 0;
        }
        popped
    }

    pub fn current_page(&self) -> Option<Page> {
        self.state.lock().unwrap().page_trail.last().cloned()
    }

    pub fn list_page(&self) -> usize {
        self.state.lock().unwrap().list_page
    }

    pub fn set_list_page(&self, page: usize) {
        self.state.lock().unwrap().list_page = page;
    }

    pub fn queue_mode(&self) -> bool {
        self.state.lock().unwrap().queue_mode
    }

    pub fn set_queue_mode(&self, queue_mode: bool) {
        self.state.lock().unwrap().queue_mode = queue_mode;
    }

    pub fn search(&self, query: &str) {
        let q = query.trim().to_owned();
        let (platform, task_gen) = {
            let mut st = self.state.lock().unwrap();
            st.search_query = q.clone();
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.busy = true;
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            (st.platform, st.browse_gen)
        };
        if q.is_empty() && platform != Platform::Local {
            let mut st = self.state.lock().unwrap();
            st.search_tracks.clear();
            st.search_entities.clear();
            st.busy = false;
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-search".to_string())
            .spawn(move || {
                let res = me.provider_of(platform).search(&q);
                let mut st = me.state.lock().unwrap();
                if st.browse_gen != task_gen {
                    return;
                }
                st.busy = false;
                match res {
                    Ok(sr) => {
                        st.search_tracks = sr.tracks;
                        st.search_entities = sr.entities;
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn load_liked(&self) {
        let (platform, task_gen) = {
            let mut st = self.state.lock().unwrap();
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.wave_view = false;
            st.search_tracks.clear();
            st.search_entities.clear();
            st.busy = true;
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            (st.platform, st.browse_gen)
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-liked".to_string())
            .spawn(move || {
                let prov = me.provider_of(platform);
                let tracks_res = prov.liked_tracks();
                let playlists_res = prov.my_playlists();
                let mut st = me.state.lock().unwrap();
                if st.browse_gen != task_gen {
                    return;
                }
                st.busy = false;
                match tracks_res {
                    Ok(list) => {
                        for tr in &list {
                            st.liked_ids.insert(tr.id.clone());
                        }
                        st.liked = list;
                    }
                    Err(e) => st.error_msg = e,
                }
                if let Ok(pls) = playlists_res {
                    st.playlists = pls;
                }
            })
            .ok();
    }

    pub fn open_entity(&self, entity: MusicEntity) {
        let (platform, task_gen) = {
            let mut st = self.state.lock().unwrap();
            st.busy = true;
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            (st.platform, st.browse_gen)
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-open".to_string())
            .spawn(move || {
                let res = me.provider_of(platform).open(&entity);
                let mut st = me.state.lock().unwrap();
                if st.browse_gen != task_gen {
                    return;
                }
                st.busy = false;
                match res {
                    Ok(page) => {
                        st.page_trail.push(page);
                        st.list_page = 0;
                        st.queue_mode = false;
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn load_youtube_charts(&self) {
        let task_gen = {
            let mut st = self.state.lock().unwrap();
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.busy = true;
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            st.browse_gen
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-yt-charts".to_string())
            .spawn(move || {
                let res = me.youtube.charts_page();
                let mut st = me.state.lock().unwrap();
                if st.browse_gen != task_gen {
                    return;
                }
                st.busy = false;
                match res {
                    Ok(page) => {
                        st.page_trail.push(page);
                        st.list_page = 0;
                        st.queue_mode = false;
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn load_wave_stations(&self) {
        let task_gen = {
            let mut st = self.state.lock().unwrap();
            st.busy = true;
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            st.browse_gen
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-stations".to_string())
            .spawn(move || {
                let res = me.yandex.wave_stations();
                let mut st = me.state.lock().unwrap();
                if st.browse_gen != task_gen {
                    return;
                }
                st.busy = false;
                match res {
                    Ok(sts) => st.stations = sts,
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn start_wave(&self, seed: &str, title: &str) {
        let seed = seed.to_owned();
        let title = title.to_owned();
        {
            let mut st = self.state.lock().unwrap();
            st.wave_fetching = true;
            st.wave_title = title;
            st.error_msg.clear();
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-wave-start".to_string())
            .spawn(move || match me.yandex.wave_start(&seed) {
                Ok(ws) => {
                    let tracks = ws.tracks;
                    {
                        let mut st = me.state.lock().unwrap();
                        st.wave_token = ws.seed;
                        st.wave_tracks = tracks.clone();
                        st.wave_view = true;
                        st.wave_fetching = false;
                    }
                    if !tracks.is_empty() {
                        me.yandex.wave_feedback(
                            &me.state.lock().unwrap().wave_token,
                            "radioStarted",
                            None,
                            0.0,
                        );
                        me.play_from(tracks, 0);
                    }
                }
                Err(e) => {
                    let mut st = me.state.lock().unwrap();
                    st.wave_fetching = false;
                    st.error_msg = e;
                }
            })
            .ok();
    }

    pub fn load_more_wave(&self) {
        let (tok, qids) = {
            let mut st = self.state.lock().unwrap();
            if st.wave_fetching || st.wave_token.is_empty() {
                return;
            }
            st.wave_fetching = true;
            let ids = st.queue.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
            (st.wave_token.clone(), ids)
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-wave-more".to_string())
            .spawn(move || {
                let res = me.yandex.wave_more(&tok, &qids);
                let mut st = me.state.lock().unwrap();
                st.wave_fetching = false;
                if let Ok((next_tok, tracks)) = res {
                    st.wave_token = next_tok;
                    for t in tracks {
                        if !st.queue.iter().any(|x| x.id == t.id) {
                            st.queue.push(t.clone());
                            st.wave_tracks.push(t);
                        }
                    }
                    st.session_dirty = true;
                }
            })
            .ok();
    }

    pub fn stop_wave(&self) {
        let mut st = self.state.lock().unwrap();
        st.wave_token.clear();
        st.wave_title.clear();
        st.wave_tracks.clear();
        st.wave_view = false;
    }

    pub fn play_from(&self, tracks: Vec<Track>, start_idx: usize) {
        if tracks.is_empty() || start_idx >= tracks.len() {
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            st.queue = tracks;
            st.history.clear();
            st.index = Some(start_idx);
            st.session_dirty = true;
        }
        self.start_current_track();
    }

    pub fn add_to_user_queue(&self, track: Track) {
        let mut st = self.state.lock().unwrap();
        st.user_queue.push(track);
        st.status_msg = i18n::get("status.queued");
    }

    pub fn remove_from_queue(&self, idx: usize) {
        let mut changed_current = false;
        {
            let mut st = self.state.lock().unwrap();
            if idx >= st.queue.len() {
                return;
            }
            st.queue.remove(idx);
            st.history.retain(|&i| i != idx);
            for h in &mut st.history {
                if *h > idx {
                    *h -= 1;
                }
            }
            if let Some(cur) = st.index {
                if st.queue.is_empty() {
                    st.index = None;
                    changed_current = true;
                } else if idx < cur {
                    st.index = Some(cur - 1);
                } else if idx == cur {
                    st.index = Some(cur.min(st.queue.len() - 1));
                    changed_current = true;
                }
            }
            st.session_dirty = true;
        }
        if changed_current {
            if self.state.lock().unwrap().index.is_some() {
                self.start_current_track();
            } else {
                self.audio.stop();
            }
        }
    }

    pub fn toggle_pause(&self) {
        let Some(cur) = self.current_track() else {
            return;
        };
        if cur.platform() == Platform::Spotify {
            let playing = self.spotify.remote_state().playing;
            let me = self.clone();
            std::thread::spawn(move || {
                let res = if playing {
                    me.spotify.pause_remote()
                } else {
                    me.spotify.resume_remote()
                };
                if let Err(e) = res {
                    me.state.lock().unwrap().error_msg = e;
                }
            });
        } else if self.audio.is_playing() {
            self.audio.pause();
        } else if self.audio.position_ms() > 0 || self.state.lock().unwrap().loading_track {
            self.audio.resume();
        } else {
            self.start_current_track();
        }
    }

    pub fn seek_to(&self, ms: u64) {
        let Some(cur) = self.current_track() else {
            return;
        };
        let target = ms.min(cur.duration_ms.max(ms));
        if cur.platform() == Platform::Spotify {
            let me = self.clone();
            std::thread::spawn(move || {
                let _ = me.spotify.seek_remote(target);
            });
        } else {
            self.audio.seek(target);
        }
    }

    pub fn seek_by(&self, delta_ms: i64) {
        let pos = self.position_ms() as i64 + delta_ms;
        let dur = self.duration_ms() as i64;
        let target = if dur > 0 {
            pos.clamp(0, dur)
        } else {
            pos.max(0)
        } as u64;
        self.seek_to(target);
    }

    pub fn next(&self) {
        self.advance(false);
    }

    pub fn prev(&self) {
        if self.position_ms() > 3500 {
            self.seek_to(0);
            return;
        }
        let target = {
            let mut st = self.state.lock().unwrap();
            if let Some(prev_idx) = st.history.pop()
                && prev_idx < st.queue.len()
            {
                st.index = Some(prev_idx);
                true
            } else if let Some(cur) = st.index
                && cur > 0
            {
                st.index = Some(cur - 1);
                true
            } else {
                false
            }
        };
        if target {
            self.start_current_track();
        } else {
            self.seek_to(0);
        }
    }

    fn advance(&self, auto_end: bool) {
        let mut should_play = false;
        let mut need_more_wave = false;
        {
            let mut st = self.state.lock().unwrap();
            if let Some(next_user) = st.user_queue.first().cloned() {
                st.user_queue.remove(0);
                let insert_at = st.index.map_or(0, |i| (i + 1).min(st.queue.len()));
                if let Some(cur) = st.index {
                    st.history.push(cur);
                }
                st.queue.insert(insert_at, next_user);
                st.index = Some(insert_at);
                should_play = true;
            } else if !st.queue.is_empty() {
                if auto_end && st.repeat == RepeatMode::One {
                    should_play = true;
                } else if st.shuffle && st.queue.len() > 1 {
                    let cur = st.index.unwrap_or(0);
                    st.history.push(cur);
                    st.rng_seed = st
                        .rng_seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let mut nxt = (st.rng_seed as usize) % st.queue.len();
                    if nxt == cur {
                        nxt = (nxt + 1) % st.queue.len();
                    }
                    st.index = Some(nxt);
                    should_play = true;
                } else {
                    let cur = st.index.unwrap_or(0);
                    if cur + 1 < st.queue.len() {
                        st.history.push(cur);
                        st.index = Some(cur + 1);
                        should_play = true;
                        if !st.wave_token.is_empty() && st.queue.len() - (cur + 1) <= 3 {
                            need_more_wave = true;
                        }
                    } else if st.repeat == RepeatMode::All || !auto_end {
                        st.history.push(cur);
                        st.index = Some(0);
                        should_play = true;
                    }
                }
            }
            st.session_dirty = true;
        }
        if need_more_wave {
            self.load_more_wave();
        }
        if should_play {
            self.start_current_track();
        }
    }

    pub fn dislike_current(&self) {
        let Some(cur) = self.current_track() else {
            return;
        };
        let (tok, pos_s) = {
            let st = self.state.lock().unwrap();
            (st.wave_token.clone(), self.position_ms() as f64 / 1000.0)
        };
        let me = self.clone();
        std::thread::spawn(move || {
            let _ = me.yandex.dislike(&cur.id);
            if !tok.is_empty() {
                me.yandex
                    .wave_feedback(&tok, "dislike", Some(&cur.id), pos_s);
            }
        });
        self.next();
    }

    pub fn toggle_shuffle(&self) {
        let mut st = self.state.lock().unwrap();
        st.shuffle = !st.shuffle;
        st.session_dirty = true;
    }

    pub fn cycle_repeat(&self) {
        let mut st = self.state.lock().unwrap();
        st.repeat = st.repeat.next();
        st.session_dirty = true;
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        {
            let mut st = self.state.lock().unwrap();
            st.volume = v;
            st.volume_popup_timer = self.hud_cfg.lock().unwrap().volume_seconds.max(1) as f32;
            st.session_dirty = true;
        }
        self.audio.set_volume(v);
        if self
            .current_track()
            .is_some_and(|t| t.platform() == Platform::Spotify)
        {
            let me = self.clone();
            std::thread::spawn(move || me.spotify.set_volume_remote(v));
        }
    }

    pub fn step_volume(&self, delta: f32) {
        let cur = self.state.lock().unwrap().volume;
        self.set_volume(cur + delta);
    }

    pub fn set_game_state(&self, game_music_vol: f32, game_active: bool, game_device: &str) {
        let duck = self.player_cfg.lock().unwrap().duck_when_inactive;
        self.audio.set_game_device(game_device);
        let mut factor = 1.0f32;
        if duck && !game_active {
            factor *= 0.28;
        }
        self.audio.set_game_volume_factor(factor);
        let mut st = self.state.lock().unwrap();
        st.game_music_volume = game_music_vol;
        st.game_active = game_active;
    }

    pub fn reopen_audio(&self) {
        self.audio.reopen_audio();
    }

    pub fn toggle_like(&self, track: Track) {
        let currently_liked = self.state.lock().unwrap().liked_ids.contains(&track.id);
        let want = !currently_liked;
        {
            let mut st = self.state.lock().unwrap();
            if want {
                st.liked_ids.insert(track.id.clone());
            } else {
                st.liked_ids.remove(&track.id);
                st.liked.retain(|t| t.id != track.id);
            }
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let res = me.provider_of(track.platform()).set_liked(&track, want);
            if let Err(e) = res {
                let mut st = me.state.lock().unwrap();
                if want {
                    st.liked_ids.remove(&track.id);
                } else {
                    st.liked_ids.insert(track.id.clone());
                }
                st.error_msg = e;
            }
        });
    }

    pub fn start_current_track(&self) {
        let (track, upcoming, task_gen, wave_tok) = {
            let mut st = self.state.lock().unwrap();
            let Some(idx) = st.index else { return };
            let Some(tr) = st.queue.get(idx).cloned() else {
                return;
            };
            st.play_gen = st.play_gen.wrapping_add(1);
            st.loading_track = true;
            st.error_msg.clear();
            st.local_end_reported_id.clear();
            st.active_audio_pcm = None;
            let p_cnt = self.player_cfg.lock().unwrap().prefetch_count.max(0) as usize;
            let mut up = Vec::new();
            for offset in 1..=p_cnt {
                if let Some(nxt) = st.queue.get(idx + offset) {
                    up.push(nxt.id.clone());
                }
            }
            (tr, up, st.play_gen, st.wave_token.clone())
        };

        self.pending_end.store(false, Ordering::SeqCst);
        let upcoming_first = upcoming.first().map(String::as_str);
        lyrics::set_active_track_ids(Some(&track.id), upcoming_first);
        self.provider_of(track.platform())
            .on_queue_change(&upcoming, Some(&track.id));

        if track.platform() != Platform::Spotify {
            self.audio.stop();
        }
        self.request_lyrics_for(&track, None);

        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-play".to_string())
            .spawn(move || {
                let platform = track.platform();
                if platform == Platform::Spotify {
                    let res = me.spotify.play_remote(&track, 0);
                    let mut st = me.state.lock().unwrap();
                    if st.play_gen != task_gen {
                        return;
                    }
                    st.loading_track = false;
                    if let Err(e) = res {
                        st.error_msg = e;
                    }
                    return;
                }

                let loaded = me.provider_of(platform).load_audio(&track);
                if me.state.lock().unwrap().play_gen != task_gen {
                    return;
                }
                match loaded {
                    Ok(PlaybackSource::Bytes { format, data }) => {
                        let flag = Arc::clone(&me.pending_end);
                        me.audio.play(format, data.clone(), 0, move || {
                            flag.store(true, Ordering::SeqCst);
                        });
                        if !wave_tok.is_empty() && platform == Platform::Yandex {
                            me.yandex.wave_feedback(
                                &wave_tok,
                                "trackStarted",
                                Some(&track.id),
                                0.0,
                            );
                        }
                        let mut st = me.state.lock().unwrap();
                        if st.play_gen != task_gen {
                            return;
                        }
                        st.loading_track = false;
                        if let Some(err) = me.audio.last_error() {
                            st.error_msg = err;
                        }
                        drop(st);
                        // Decode PCM once in background for Level B DSP aligner if needed
                        let me2 = me.clone();
                        let tr_id = track.id.clone();
                        std::thread::Builder::new()
                            .name("overtone-dsp-pcm".to_string())
                            .spawn(move || {
                                if let Some(dsp) = PlaybackSource::decode_dsp_input(format, &data) {
                                    let needs_retry = {
                                        let mut st = me2.state.lock().unwrap();
                                        if st.play_gen != task_gen {
                                            return;
                                        }
                                        st.active_audio_pcm =
                                            Some((tr_id.clone(), Arc::clone(&dsp)));
                                        st.lyrics.as_ref().is_none_or(|r| r.candidates.is_empty())
                                    };
                                    if needs_retry {
                                        me2.request_lyrics_for(&track, Some(dsp));
                                    }
                                }
                            })
                            .ok();
                    }
                    Ok(PlaybackSource::External) => {
                        me.state.lock().unwrap().loading_track = false;
                    }
                    Err(e) => {
                        let mut st = me.state.lock().unwrap();
                        st.loading_track = false;
                        st.error_msg = e;
                    }
                }
            })
            .ok();
    }

    pub fn request_lyrics_for(&self, track: &Track, dsp_hint: Option<Arc<AudioDspInput>>) {
        lyrics::set_active_track_ids(Some(&track.id), None);
        let task_gen = {
            let mut st = self.state.lock().unwrap();
            if st.lyrics_track_id != track.id {
                st.lyrics = None;
                st.lyrics_track_id = track.id.clone();
            }
            st.lyrics_loading = true;
            st.lyrics_gen = st.lyrics_gen.wrapping_add(1);
            st.lyrics_gen
        };
        let cfg = self.player_cfg.lock().unwrap().clone();
        let tr = track.clone();
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-lyrics".to_string())
            .spawn(move || {
                let dsp = dsp_hint.or_else(|| {
                    me.state
                        .lock()
                        .unwrap()
                        .active_audio_pcm
                        .as_ref()
                        .filter(|(id, _)| id == &tr.id)
                        .map(|(_, d)| Arc::clone(d))
                });
                let plat_res: Arc<Mutex<Option<crate::providers::PlatformLyrics>>> =
                    Arc::new(Mutex::new(None));
                let me_s = me.clone();
                let pr_s = Arc::clone(&plat_res);
                let me_p = me.clone();
                let pr_p = Arc::clone(&plat_res);
                let tr_p = tr.clone();
                let res = lyrics::resolve_lyrics(
                    &tr,
                    &cfg,
                    move |t| {
                        let fetched = me_s.provider_of(t.platform()).fetch_lyrics(t);
                        *pr_s.lock().unwrap() = Some(fetched.clone());
                        fetched.synced
                    },
                    move || {
                        let existing = pr_p.lock().unwrap().clone();
                        match existing {
                            Some(pl) => pl.plain,
                            None => me_p.provider_of(tr_p.platform()).fetch_lyrics(&tr_p).plain,
                        }
                    },
                    dsp,
                );
                if !lyrics::is_track_still_wanted(&tr) {
                    return;
                }
                let mut st = me.state.lock().unwrap();
                if st.lyrics_gen != task_gen {
                    return;
                }
                st.lyrics_loading = false;
                st.lyrics = Some(res);
            })
            .ok();
    }

    pub fn active_lyrics(&self) -> Option<LyricCandidate> {
        let st = self.state.lock().unwrap();
        let res = st.lyrics.as_ref()?;
        res.candidates.get(res.active_idx).cloned()
    }

    pub fn plain_lyrics_waiting_for_dsp(&self) -> bool {
        if self.player_cfg.lock().unwrap().dsp_auto_align {
            return false;
        }
        self.state
            .lock()
            .unwrap()
            .lyrics
            .as_ref()
            .is_some_and(|r| r.candidates.is_empty() && r.plain_available)
    }

    /// Compute the effective playback position (ms) after applying per-track and global lyrics shift.
    /// Negative shift moves timestamps earlier (effective pos increases); positive shift delays them.
    pub fn adjusted_lyrics_pos_ms(
        &self,
        raw_pos_ms: u64,
        track_id: &str,
        global_shift_ms: i32,
    ) -> u64 {
        let shift = self.track_shift_ms(track_id) + global_shift_ms as i64;
        (raw_pos_ms as i64 - shift).max(0) as u64
    }

    pub fn current_lyric_index(&self) -> Option<usize> {
        let pos = self.position_ms();
        let tr_id = self.current_track().map(|t| t.id).unwrap_or_default();
        let global_shift = self.hud_cfg.lock().unwrap().lyrics_shift_ms;
        let adjusted = self.adjusted_lyrics_pos_ms(pos, &tr_id, global_shift);
        let st = self.state.lock().unwrap();
        let cand = st
            .lyrics
            .as_ref()?
            .candidates
            .get(st.lyrics.as_ref()?.active_idx)?;
        if cand.lines.is_empty() {
            return None;
        }
        lyrics::current_index(&cand.lines, adjusted)
    }

    pub fn hud_config(&self) -> HudConfig {
        self.hud_cfg.lock().unwrap().clone()
    }

    pub fn update_hud_config(&self, f: impl FnOnce(&mut HudConfig)) {
        {
            let mut g = self.hud_cfg.lock().unwrap();
            f(&mut g);
            g.sanitize();
        }
        self.save_hud_config();
    }

    pub fn player_config(&self) -> PlayerConfig {
        self.player_cfg.lock().unwrap().clone()
    }

    pub fn update_player_config(&self, f: impl FnOnce(&mut PlayerConfig)) {
        {
            let mut g = self.player_cfg.lock().unwrap();
            f(&mut g);
            g.sanitize();
        }
        self.save_player_config();
    }

    pub fn hotkey_config(&self) -> HotkeyConfig {
        self.hotkey_cfg.lock().unwrap().clone()
    }

    pub fn update_hotkey_config(&self, f: impl FnOnce(&mut HotkeyConfig)) {
        {
            let mut g = self.hotkey_cfg.lock().unwrap();
            f(&mut g);
        }
        self.save_hotkey_config();
    }

    pub fn is_idle(&self) -> bool {
        let cur = self.current_track();
        match cur.as_ref().map(Track::platform) {
            None => true,
            Some(Platform::Spotify) => false,
            Some(_) => {
                !self.is_playing()
                    && self.position_ms() == 0
                    && !self.state.lock().unwrap().loading_track
            }
        }
    }

    pub fn volume(&self) -> f32 {
        self.state.lock().unwrap().volume
    }

    pub fn toggle_mute(&self) {
        let target = {
            let mut st = self.state.lock().unwrap();
            if st.volume > 0.01 {
                st.saved_unmuted_volume = st.volume;
                0.0
            } else {
                st.saved_unmuted_volume.max(0.5)
            }
        };
        self.set_volume(target);
    }

    pub fn track_shift_ms(&self, track_id: &str) -> i64 {
        self.state
            .lock()
            .unwrap()
            .track_shifts
            .get(track_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn add_track_shift_ms(&self, track_id: &str, delta_ms: i64) {
        if track_id.is_empty() {
            return;
        }
        let mut st = self.state.lock().unwrap();
        let entry = st.track_shifts.entry(track_id.to_owned()).or_insert(0);
        *entry = (*entry + delta_ms).clamp(-10_000, 10_000);
        st.session_dirty = true;
    }

    pub fn reset_track_shift_ms(&self, track_id: &str) {
        let mut st = self.state.lock().unwrap();
        if st.track_shifts.remove(track_id).is_some() {
            st.session_dirty = true;
        }
    }

    pub fn toggle_hud_master(&self) -> bool {
        let mut next = false;
        self.update_hud_config(|c| {
            c.master = !c.master;
            next = c.master;
        });
        next
    }

    pub fn toggle_world_lyrics(&self) -> bool {
        let mut next = false;
        self.update_hud_config(|c| {
            c.world_lyrics = !c.world_lyrics;
            next = c.world_lyrics;
        });
        next
    }

    pub fn queue_snapshot(&self) -> Vec<Track> {
        self.state.lock().unwrap().queue.clone()
    }

    pub fn wave_snapshot(&self) -> Vec<Track> {
        self.state.lock().unwrap().wave_tracks.clone()
    }

    pub fn wave_shown(&self) -> bool {
        let st = self.state.lock().unwrap();
        !st.wave_token.is_empty() && st.wave_view
    }

    pub fn play_queue_index(&self, idx: usize) {
        let ok = {
            let mut st = self.state.lock().unwrap();
            if idx < st.queue.len() {
                if let Some(cur) = st.index {
                    st.history.push(cur);
                }
                st.index = Some(idx);
                st.session_dirty = true;
                true
            } else {
                false
            }
        };
        if ok {
            self.start_current_track();
        }
    }

    pub fn play_wave_index(&self, idx: usize) {
        let wave = self.wave_snapshot();
        if idx < wave.len() {
            self.play_from(wave, idx);
        }
    }

    pub fn queue_move_up(&self, idx: usize) {
        let mut st = self.state.lock().unwrap();
        if idx == 0 || idx >= st.queue.len() {
            return;
        }
        st.queue.swap(idx, idx - 1);
        if st.index == Some(idx) {
            st.index = Some(idx - 1);
        } else if st.index == Some(idx - 1) {
            st.index = Some(idx);
        }
        st.session_dirty = true;
    }

    pub fn play_next_track(&self, track: Track) {
        let mut st = self.state.lock().unwrap();
        st.user_queue.insert(0, track);
        st.status_msg = i18n::get("status.queued");
    }

    pub fn open_artist_of(&self, track: &Track) {
        if track.artist.trim().is_empty() {
            return;
        }
        let id = if matches!(
            track.platform(),
            crate::model::Platform::Yandex | crate::model::Platform::YouTube
        ) {
            format!("track:{}", track.id)
        } else {
            track.id.clone()
        };
        let entity = MusicEntity {
            kind: crate::model::EntityKind::Artist,
            id,
            title: track.artist.clone(),
            subtitle: String::new(),
            cover_url: track.cover_url.clone(),
        };
        self.open_entity(entity);
    }

    pub fn open_album_of(&self, track: &Track) {
        if track.album.is_empty() {
            return;
        }
        let id = if matches!(
            track.platform(),
            crate::model::Platform::Yandex | crate::model::Platform::YouTube
        ) {
            format!("track:{}", track.id)
        } else {
            track.id.clone()
        };
        let entity = MusicEntity {
            kind: crate::model::EntityKind::Album,
            id,
            title: track.album.clone(),
            subtitle: track.artist.clone(),
            cover_url: track.cover_url.clone(),
        };
        self.open_entity(entity);
    }

    pub fn can_like(&self, track: Option<&Track>) -> bool {
        let Some(t) = track else {
            return false;
        };
        match t.platform() {
            Platform::Local => false,
            Platform::SoundCloud => self.soundcloud.has_auth(),
            Platform::Yandex | Platform::Spotify | Platform::YouTube => true,
        }
    }

    pub fn account_name(&self, platform: Platform) -> String {
        let ts = self.tokens.lock().unwrap();
        match platform {
            Platform::Yandex => ts
                .yandex_accounts
                .first()
                .filter(|a| a.token == ts.yandex_token)
                .map(|a| a.label.clone())
                .unwrap_or_default(),
            Platform::SoundCloud => ts.soundcloud_user.clone(),
            Platform::Spotify => ts.spotify_user.clone(),
            Platform::YouTube => ts.youtube_user.clone(),
            Platform::Local => String::new(),
        }
    }

    pub fn saved_yandex_accounts(&self) -> Vec<crate::model::Account> {
        self.tokens.lock().unwrap().yandex_accounts.clone()
    }

    pub fn forget_yandex_account(&self, token: &str) {
        self.tokens.lock().unwrap().remove_yandex_by_token(token);
    }

    pub fn login_yandex_saved(&self, label: String, token: String) {
        self.tokens.lock().unwrap().add_yandex_account(label, token);
        self.load_liked();
    }

    pub fn submit_yandex_token(&self, token: String) {
        let me = self.clone();
        self.state.lock().unwrap().busy = true;
        std::thread::Builder::new()
            .name("overtone-ya-login".to_string())
            .spawn(move || {
                let res = me.yandex.verify_and_save_token(&token);
                let mut st = me.state.lock().unwrap();
                st.busy = false;
                match res {
                    Ok(_) => {
                        st.error_msg.clear();
                        drop(st);
                        me.load_liked();
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn link_soundcloud(&self, input: String) {
        let me = self.clone();
        self.state.lock().unwrap().busy = true;
        std::thread::Builder::new()
            .name("overtone-sc-link".to_string())
            .spawn(move || {
                let trimmed = input.trim();
                let res = if SoundCloudProvider::is_valid_oauth_token(trimmed) {
                    me.soundcloud.verify_and_save_token(trimmed)
                } else {
                    me.soundcloud.set_profile_url(trimmed)
                };
                let mut st = me.state.lock().unwrap();
                st.busy = false;
                match res {
                    Ok(_) => {
                        st.error_msg.clear();
                        drop(st);
                        me.load_liked();
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn login_spotify(&self, client_id: String) {
        self.spotify.set_client_id(&client_id);
        let me = self.clone();
        {
            let mut st = self.state.lock().unwrap();
            st.busy = true;
            st.error_msg.clear();
            st.status_msg = i18n::get("status.sp_browser");
        }
        std::thread::Builder::new()
            .name("overtone-sp-login".to_string())
            .spawn(move || {
                let res = me.spotify.start_pkce_login();
                let mut st = me.state.lock().unwrap();
                st.busy = false;
                if let Err(e) = res {
                    st.status_msg.clear();
                    st.error_msg = e;
                } else {
                    st.error_msg.clear();
                }
            })
            .ok();
    }

    pub fn login_youtube(&self, cookie_input: String) {
        let me = self.clone();
        {
            let mut st = self.state.lock().unwrap();
            st.busy = true;
            st.error_msg.clear();
        }
        std::thread::Builder::new()
            .name("overtone-yt-login".to_string())
            .spawn(move || {
                let res = me.youtube.verify_and_save_cookie(&cookie_input);
                let mut st = me.state.lock().unwrap();
                st.busy = false;
                match res {
                    Ok(_) => {
                        st.error_msg.clear();
                        st.status_msg.clear();
                        drop(st);
                        me.load_liked();
                    }
                    Err(e) => st.error_msg = e,
                }
            })
            .ok();
    }

    pub fn logout_platform(&self, platform: Platform) {
        match platform {
            Platform::Yandex => {
                let mut ts = self.tokens.lock().unwrap();
                ts.yandex_token.clear();
                ts.save();
            }
            Platform::SoundCloud => self.soundcloud.clear_auth(),
            Platform::Spotify => self.spotify.logout(),
            Platform::YouTube => self.youtube.clear_auth(),
            Platform::Local => {}
        }
        let mut st = self.state.lock().unwrap();
        if st.platform == platform {
            st.page_trail.clear();
            st.list_page = 0;
            st.queue_mode = false;
            st.wave_view = false;
            st.wave_fetching = false;
            st.search_tracks.clear();
            st.search_entities.clear();
            st.liked.clear();
            st.playlists.clear();
            st.stations.clear();
            st.status_msg.clear();
            st.error_msg.clear();
            st.browse_gen = st.browse_gen.wrapping_add(1);
            st.session_dirty = true;
        }
    }

    pub fn logout_active(&self) {
        let p = self.active_platform();
        self.logout_platform(p);
    }

    pub fn wave_dislike(&self) {
        let Some(cur) = self.current_track() else {
            return;
        };
        if cur.platform() != Platform::Yandex {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-dislike".to_string())
            .spawn(move || {
                let _ = me.yandex.dislike(&cur.id);
            })
            .ok();
        self.dislike_current();
    }

    pub fn next_lyric_candidate(&self) {
        let mut st = self.state.lock().unwrap();
        if let Some(ref mut lr) = st.lyrics
            && lr.candidates.len() > 1
        {
            lr.active_idx = (lr.active_idx + 1) % lr.candidates.len();
        }
    }

    pub fn cycle_lyrics_source_mode(&self) {
        {
            let mut cfg = self.player_cfg.lock().unwrap();
            cfg.lyrics_source_mode = cfg.lyrics_source_mode.next();
            cfg.save();
        }
        if let Some(tr) = self.current_track() {
            self.request_lyrics_for(&tr, None);
        }
    }

    pub fn toggle_dsp_auto_align(&self) {
        {
            let mut cfg = self.player_cfg.lock().unwrap();
            cfg.dsp_auto_align = !cfg.dsp_auto_align;
            cfg.save();
        }
        if let Some(tr) = self.current_track() {
            self.request_lyrics_for(&tr, None);
        }
    }

    pub fn sync_current_lyric_to_now(&self) -> bool {
        let Some(track) = self.current_track() else {
            return false;
        };
        let now_pos = self.position_ms();
        let line_idx = self.current_lyric_index().unwrap_or(0);
        let mut st = self.state.lock().unwrap();
        let Some(ref mut lr) = st.lyrics else {
            return false;
        };
        let Some(cand) = lr.candidates.get_mut(lr.active_idx) else {
            return false;
        };
        if cand.lines.is_empty() {
            return false;
        }
        let idx = line_idx.min(cand.lines.len() - 1);
        dsp_aligner::reanchor_tail_from(&mut cand.lines, idx, now_pos);
        let _ = lyrics_cache::save_custom(&track, &cand.lines);
        cand.source = crate::model::LyricSourceKind::Custom;
        st.status_msg = i18n::get("lyrics.synced_now");
        true
    }

    pub fn import_lyrics_file(&self, path: &Path) -> Result<String, String> {
        let Some(track) = self.current_track() else {
            return Err("No active track for lyrics import".to_owned());
        };
        let (timed, plain) = lyrics_cache::parse_dropped_lyrics_file(path)?;
        let final_lines = if !timed.is_empty() {
            timed
        } else {
            let dsp_opt = self
                .state
                .lock()
                .unwrap()
                .active_audio_pcm
                .as_ref()
                .filter(|(id, _)| id == &track.id)
                .map(|(_, d)| Arc::clone(d));
            let Some(audio) = dsp_opt else {
                return Err("Plain lyrics dropped but audio PCM not ready".to_owned());
            };
            let aligned = dsp_aligner::align_plain_lyrics(
                &plain,
                &audio.pcm,
                audio.channels,
                audio.sample_rate,
                track.duration_ms,
            );
            if aligned.is_empty() {
                return Err("Failed to align plain lyrics".to_owned());
            }
            aligned
        };
        lyrics_cache::save_custom(&track, &final_lines)?;
        self.request_lyrics_for(&track, None);
        Ok(i18n::get("lyrics.custom_saved"))
    }

    pub fn remove_custom_lyrics(&self) {
        if let Some(tr) = self.current_track() {
            lyrics_cache::remove_custom(&tr);
            self.request_lyrics_for(&tr, None);
        }
    }

    pub fn add_local_paths(&self, paths: &[PathBuf]) -> usize {
        self.local.add_paths(paths)
    }

    pub fn open_local_folder(&self) {
        self.local.open_folder();
    }

    pub fn start_yandex_device_auth(&self) -> Result<DeviceAuthSession, String> {
        self.yandex.start_device_auth()
    }

    pub fn poll_yandex_device_auth(&self, device_code: &str) -> DevicePollResult {
        self.yandex.poll_device_auth(device_code)
    }

    pub fn should_capture_system_audio(&self, cfg: &PlayerConfig) -> bool {
        let hud = self.hud_cfg.lock().unwrap();
        let cur_plat = self.active_platform();
        hud.viz_enabled
            && match hud.viz_source {
                crate::config::VizSource::System => true,
                crate::config::VizSource::Auto => cur_plat == Platform::Spotify,
                crate::config::VizSource::Mod => {
                    let _ = cfg;
                    false
                }
            }
    }

    pub fn tick(&self, dt_sec: f32) {
        let cur = self.current_track();
        let is_spotify = cur
            .as_ref()
            .is_some_and(|t| t.platform() == Platform::Spotify);

        // Advance on local audio EOF
        if !is_spotify && self.pending_end.swap(false, Ordering::SeqCst) {
            self.advance(true);
        }

        // Spotify remote poller (every 1000ms playing / 400ms near track end)
        if is_spotify {
            let remote = self.spotify.remote_state();
            let near_end = remote.playing
                && remote.duration_ms > 0
                && remote.duration_ms.saturating_sub(remote.position_ms) <= END_WINDOW_MS;
            let interval = if near_end {
                Duration::from_millis(400)
            } else {
                Duration::from_millis(1000)
            };
            let need_poll = {
                let mut st = self.state.lock().unwrap();
                if st.last_spotify_poll.elapsed() >= interval {
                    st.last_spotify_poll = Instant::now();
                    true
                } else {
                    false
                }
            };
            if need_poll {
                let me = self.clone();
                std::thread::Builder::new()
                    .name("overtone-spotify-poll".to_string())
                    .spawn(move || {
                        let prev = me.spotify.remote_state();
                        let next = me.spotify.poll_remote();
                        if prev.playing
                            && !next.playing
                            && prev.duration_ms > 0
                            && prev.duration_ms.saturating_sub(prev.position_ms) <= END_WINDOW_MS
                            && next.position_ms <= 2000
                        {
                            me.advance(true);
                        }
                    })
                    .ok();
            }
        }

        // Update visualizer
        {
            let hud = self.hud_cfg.lock().unwrap();
            if hud.viz_enabled || self.state.lock().unwrap().tab == Tab::Search {
                let mut viz = self.viz.lock().unwrap();
                viz.update(
                    hud.viz_bars as usize,
                    hud.viz_sensitivity,
                    hud.viz_smoothing,
                    hud.viz_auto_sens,
                    hud.viz_stereo,
                    false,
                );
            }
        }

        // Volume popup countdown & autosave every 15s
        let mut need_save = false;
        {
            let mut st = self.state.lock().unwrap();
            if st.volume_popup_timer > 0.0 {
                st.volume_popup_timer = (st.volume_popup_timer - dt_sec).max(0.0);
            }
            if st.session_dirty && st.last_session_save.elapsed() >= Duration::from_secs(15) {
                st.last_session_save = Instant::now();
                st.session_dirty = false;
                need_save = true;
            }
        }
        if need_save {
            self.save_session();
        }

        // Update Discord Rich Presence snapshot
        let cfg = self.player_cfg.lock().unwrap();
        let pos = self.position_ms();
        let playing = self.is_playing();
        self.discord.update_snapshot(PresenceSnapshot {
            track: cur,
            idle: !playing && self.current_track().is_none(),
            playing,
            position_ms: pos as i64,
            enabled: cfg.discord_presence,
        });
    }

    /// Gracefully persists session & configs and halts local audio output on client exit.
    pub fn shutdown(&self) {
        self.save_session();
        self.save_player_config();
        self.save_hud_config();
        self.save_hotkey_config();
        self.audio.stop();
        self.discord.update_snapshot(PresenceSnapshot {
            track: None,
            idle: true,
            playing: false,
            position_ms: 0,
            enabled: false,
        });
        self.discord.tick();
    }
}

/// Opens an external HTTP(S) URL (OAuth login page, info links) using the system browser.
pub fn open_browser_url(url: &str) {
    let trimmed = url.trim();
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        let _ = open::that_detached(trimmed);
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LyricLine;

    #[test]
    fn queue_shuffle_repeat_and_reanchor_work() {
        let ctrl = PlayerController::new(false);
        let t1 = Track {
            id: "1".into(),
            title: "One".into(),
            artist: "A".into(),
            album: String::new(),
            duration_ms: 10_000,
            cover_url: None,
            provider: Platform::Local.id().into(),
        };
        let t2 = Track {
            id: "2".into(),
            title: "Two".into(),
            artist: "B".into(),
            album: String::new(),
            duration_ms: 10_000,
            cover_url: None,
            provider: Platform::Local.id().into(),
        };
        {
            let mut st = ctrl.state.lock().unwrap();
            st.queue = vec![t1.clone(), t2.clone()];
            st.index = Some(0);
            st.lyrics = Some(LyricsLookupResult {
                candidates: vec![LyricCandidate {
                    source: crate::model::LyricSourceKind::LrcLib,
                    lines: vec![
                        LyricLine::new(1000, "First"),
                        LyricLine::new(4000, "Second"),
                    ],
                    score: 100.0,
                    has_word_timings: false,
                    is_auto_dsp: false,
                }],
                active_idx: 0,
                plain_available: false,
            });
        }
        ctrl.cycle_repeat();
        assert_eq!(ctrl.state.lock().unwrap().repeat, RepeatMode::All);
        assert!(ctrl.sync_current_lyric_to_now());
        assert_eq!(ctrl.active_lyrics().unwrap().lines[0].time_ms, 0);
        ctrl.remove_from_queue(1);
        assert_eq!(ctrl.state.lock().unwrap().queue.len(), 1);
    }
}
