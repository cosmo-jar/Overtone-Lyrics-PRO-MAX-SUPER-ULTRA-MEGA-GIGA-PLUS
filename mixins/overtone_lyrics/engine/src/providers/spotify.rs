//! Spotify Connect + PKCE OAuth + optional Linux MPRIS provider, ported 1:1 from `SpotifyProvider.java`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::TokenStore;
use crate::i18n;
use crate::model::{EntityKind, MusicEntity, Page, Platform, Track};
use crate::net;
use crate::providers::{
    MusicProvider, PlatformLyrics, PlaybackSource, SearchResult, json_id, json_str, json_u64,
};

const API: &str = "https://api.spotify.com/v1";
const ACCOUNTS: &str = "https://accounts.spotify.com";
pub const REDIRECT_URI: &str = "http://127.0.0.1:43821/callback";
const CALLBACK_ADDR: &str = "127.0.0.1:43821";
const SCOPES: &str = "user-read-private user-library-read user-library-modify playlist-read-private playlist-read-collaborative user-read-playback-state user-modify-playback-state";

pub const SETTLE_MS: u64 = 7000;
pub const END_WINDOW_MS: u64 = 3000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpotifyAuthState {
    Idle,
    WaitingForBrowser,
    Authorized(String),
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct SpotifyRemoteState {
    pub active_id: String,
    pub linked_from_id: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub device_id: String,
    pub device_name: String,
    pub no_device: bool,
    pub last_cmd_at_ms: u64,
    pub expected_id: String,
    pub expected_pos_ms: Option<u64>,
    pub expected_playing: Option<bool>,
    pub updated_at_ms: u64,
}

#[derive(Clone)]
pub struct SpotifyProvider {
    tokens: Arc<Mutex<TokenStore>>,
    auth_state: Arc<Mutex<SpotifyAuthState>>,
    remote: Arc<Mutex<SpotifyRemoteState>>,
    login_gen: Arc<AtomicU64>,
}

impl SpotifyProvider {
    pub fn new(tokens: Arc<Mutex<TokenStore>>) -> Self {
        Self {
            tokens,
            auth_state: Arc::new(Mutex::new(SpotifyAuthState::Idle)),
            remote: Arc::new(Mutex::new(SpotifyRemoteState::default())),
            login_gen: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn is_valid_client_id(cid: &str) -> bool {
        let s = cid.trim();
        s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
    }

    pub fn cancel_login(&self) {
        self.login_gen.fetch_add(1, Ordering::SeqCst);
        let mut st = self.auth_state.lock().unwrap();
        if matches!(*st, SpotifyAuthState::WaitingForBrowser) {
            *st = SpotifyAuthState::Idle;
        }
    }

    pub fn redirect_uri() -> &'static str {
        REDIRECT_URI
    }

    pub fn client_id(&self) -> String {
        self.tokens
            .lock()
            .unwrap()
            .spotify_client_id
            .trim()
            .to_owned()
    }

    pub fn set_client_id(&self, cid: &str) {
        let mut ts = self.tokens.lock().unwrap();
        ts.spotify_client_id = cid.trim().to_owned();
        ts.save();
    }

    pub fn auth_state(&self) -> SpotifyAuthState {
        self.auth_state.lock().unwrap().clone()
    }

    pub fn remote_state(&self) -> SpotifyRemoteState {
        self.remote.lock().unwrap().clone()
    }

    pub fn logout(&self) {
        self.cancel_login();
        let mut ts = self.tokens.lock().unwrap();
        ts.spotify_access_token.clear();
        ts.spotify_refresh_token.clear();
        ts.spotify_expires_at_ms = 0;
        ts.spotify_user.clear();
        ts.save();
        *self.auth_state.lock().unwrap() = SpotifyAuthState::Idle;
    }

    pub fn start_pkce_login(&self) -> Result<(), String> {
        let cid = self.client_id();
        if !Self::is_valid_client_id(&cid) {
            return Err(i18n::get("err.sp_bad_client"));
        }
        let my_gen = self.login_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let verifier = random_base64url(48);
        let challenge = pkce_challenge(&verifier);
        let expected_state = random_base64url(16);

        // Give any superseded callback thread a moment to drop its socket cleanly.
        let mut listener_opt = None;
        let mut last_err = None;
        for attempt in 0..15 {
            match TcpListener::bind(CALLBACK_ADDR) {
                Ok(l) => {
                    listener_opt = Some(l);
                    break;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::AddrInUse
                        && attempt < 14
                        && self.login_gen.load(Ordering::SeqCst) == my_gen =>
                {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(e) => {
                    return Err(format!("Failed to bind {CALLBACK_ADDR}: {e}"));
                }
            }
        }
        let listener = listener_opt.ok_or_else(|| {
            format!(
                "Failed to bind {CALLBACK_ADDR}: {}",
                last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "address in use".to_owned())
            )
        })?;
        let _ = listener.set_nonblocking(true);

        let auth_url = net::build_query_url(
            &format!("{ACCOUNTS}/authorize"),
            &[
                ("client_id", &cid),
                ("response_type", "code"),
                ("redirect_uri", REDIRECT_URI),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge),
                ("state", &expected_state),
                ("scope", SCOPES),
                ("show_dialog", "true"),
            ],
        );

        *self.auth_state.lock().unwrap() = SpotifyAuthState::WaitingForBrowser;
        if let Err(e) = open::that(&auth_url) {
            drop(listener);
            *self.auth_state.lock().unwrap() =
                SpotifyAuthState::Failed(format!("Could not open browser: {e}"));
            return Err(format!("Could not open browser: {e}"));
        }

        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-spotify-pkce".to_string())
            .spawn(move || {
                me.wait_pkce_callback(listener, cid, verifier, expected_state, my_gen);
            })
            .ok();
        Ok(())
    }

    fn wait_pkce_callback(
        &self,
        listener: TcpListener,
        cid: String,
        verifier: String,
        expected_state: String,
        my_gen: u64,
    ) {
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            if self.login_gen.load(Ordering::SeqCst) != my_gen {
                return;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                    if !path.starts_with("/callback") {
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let parsed = url::Url::parse(&format!("http://127.0.0.1{path}")).ok();
                    let mut code = String::new();
                    let mut state = String::new();
                    let mut err = String::new();
                    if let Some(u) = parsed {
                        for (k, v) in u.query_pairs() {
                            match k.as_ref() {
                                "code" => code = v.into_owned(),
                                "state" => state = v.into_owned(),
                                "error" => err = v.into_owned(),
                                _ => {}
                            }
                        }
                    }
                    let html = if !err.is_empty() || code.is_empty() || state != expected_state {
                        callback_html(false, &i18n::get("err.sp_denied"))
                    } else {
                        callback_html(true, &i18n::get("login.sp_done"))
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        html.len(),
                        html
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();

                    if self.login_gen.load(Ordering::SeqCst) != my_gen {
                        return;
                    }
                    if !err.is_empty() {
                        *self.auth_state.lock().unwrap() = SpotifyAuthState::Failed(err);
                        return;
                    }
                    if code.is_empty() || state != expected_state {
                        *self.auth_state.lock().unwrap() =
                            SpotifyAuthState::Failed("Invalid PKCE callback state".to_owned());
                        return;
                    }
                    match self.exchange_pkce_code(&cid, &code, &verifier) {
                        Ok(user) => {
                            if self.login_gen.load(Ordering::SeqCst) == my_gen {
                                *self.auth_state.lock().unwrap() =
                                    SpotifyAuthState::Authorized(user);
                            }
                        }
                        Err(e) => {
                            if self.login_gen.load(Ordering::SeqCst) == my_gen {
                                *self.auth_state.lock().unwrap() = SpotifyAuthState::Failed(e);
                            }
                        }
                    }
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(e) => {
                    if self.login_gen.load(Ordering::SeqCst) == my_gen {
                        *self.auth_state.lock().unwrap() = SpotifyAuthState::Failed(e.to_string());
                    }
                    return;
                }
            }
        }
        if self.login_gen.load(Ordering::SeqCst) == my_gen {
            *self.auth_state.lock().unwrap() =
                SpotifyAuthState::Failed(i18n::get("err.sp_timeout"));
        }
    }

    fn exchange_pkce_code(&self, cid: &str, code: &str, verifier: &str) -> Result<String, String> {
        let reply = net::post_form(
            &format!("{ACCOUNTS}/api/token"),
            &[],
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
                ("client_id", cid),
                ("code_verifier", verifier),
            ],
        )?;
        let val: Value = serde_json::from_slice(&reply.body)
            .map_err(|e| format!("Spotify token JSON error: {e}"))?;
        let access = json_str(&val, "access_token").to_owned();
        if access.is_empty() {
            return Err(format!(
                "Spotify auth failed: {}",
                json_str(&val, "error_description")
            ));
        }
        let refresh = json_str(&val, "refresh_token").to_owned();
        let expires_in = json_u64(&val, "expires_in").max(300) as i64;
        {
            let mut ts = self.tokens.lock().unwrap();
            ts.spotify_access_token = access;
            if !refresh.is_empty() {
                ts.spotify_refresh_token = refresh;
            }
            ts.spotify_expires_at_ms = now_ms() as i64 + expires_in * 1000;
            ts.save();
        }
        let me = self.api_get_json("/me")?;
        let user = json_str(&me, "display_name");
        let fallback_id = json_id(&me, "id");
        let name = if user.is_empty() {
            fallback_id
        } else {
            user.to_owned()
        };
        {
            let mut ts = self.tokens.lock().unwrap();
            ts.spotify_user = name.clone();
            ts.save();
        }
        Ok(name)
    }

    fn refresh_if_needed(&self) -> Result<String, String> {
        let (access, refresh, cid, exp) = {
            let ts = self.tokens.lock().unwrap();
            (
                ts.spotify_access_token.clone(),
                ts.spotify_refresh_token.clone(),
                ts.spotify_client_id.clone(),
                ts.spotify_expires_at_ms,
            )
        };
        if !access.is_empty() && (exp <= 0 || (now_ms() as i64) < exp - 60_000) {
            return Ok(access);
        }
        if refresh.is_empty() || cid.is_empty() {
            if !access.is_empty() {
                return Ok(access);
            }
            return Err("Not signed in to Spotify".to_owned());
        }
        let reply = net::post_form(
            &format!("{ACCOUNTS}/api/token"),
            &[],
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", &cid),
            ],
        )?;
        let val: Value = serde_json::from_slice(&reply.body)
            .map_err(|e| format!("Spotify refresh JSON error: {e}"))?;
        let new_access = json_str(&val, "access_token").to_owned();
        if new_access.is_empty() {
            return Err("Spotify session expired — please sign in again".to_owned());
        }
        let new_refresh = json_str(&val, "refresh_token").to_owned();
        let expires_in = json_u64(&val, "expires_in").max(300) as i64;
        {
            let mut ts = self.tokens.lock().unwrap();
            ts.spotify_access_token = new_access.clone();
            if !new_refresh.is_empty() {
                ts.spotify_refresh_token = new_refresh;
            }
            ts.spotify_expires_at_ms = now_ms() as i64 + expires_in * 1000;
            ts.save();
        }
        Ok(new_access)
    }

    fn api_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<net::HttpReply, String> {
        let url = format!("{API}{path}");
        for attempt in 0..2 {
            let tok = self.refresh_if_needed()?;
            let auth = format!("Bearer {tok}");
            let mut hdrs = vec![("Authorization", auth.as_str())];
            if body.is_some() {
                hdrs.push(("Content-Type", "application/json"));
            }
            let reply = net::send(method, &url, &hdrs, body, 8 << 20)?;
            if reply.status == 401 && attempt == 0 {
                let mut ts = self.tokens.lock().unwrap();
                if !ts.spotify_refresh_token.is_empty() {
                    ts.spotify_access_token.clear();
                    ts.spotify_expires_at_ms = 0;
                    continue;
                }
            }
            return Ok(reply);
        }
        Err(i18n::get("err.sp_relogin"))
    }

    fn api_get_json(&self, path: &str) -> Result<Value, String> {
        let reply = self.api_request("GET", path, None)?;
        if !(200..300).contains(&reply.status) {
            return Err(spotify_http_error(&reply, path));
        }
        serde_json::from_slice(&reply.body).map_err(|e| format!("Spotify JSON error: {e}"))
    }

    fn choose_device_id(&self) -> Result<(String, String), String> {
        let val = self.api_get_json("/me/player/devices")?;
        let devs = val
            .get("devices")
            .and_then(Value::as_array)
            .ok_or_else(|| "No Spotify devices available".to_owned())?;
        let host = hostname_hint();
        let mut active: Option<(String, String)> = None;
        let mut host_match: Option<(String, String)> = None;
        let mut computer: Option<(String, String)> = None;
        let mut any: Option<(String, String)> = None;

        for d in devs {
            if d.get("is_restricted").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let id = json_id(d, "id");
            let name = json_str(d, "name").to_owned();
            if id.is_empty() {
                continue;
            }
            if any.is_none() {
                any = Some((id.clone(), name.clone()));
            }
            if d.get("is_active").and_then(Value::as_bool) == Some(true) {
                active = Some((id.clone(), name.clone()));
            }
            if !host.is_empty() && name.to_ascii_lowercase().contains(&host) {
                host_match = Some((id.clone(), name.clone()));
            }
            let dtype = json_str(d, "type");
            if dtype.eq_ignore_ascii_case("Computer") && !name.to_ascii_lowercase().contains("web")
            {
                computer = Some((id, name));
            }
        }
        active
            .or(host_match)
            .or(computer)
            .or(any)
            .ok_or_else(|| "Open Spotify on a device first".to_owned())
    }

    pub fn play_remote(&self, track: &Track, position_ms: u64) -> Result<(), String> {
        let (dev_id, dev_name) = self.choose_device_id()?;
        {
            let mut r = self.remote.lock().unwrap();
            r.device_id = dev_id.clone();
            r.device_name = dev_name;
            r.no_device = false;
            r.expected_id = track.id.clone();
            r.expected_pos_ms = Some(position_ms);
            r.expected_playing = Some(true);
            r.last_cmd_at_ms = now_ms();
        }
        let uri = format!("spotify:track:{}", track.id);
        let payload = json!({
            "uris": [uri],
            "position_ms": position_ms,
        });
        let body = serde_json::to_vec(&payload).unwrap();
        let path = format!("/me/player/play?device_id={}", net::url_encode(&dev_id));
        let mut reply = self.api_request("PUT", &path, Some(&body))?;
        if reply.status >= 500 || reply.status == 404 {
            let xfer =
                serde_json::to_vec(&json!({ "device_ids": [dev_id], "play": true })).unwrap();
            let _ = self.api_request("PUT", "/me/player", Some(&xfer));
            std::thread::sleep(Duration::from_millis(250));
            reply = self.api_request("PUT", &path, Some(&body))?;
        }
        if !(200..300).contains(&reply.status) && reply.status != 204 {
            return Err(spotify_http_error(&reply, &path));
        }
        Ok(())
    }

    pub fn pause_remote(&self) -> Result<(), String> {
        {
            let mut r = self.remote.lock().unwrap();
            r.expected_playing = Some(false);
            r.last_cmd_at_ms = now_ms();
        }
        #[cfg(target_os = "linux")]
        if mpris_call("Pause") {
            return Ok(());
        }
        let reply = self.api_request("PUT", "/me/player/pause", None)?;
        if !(200..300).contains(&reply.status) && reply.status != 204 {
            return Err(spotify_http_error(&reply, "/me/player/pause"));
        }
        Ok(())
    }

    pub fn resume_remote(&self) -> Result<(), String> {
        {
            let mut r = self.remote.lock().unwrap();
            r.expected_playing = Some(true);
            r.last_cmd_at_ms = now_ms();
        }
        #[cfg(target_os = "linux")]
        if mpris_call("Play") {
            return Ok(());
        }
        let reply = self.api_request("PUT", "/me/player/play", None)?;
        if !(200..300).contains(&reply.status) && reply.status != 204 {
            return Err(spotify_http_error(&reply, "/me/player/play"));
        }
        Ok(())
    }

    pub fn seek_remote(&self, position_ms: u64) -> Result<(), String> {
        {
            let mut r = self.remote.lock().unwrap();
            r.expected_pos_ms = Some(position_ms);
            r.last_cmd_at_ms = now_ms();
        }
        let path = format!("/me/player/seek?position_ms={position_ms}");
        let reply = self.api_request("PUT", &path, None)?;
        if !(200..300).contains(&reply.status) && reply.status != 204 {
            return Err(spotify_http_error(&reply, &path));
        }
        Ok(())
    }

    pub fn set_volume_remote(&self, volume: f32) {
        let pct = spotify_percent(volume);
        let path = format!("/me/player/volume?volume_percent={pct}");
        let _ = self.api_request("PUT", &path, None);
    }

    pub fn poll_remote(&self) -> SpotifyRemoteState {
        let Ok(reply) = self.api_request("GET", "/me/player", None) else {
            return self.remote.lock().unwrap().clone();
        };
        let mut r = self.remote.lock().unwrap();
        let tnow = now_ms();
        if reply.status == 204 {
            r.no_device = true;
            r.playing = false;
            r.updated_at_ms = tnow;
            return r.clone();
        }
        if !(200..300).contains(&reply.status) {
            return r.clone();
        }
        if let Ok(val) = serde_json::from_slice::<Value>(&reply.body) {
            let playing = val
                .get("is_playing")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let progress = json_u64(&val, "progress_ms");
            let item = val.get("item");
            let id = item.map(|it| json_id(it, "id")).unwrap_or_default();
            let linked = item
                .and_then(|it| it.get("linked_from"))
                .map(|lf| json_id(lf, "id"))
                .unwrap_or_default();
            let dur = item.map(|it| json_u64(it, "duration_ms")).unwrap_or(0);
            if let Some(dev) = val.get("device") {
                let dn = json_str(dev, "name");
                let did = json_id(dev, "id");
                if !dn.is_empty() {
                    r.device_name = dn.to_owned();
                }
                if !did.is_empty() {
                    r.device_id = did;
                }
            }
            r.no_device = false;
            let settling = tnow.saturating_sub(r.last_cmd_at_ms) < SETTLE_MS;
            if settling && !r.expected_id.is_empty() && same_song(&r.expected_id, &id, &linked) {
                r.expected_id.clear();
            }
            if !settling || r.expected_id.is_empty() {
                r.active_id = id;
                r.linked_from_id = linked;
                r.playing = playing;
                r.position_ms = progress;
                r.duration_ms = dur;
            } else {
                r.playing = r.expected_playing.unwrap_or(playing);
                r.position_ms = r.expected_pos_ms.unwrap_or(progress);
            }
            r.updated_at_ms = tnow;
        }
        r.clone()
    }
}

impl MusicProvider for SpotifyProvider {
    fn platform(&self) -> Platform {
        Platform::Spotify
    }

    fn has_auth(&self) -> bool {
        let ts = self.tokens.lock().unwrap();
        !ts.spotify_access_token.trim().is_empty() || !ts.spotify_refresh_token.trim().is_empty()
    }

    fn search(&self, query: &str) -> Result<SearchResult, String> {
        let q = net::url_encode(query.trim());
        let val = self.api_get_json(&format!(
            "/search?q={q}&type=track,album,artist,playlist&limit=20"
        ))?;
        let mut out = SearchResult::default();
        if let Some(arr) = val
            .get("tracks")
            .and_then(|t| t.get("items"))
            .and_then(Value::as_array)
        {
            for item in arr {
                if let Some(tr) = parse_spotify_track(item) {
                    out.tracks.push(tr);
                }
            }
        }
        if let Some(arr) = val
            .get("artists")
            .and_then(|a| a.get("items"))
            .and_then(Value::as_array)
        {
            for a in arr.iter().take(6) {
                let id = json_id(a, "id");
                let name = json_str(a, "name").to_owned();
                if !id.is_empty() && !name.is_empty() {
                    out.entities.push(MusicEntity {
                        kind: EntityKind::Artist,
                        id,
                        title: name,
                        subtitle: i18n::get("kind.artist"),
                        cover_url: first_image_url(a.get("images")),
                    });
                }
            }
        }
        if let Some(arr) = val
            .get("albums")
            .and_then(|a| a.get("items"))
            .and_then(Value::as_array)
        {
            for al in arr.iter().take(8) {
                let id = json_id(al, "id");
                let title = json_str(al, "name").to_owned();
                if !id.is_empty() && !title.is_empty() {
                    out.entities.push(MusicEntity {
                        kind: EntityKind::Album,
                        id,
                        title,
                        subtitle: join_spotify_artists(al.get("artists")),
                        cover_url: first_image_url(al.get("images")),
                    });
                }
            }
        }
        if let Some(arr) = val
            .get("playlists")
            .and_then(|p| p.get("items"))
            .and_then(Value::as_array)
        {
            for pl in arr.iter().take(8) {
                if let Some(ent) = parse_spotify_playlist(pl) {
                    out.entities.push(ent);
                }
            }
        }
        Ok(out)
    }

    fn liked_tracks(&self) -> Result<Vec<Track>, String> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset < 250 {
            let val = self.api_get_json(&format!("/me/tracks?limit=50&offset={offset}"))?;
            let Some(arr) = val.get("items").and_then(Value::as_array) else {
                break;
            };
            if arr.is_empty() {
                break;
            }
            for item in arr {
                let obj = item.get("track").unwrap_or(item);
                if let Some(tr) = parse_spotify_track(obj) {
                    out.push(tr);
                }
            }
            offset += arr.len();
            if val.get("next").is_none_or(Value::is_null) {
                break;
            }
        }
        Ok(out)
    }

    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String> {
        let val = self.api_get_json("/me/playlists?limit=50")?;
        let mut out = Vec::new();
        if let Some(arr) = val.get("items").and_then(Value::as_array) {
            for pl in arr {
                if let Some(ent) = parse_spotify_playlist(pl) {
                    out.push(ent);
                }
            }
        }
        Ok(out)
    }

    fn open(&self, entity: &MusicEntity) -> Result<Page, String> {
        match entity.kind {
            EntityKind::Artist => {
                let top = self.api_get_json(&format!(
                    "/artists/{}/top-tracks?market=from_token",
                    net::url_encode(&entity.id)
                ))?;
                let mut tracks = Vec::new();
                if let Some(arr) = top.get("tracks").and_then(Value::as_array) {
                    for t in arr {
                        if let Some(tr) = parse_spotify_track(t) {
                            tracks.push(tr);
                        }
                    }
                }
                let mut entities = Vec::new();
                if let Ok(alb) = self.api_get_json(&format!(
                    "/artists/{}/albums?include_groups=album,single&limit=20",
                    net::url_encode(&entity.id)
                )) && let Some(arr) = alb.get("items").and_then(Value::as_array)
                {
                    for al in arr {
                        let id = json_id(al, "id");
                        let title = json_str(al, "name").to_owned();
                        if !id.is_empty() && !title.is_empty() {
                            entities.push(MusicEntity {
                                kind: EntityKind::Album,
                                id,
                                title,
                                subtitle: json_str(al, "release_date").to_owned(),
                                cover_url: first_image_url(al.get("images")),
                            });
                        }
                    }
                }
                Ok(Page {
                    source: entity.clone(),
                    tracks,
                    entities,
                })
            }
            EntityKind::Album => {
                let alb = self.api_get_json(&format!("/albums/{}", net::url_encode(&entity.id)))?;
                let cover = first_image_url(alb.get("images")).or_else(|| entity.cover_url.clone());
                let alb_title = json_str(&alb, "name").to_owned();
                let mut tracks = Vec::new();
                if let Some(arr) = alb
                    .get("tracks")
                    .and_then(|t| t.get("items"))
                    .and_then(Value::as_array)
                {
                    for item in arr {
                        let id = json_id(item, "id");
                        let title = json_str(item, "name").to_owned();
                        if !id.is_empty() && !title.is_empty() {
                            tracks.push(Track {
                                id,
                                title,
                                artist: join_spotify_artists(item.get("artists")),
                                album: alb_title.clone(),
                                duration_ms: json_u64(item, "duration_ms"),
                                cover_url: cover.clone(),
                                provider: Platform::Spotify.id().to_owned(),
                            });
                        }
                    }
                }
                Ok(Page {
                    source: entity.clone(),
                    tracks,
                    entities: Vec::new(),
                })
            }
            EntityKind::Playlist => {
                if entity.is_mine() {
                    return Ok(Page {
                        source: entity.clone(),
                        tracks: self.liked_tracks()?,
                        entities: Vec::new(),
                    });
                }
                let pid = net::url_encode(&entity.id);
                let val = self
                    .api_get_json(&format!("/playlists/{pid}/items?limit=100"))
                    .or_else(|_| {
                        self.api_get_json(&format!("/playlists/{pid}/tracks?limit=100"))
                    })?;
                let mut tracks = Vec::new();
                if let Some(arr) = val.get("items").and_then(Value::as_array) {
                    for item in arr {
                        let obj = item
                            .get("track")
                            .or_else(|| item.get("item"))
                            .unwrap_or(item);
                        if let Some(tr) = parse_spotify_track(obj) {
                            tracks.push(tr);
                        }
                    }
                }
                Ok(Page {
                    source: entity.clone(),
                    tracks,
                    entities: Vec::new(),
                })
            }
            EntityKind::Station => Ok(Page {
                source: entity.clone(),
                tracks: Vec::new(),
                entities: Vec::new(),
            }),
        }
    }

    fn fetch_lyrics(&self, _track: &Track) -> PlatformLyrics {
        PlatformLyrics::default()
    }

    fn set_liked(&self, track: &Track, liked: bool) -> Result<(), String> {
        let method = if liked { "PUT" } else { "DELETE" };
        let uri = format!("spotify:track:{}", track.id);
        let r1 = self.api_request(
            method,
            &format!("/me/library?uris={}", net::url_encode(&uri)),
            None,
        )?;
        if (200..300).contains(&r1.status) || r1.status == 204 {
            return Ok(());
        }
        if r1.status == 404 || r1.status == 410 {
            let r2 = self.api_request(
                method,
                &format!("/me/tracks?ids={}", net::url_encode(&track.id)),
                None,
            )?;
            if (200..300).contains(&r2.status) || r2.status == 204 {
                return Ok(());
            }
            return Err(format!("Spotify library HTTP {}", r2.status));
        }
        Err(format!("Spotify library HTTP {}", r1.status))
    }

    fn load_audio(&self, _track: &Track) -> Result<PlaybackSource, String> {
        Ok(PlaybackSource::External)
    }
}

pub fn spotify_percent(volume: f32) -> u32 {
    let clamped = volume.clamp(0.0, 1.0);
    (clamped.cbrt() * 100.0).round() as u32
}

pub fn same_song(expected: &str, actual: &str, linked_from: &str) -> bool {
    !expected.is_empty()
        && (expected == actual || (!linked_from.is_empty() && expected == linked_from))
}

fn pkce_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

fn random_base64url(num_bytes: usize) -> String {
    let mut seed = now_ms() ^ (std::process::id() as u64).wrapping_mul(0x9E3779B97F4A7C15);
    let mut bytes = Vec::with_capacity(num_bytes);
    for _ in 0..num_bytes {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        bytes.push((seed >> 24) as u8);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn hostname_hint() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn spotify_http_error(reply: &net::HttpReply, path: &str) -> String {
    if let Ok(val) = serde_json::from_slice::<Value>(&reply.body)
        && let Some(err) = val.get("error")
    {
        let reason = json_str(err, "reason");
        if reason == "PREMIUM_REQUIRED" {
            return i18n::get("err.sp_premium");
        }
        if reason == "NO_ACTIVE_DEVICE" || (reply.status == 404 && path.starts_with("/me/player")) {
            return i18n::get("err.sp_no_device");
        }
        let msg = json_str(err, "message");
        if !msg.is_empty() {
            return format!("Spotify: {msg}");
        }
    }
    if reply.status == 404 && path.starts_with("/me/player") {
        return i18n::get("err.sp_no_device");
    }
    if reply.status == 403 {
        return i18n::get("err.sp_premium");
    }
    format!("Spotify HTTP {}", reply.status)
}

fn callback_html(ok: bool, msg: &str) -> String {
    let color = if ok { "#1DB954" } else { "#EF4444" };
    format!(
        "<!DOCTYPE html><html><body style=\"background:#111;color:#fff;font-family:sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0\"><div style=\"text-align:center;border:1px solid {color};padding:28px 36px;border-radius:12px\"><h2 style=\"color:{color}\">Overtone: Lyrics PRO MAX</h2><p>{msg}</p></div></body></html>"
    )
}

fn join_spotify_artists(v: Option<&Value>) -> String {
    let Some(arr) = v.and_then(Value::as_array) else {
        return String::new();
    };
    arr.iter()
        .filter_map(|a| a.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn first_image_url(v: Option<&Value>) -> Option<String> {
    let arr = v?.as_array()?;
    let item = arr.get(1).or_else(|| arr.first())?;
    let url = item.get("url").and_then(Value::as_str)?.trim();
    if url.starts_with("https://") {
        Some(url.to_owned())
    } else {
        None
    }
}

fn parse_spotify_track(t: &Value) -> Option<Track> {
    let id = json_id(t, "id");
    let title = json_str(t, "name").trim().to_owned();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let album = t
        .get("album")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let cover = first_image_url(t.get("album").and_then(|a| a.get("images")));
    Some(Track {
        id,
        title,
        artist: join_spotify_artists(t.get("artists")),
        album,
        duration_ms: json_u64(t, "duration_ms"),
        cover_url: cover,
        provider: Platform::Spotify.id().to_owned(),
    })
}

fn parse_spotify_playlist(pl: &Value) -> Option<MusicEntity> {
    let id = json_id(pl, "id");
    let title = json_str(pl, "name").trim().to_owned();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let sub = pl
        .get("owner")
        .and_then(|o| o.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(MusicEntity {
        kind: EntityKind::Playlist,
        id,
        title,
        subtitle: sub,
        cover_url: first_image_url(pl.get("images")),
    })
}

#[cfg(target_os = "linux")]
fn mpris_call(method: &str) -> bool {
    std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.mpris.MediaPlayer2.spotify",
            "--object-path",
            "/org/mpris/MediaPlayer2",
            "--method",
            &format!("org.mpris.MediaPlayer2.Player.{method}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_curve_and_pkce_challenge() {
        assert_eq!(spotify_percent(0.0), 0);
        assert_eq!(spotify_percent(1.0), 100);
        // 0.125^(1/3) = 0.5 -> 50%
        assert_eq!(spotify_percent(0.125), 50);

        let ch = pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(ch, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

        assert!(same_song("abc", "def", "abc"));
        assert!(!same_song("abc", "def", "ghi"));

        assert!(SpotifyProvider::is_valid_client_id(
            "3bc21d91572d488a9e9b337fb7fa7415"
        ));
        assert!(!SpotifyProvider::is_valid_client_id(
            "3bc21d91572d488a9e9b337fb7fa7415v"
        ));
        assert!(!SpotifyProvider::is_valid_client_id(""));

        let sp = SpotifyProvider::new(Arc::new(Mutex::new(TokenStore::default())));
        *sp.auth_state.lock().unwrap() = SpotifyAuthState::WaitingForBrowser;
        sp.cancel_login();
        assert_eq!(sp.auth_state(), SpotifyAuthState::Idle);
    }
}
