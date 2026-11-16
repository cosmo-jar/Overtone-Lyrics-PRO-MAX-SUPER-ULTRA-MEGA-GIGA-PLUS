//! Yandex Music provider, ported 1:1 from `YandexMusicProvider.java`.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::audio::AudioFormat;
use crate::config::TokenStore;
use crate::i18n;
use crate::local::AudioCache;
use crate::lyrics::parsers::{parse_lrc, parse_plain_lines};
use crate::model::{EntityKind, MusicEntity, Page, Platform, Track};
use crate::net::{self, MAX_DOWNLOAD_BYTES};
use crate::providers::{
    MusicProvider, PlatformLyrics, PlaybackSource, SearchResult, WaveStart, json_id, json_str,
    json_u64, yandex_cover_url,
};

type HmacSha256 = Hmac<Sha256>;

const API: &str = "https://api.music.yandex.net";
const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
const CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
const DOWNLOAD_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";
const LYRICS_SIGN_KEY: &[u8] = b"p93jhgh689SBReK6ghtw62";
const USER_AGENT: &str = "Yandex-Music-API";

const RANGE_CHUNK: usize = 512 * 1024;
const PREFETCH_THREADS: usize = 2;

#[derive(Clone, Debug)]
pub struct DeviceAuthSession {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval_sec: u64,
    pub expires_in_sec: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevicePollResult {
    Pending,
    Authorized { token: String, label: String },
    SlowDown,
    Failed(String),
}

struct PrefetchState {
    queue: VecDeque<String>,
    in_flight: HashSet<String>,
    stop: bool,
}

#[derive(Clone)]
pub struct YandexMusicProvider {
    tokens: Arc<Mutex<TokenStore>>,
    cache: AudioCache,
    uid_cache: Arc<Mutex<Option<(String, String)>>>,
    prefetch_enabled: Arc<Mutex<bool>>,
    prefetch_state: Arc<(Mutex<PrefetchState>, Condvar)>,
}

impl YandexMusicProvider {
    pub fn new(tokens: Arc<Mutex<TokenStore>>, cache: AudioCache) -> Self {
        let prefetch_state = Arc::new((
            Mutex::new(PrefetchState {
                queue: VecDeque::new(),
                in_flight: HashSet::new(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let p = Self {
            tokens,
            cache,
            uid_cache: Arc::new(Mutex::new(None)),
            prefetch_enabled: Arc::new(Mutex::new(true)),
            prefetch_state,
        };
        p.spawn_prefetch_workers();
        p
    }

    pub fn set_prefetch_enabled(&self, enabled: bool) {
        *self.prefetch_enabled.lock().unwrap() = enabled;
    }

    pub fn start_device_auth(&self) -> Result<DeviceAuthSession, String> {
        let reply = net::post_form(
            "https://oauth.yandex.ru/device/code",
            &[("User-Agent", USER_AGENT)],
            &[("client_id", CLIENT_ID)],
        )?;
        let val: Value = serde_json::from_slice(&reply.body)
            .map_err(|e| format!("Invalid device/code JSON: {e}"))?;
        let device_code = json_str(&val, "device_code").to_owned();
        let user_code = json_str(&val, "user_code").to_owned();
        let verification_url = json_str(&val, "verification_url").to_owned();
        if device_code.is_empty() || user_code.is_empty() {
            let err = json_str(&val, "error_description");
            return Err(if err.is_empty() {
                "Yandex device authorization unavailable".to_owned()
            } else {
                err.to_owned()
            });
        }
        Ok(DeviceAuthSession {
            device_code,
            user_code,
            verification_url: if verification_url.is_empty() {
                "https://ya.ru/device".to_owned()
            } else {
                verification_url
            },
            interval_sec: json_u64(&val, "interval").max(3),
            expires_in_sec: json_u64(&val, "expires_in").max(60),
        })
    }

    pub fn poll_device_auth(&self, device_code: &str) -> DevicePollResult {
        let Ok(reply) = net::post_form(
            "https://oauth.yandex.ru/token",
            &[("User-Agent", USER_AGENT)],
            &[
                ("grant_type", "device_code"),
                ("code", device_code),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
            ],
        ) else {
            return DevicePollResult::Pending;
        };
        let Ok(val) = serde_json::from_slice::<Value>(&reply.body) else {
            return DevicePollResult::Pending;
        };
        if let Some(tok) = val.get("access_token").and_then(Value::as_str)
            && !tok.trim().is_empty()
        {
            let token = tok.trim().to_owned();
            let label = self
                .verify_and_save_token(&token)
                .unwrap_or_else(|_| "Yandex".to_owned());
            return DevicePollResult::Authorized { token, label };
        }
        match json_str(&val, "error") {
            "authorization_pending" => DevicePollResult::Pending,
            "slow_down" => DevicePollResult::SlowDown,
            "" => DevicePollResult::Failed(format!("HTTP {}", reply.status)),
            other => {
                let desc = json_str(&val, "error_description");
                DevicePollResult::Failed(if desc.is_empty() {
                    other.to_owned()
                } else {
                    desc.to_owned()
                })
            }
        }
    }

    pub fn verify_and_save_token(&self, token: &str) -> Result<String, String> {
        let tok = token
            .trim()
            .strip_prefix("OAuth ")
            .unwrap_or(token.trim())
            .to_owned();
        if tok.is_empty() {
            return Err("Empty Yandex token".to_owned());
        }
        let status = self.get_json_with_token("/account/status", &tok)?;
        let acct = status
            .get("result")
            .and_then(|r| r.get("account"))
            .ok_or_else(|| "Invalid Yandex account status".to_owned())?;
        let uid = json_id(acct, "uid");
        if uid.is_empty() {
            return Err("Missing Yandex account UID".to_owned());
        }
        let display = json_str(acct, "displayName");
        let login = json_str(acct, "login");
        let label = if !display.is_empty() {
            display.to_owned()
        } else if !login.is_empty() {
            login.to_owned()
        } else {
            uid.clone()
        };
        *self.uid_cache.lock().unwrap() = Some((tok.clone(), uid));
        self.tokens
            .lock()
            .unwrap()
            .add_yandex_account(label.clone(), tok);
        Ok(label)
    }

    fn token(&self) -> String {
        self.tokens.lock().unwrap().yandex_token.trim().to_owned()
    }

    fn uid(&self) -> Result<String, String> {
        let tok = self.token();
        if tok.is_empty() {
            return Err("Not signed in to Yandex Music".to_owned());
        }
        {
            let guard = self.uid_cache.lock().unwrap();
            if let Some((cached_tok, cached_uid)) = guard.as_ref()
                && cached_tok == &tok
                && !cached_uid.is_empty()
            {
                return Ok(cached_uid.clone());
            }
        }
        let val = self.get_json("/account/status")?;
        let uid = val
            .get("result")
            .and_then(|r| r.get("account"))
            .map(|a| json_id(a, "uid"))
            .unwrap_or_default();
        if uid.is_empty() {
            return Err("Failed to get Yandex UID".to_owned());
        }
        *self.uid_cache.lock().unwrap() = Some((tok, uid.clone()));
        Ok(uid)
    }

    fn auth_headers(tok: &str) -> Vec<(&'static str, String)> {
        let mut h = vec![
            ("User-Agent", USER_AGENT.to_owned()),
            ("Accept", "application/json".to_owned()),
            (
                "X-Yandex-Music-Client",
                "YandexMusicAndroid/24023621".to_owned(),
            ),
        ];
        if !tok.is_empty() {
            h.push(("Authorization", format!("OAuth {tok}")));
        }
        h
    }

    fn get_json(&self, path: &str) -> Result<Value, String> {
        self.get_json_with_token(path, &self.token())
    }

    fn get_json_with_token(&self, path: &str, tok: &str) -> Result<Value, String> {
        let url = format!("{API}{path}");
        let hdr_owned = Self::auth_headers(tok);
        let hdrs: Vec<(&str, &str)> = hdr_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        net::get_json(&url, &hdrs)
    }

    fn post_form_api(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{API}{path}");
        let tok = self.token();
        let hdr_owned = Self::auth_headers(&tok);
        let hdrs: Vec<(&str, &str)> = hdr_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let reply = net::post_form(&url, &hdrs, form)?;
        if !(200..300).contains(&reply.status) {
            return Err(format!("Yandex HTTP {}", reply.status));
        }
        serde_json::from_slice(&reply.body).map_err(|e| format!("Yandex JSON error: {e}"))
    }

    fn post_json_api(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let url = format!("{API}{path}");
        let tok = self.token();
        let hdr_owned = Self::auth_headers(&tok);
        let hdrs: Vec<(&str, &str)> = hdr_owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        net::post_json(&url, &hdrs, payload)
    }

    pub fn wave_stations(&self) -> Result<Vec<MusicEntity>, String> {
        let lang = if i18n::current_language().code().starts_with("ru") {
            "ru"
        } else {
            "en"
        };
        let val = self.get_json(&format!("/rotor/stations/list?language={lang}"))?;
        let mut out = Vec::new();
        if let Some(arr) = val.get("result").and_then(Value::as_array) {
            for item in arr {
                let Some(st) = item.get("station") else {
                    continue;
                };
                let Some(id_obj) = st.get("id") else { continue };
                let st_type = json_str(id_obj, "type");
                let st_tag = json_str(id_obj, "tag");
                if st_type.is_empty() || st_tag.is_empty() {
                    continue;
                }
                let id = format!("{st_type}:{st_tag}");
                let name = json_str(st, "name").to_owned();
                let sub = item
                    .get("rupTitle")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let cover = st
                    .get("icon")
                    .and_then(|ic| ic.get("imageUrl"))
                    .and_then(Value::as_str)
                    .and_then(|u| yandex_cover_url(u, "200x200"));
                out.push(MusicEntity {
                    kind: EntityKind::Station,
                    id,
                    title: if name.is_empty() {
                        "Wave".to_owned()
                    } else {
                        name
                    },
                    subtitle: sub,
                    cover_url: cover,
                });
            }
        }
        Ok(out)
    }

    pub fn wave_start(&self, seed: &str) -> Result<WaveStart, String> {
        let seed_use = if seed.trim().is_empty() {
            "user:onyourwave"
        } else {
            seed.trim()
        };
        let val = self.post_json_api(
            "/rotor/session/new",
            &json!({ "seeds": [seed_use], "includeTracksInResponse": true }),
        )?;
        let Some(res) = val.get("result") else {
            return Err("Empty Yandex Wave session".to_owned());
        };
        let session_id = json_id(res, "radioSessionId");
        let batch_id = json_id(res, "batchId");
        let tracks = parse_rotor_sequence(res.get("sequence"));
        let token = if batch_id.is_empty() {
            session_id
        } else {
            format!("{session_id}|{batch_id}")
        };
        Ok(WaveStart {
            seed: token,
            tracks,
        })
    }

    pub fn wave_more(
        &self,
        wave_token: &str,
        queue_ids: &[String],
    ) -> Result<(String, Vec<Track>), String> {
        let (session_id, _) = wave_token.split_once('|').unwrap_or((wave_token, ""));
        if session_id.is_empty() {
            return Ok((wave_token.to_owned(), Vec::new()));
        }
        let body = json!({
            "queue": queue_ids.iter().take(20).collect::<Vec<_>>(),
        });
        let path = format!("/rotor/session/{}/tracks", net::url_encode(session_id));
        let val = self.post_json_api(&path, &body)?;
        let Some(res) = val.get("result") else {
            return Ok((wave_token.to_owned(), Vec::new()));
        };
        let batch_id = json_id(res, "batchId");
        let next_tok = if batch_id.is_empty() {
            session_id.to_owned()
        } else {
            format!("{session_id}|{batch_id}")
        };
        let tracks = parse_rotor_sequence(res.get("sequence"));
        Ok((next_tok, tracks))
    }

    pub fn wave_feedback(
        &self,
        wave_token: &str,
        event: &str,
        track_id: Option<&str>,
        total_played_s: f64,
    ) {
        let (session_id, batch_id) = wave_token.split_once('|').unwrap_or((wave_token, ""));
        if session_id.is_empty() {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut ev = json!({
            "type": event,
            "timestamp": format!("{ts}"),
            "totalPlayedSeconds": total_played_s.max(0.0),
        });
        if let Some(tid) = track_id
            && !tid.is_empty()
        {
            ev["trackId"] = json!(tid);
        }
        let mut payload = json!({ "event": ev });
        if !batch_id.is_empty() {
            payload["batchId"] = json!(batch_id);
        }
        let path = format!("/rotor/session/{}/feedback", net::url_encode(session_id));
        let _ = self.post_json_api(&path, &payload);
    }

    pub fn dislike(&self, track_id: &str) -> Result<(), String> {
        let uid = self.uid()?;
        let path = format!("/users/{uid}/dislikes/tracks/add-multiple");
        let _ = self.post_form_api(&path, &[("track-ids", track_id)])?;
        Ok(())
    }

    fn sign_download_url(info_xml: &str) -> Result<String, String> {
        let host = xml_tag(info_xml, "host").ok_or("Missing host in download XML")?;
        let path = xml_tag(info_xml, "path").ok_or("Missing path in download XML")?;
        let ts = xml_tag(info_xml, "ts").ok_or("Missing ts in download XML")?;
        let s = xml_tag(info_xml, "s").ok_or("Missing s in download XML")?;
        let path_no_slash = path.strip_prefix('/').unwrap_or(&path);
        let to_hash = format!("{DOWNLOAD_SALT}{path_no_slash}{s}");
        let sign = net::md5_hex(to_hash.as_bytes());
        Ok(format!("https://{host}/get-mp3/{sign}/{ts}{path}"))
    }

    pub fn sign_lyrics_query(track_id: &str, ts_sec: u64) -> String {
        let mut mac =
            HmacSha256::new_from_slice(LYRICS_SIGN_KEY).expect("HMAC accepts any key size");
        mac.update(format!("{track_id}{ts_sec}").as_bytes());
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }

    fn stream_url(&self, track_id: &str) -> Result<String, String> {
        let info = self.get_json(&format!(
            "/tracks/{}/download-info",
            net::url_encode(track_id)
        ))?;
        let items = info
            .get("result")
            .and_then(Value::as_array)
            .ok_or_else(|| "No download-info items".to_owned())?;
        let best = items
            .iter()
            .filter(|i| json_str(i, "codec").eq_ignore_ascii_case("mp3"))
            .max_by_key(|i| json_u64(i, "bitrateInKbps"))
            .or_else(|| items.first())
            .ok_or_else(|| "No playable stream found".to_owned())?;
        let dl_info_url = json_str(best, "downloadInfoUrl");
        if dl_info_url.is_empty() {
            return Err("Missing downloadInfoUrl".to_owned());
        }
        let xml = net::get_text(dl_info_url, &[("User-Agent", USER_AGENT)])?;
        Self::sign_download_url(&xml)
    }

    fn download_bytes(&self, track_id: &str) -> Result<Vec<u8>, String> {
        if let Some(cached) = self.cache.get(track_id) {
            return Ok(cached);
        }
        let url = self.stream_url(track_id)?;
        let data = download_resumable(&url)?;
        if data.len() < 1024 {
            return Err("Downloaded Yandex audio unexpectedly small".to_owned());
        }
        self.cache.put(track_id, data.clone());
        Ok(data)
    }

    fn spawn_prefetch_workers(&self) {
        for idx in 0..PREFETCH_THREADS {
            let me = self.clone();
            std::thread::Builder::new()
                .name(format!("overtone-yandex-prefetch-{idx}"))
                .spawn(move || {
                    loop {
                        let next_id = {
                            let (lock, cvar) = &*me.prefetch_state;
                            let mut st = lock.lock().unwrap();
                            while !st.stop && st.queue.is_empty() {
                                st = cvar.wait(st).unwrap();
                            }
                            if st.stop {
                                return;
                            }
                            let id = st.queue.pop_front();
                            if let Some(ref tid) = id {
                                st.in_flight.insert(tid.clone());
                            }
                            id
                        };
                        let Some(tid) = next_id else { continue };
                        let _ = me.download_bytes(&tid);
                        let (lock, _) = &*me.prefetch_state;
                        lock.lock().unwrap().in_flight.remove(&tid);
                    }
                })
                .ok();
        }
    }

    fn fetch_tracks(&self, ids: &[String]) -> Result<Vec<Track>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_id = std::collections::HashMap::new();
        for chunk in ids.chunks(200) {
            let joined = chunk.join(",");
            let res = self.post_form_api("/tracks", &[("track-ids", &joined)])?;
            if let Some(arr) = res.get("result").and_then(Value::as_array) {
                for t in arr {
                    if let Some(tr) = parse_track(t) {
                        by_id.entry(tr.id.clone()).or_insert(tr);
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(ids.len());
        for raw in ids {
            let bare = raw.split_once(':').map_or(raw.as_str(), |(id, _)| id);
            if let Some(tr) = by_id.remove(bare) {
                out.push(tr);
            }
        }
        Ok(out)
    }

    fn resolve_entity_from_track(&self, entity: &MusicEntity) -> Result<MusicEntity, String> {
        let Some(raw_track_id) = entity.id.strip_prefix("track:") else {
            return Ok(entity.clone());
        };
        let bare_id = raw_track_id
            .split_once(':')
            .map_or(raw_track_id, |(id, _)| id);

        if let Ok(res) = self.post_form_api("/tracks", &[("track-ids", bare_id)])
            && let Some(tr) = res
                .get("result")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
        {
            match entity.kind {
                EntityKind::Artist => {
                    if let Some(ar) = tr
                        .get("artists")
                        .and_then(Value::as_array)
                        .and_then(|arr| arr.first())
                    {
                        let id = json_id(ar, "id");
                        let name = json_str(ar, "name").to_owned();
                        if !id.is_empty() {
                            let cover = ar
                                .get("cover")
                                .and_then(|c| c.get("uri"))
                                .and_then(Value::as_str)
                                .and_then(|u| yandex_cover_url(u, "200x200"))
                                .or_else(|| entity.cover_url.clone());
                            return Ok(MusicEntity {
                                kind: EntityKind::Artist,
                                id,
                                title: if name.is_empty() {
                                    entity.title.clone()
                                } else {
                                    name
                                },
                                subtitle: i18n::get("kind.artist"),
                                cover_url: cover,
                            });
                        }
                    }
                }
                EntityKind::Album => {
                    if let Some(al) = tr
                        .get("albums")
                        .and_then(Value::as_array)
                        .and_then(|arr| arr.first())
                    {
                        let id = json_id(al, "id");
                        let title = json_str(al, "title").to_owned();
                        if !id.is_empty() {
                            let cover = yandex_cover_url(json_str(al, "coverUri"), "200x200")
                                .or_else(|| entity.cover_url.clone());
                            let artist = join_artists(al.get("artists"));
                            return Ok(MusicEntity {
                                kind: EntityKind::Album,
                                id,
                                title: if title.is_empty() {
                                    entity.title.clone()
                                } else {
                                    title
                                },
                                subtitle: if artist.is_empty() {
                                    entity.subtitle.clone()
                                } else {
                                    artist
                                },
                                cover_url: cover,
                            });
                        }
                    }
                }
                EntityKind::Playlist | EntityKind::Station => {}
            }
        }

        let search_query = match entity.kind {
            EntityKind::Artist => entity
                .title
                .split(',')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(entity.title.as_str())
                .to_owned(),
            EntityKind::Album => {
                let first_artist = entity
                    .subtitle
                    .split(',')
                    .next()
                    .map(str::trim)
                    .unwrap_or("");
                if first_artist.is_empty() {
                    entity.title.clone()
                } else {
                    format!("{first_artist} {}", entity.title)
                }
            }
            EntityKind::Playlist | EntityKind::Station => {
                return Err("Unsupported entity kind".to_owned());
            }
        };

        if !search_query.trim().is_empty()
            && let Ok(found) = self.search(&search_query)
            && let Some(matched) = found.entities.into_iter().find(|e| e.kind == entity.kind)
        {
            return Ok(matched);
        }

        Err(match entity.kind {
            EntityKind::Artist => "Artist not found for track".to_owned(),
            EntityKind::Album => "Album not found for track".to_owned(),
            EntityKind::Playlist | EntityKind::Station => "Playlist not found".to_owned(),
        })
    }
}

impl Drop for YandexMusicProvider {
    fn drop(&mut self) {
        if Arc::strong_count(&self.prefetch_state) == 1 {
            let (lock, cvar) = &*self.prefetch_state;
            lock.lock().unwrap().stop = true;
            cvar.notify_all();
        }
    }
}

impl MusicProvider for YandexMusicProvider {
    fn platform(&self) -> Platform {
        Platform::Yandex
    }

    fn has_auth(&self) -> bool {
        !self.token().is_empty()
    }

    fn search(&self, query: &str) -> Result<SearchResult, String> {
        let q = net::url_encode(query.trim());
        let val = self.get_json(&format!("/search?text={q}&page=0&type=all&nocorrect=false"))?;
        let Some(res) = val.get("result") else {
            return Ok(SearchResult::default());
        };
        let mut out = SearchResult::default();
        if let Some(arr) = res
            .get("tracks")
            .and_then(|t| t.get("results"))
            .and_then(Value::as_array)
        {
            for t in arr {
                if let Some(tr) = parse_track(t) {
                    out.tracks.push(tr);
                }
            }
        }
        if let Some(arr) = res
            .get("artists")
            .and_then(|a| a.get("results"))
            .and_then(Value::as_array)
        {
            for a in arr.iter().take(6) {
                let id = json_id(a, "id");
                let name = json_str(a, "name").to_owned();
                if id.is_empty() || name.is_empty() {
                    continue;
                }
                let cover = a
                    .get("cover")
                    .and_then(|c| c.get("uri"))
                    .and_then(Value::as_str)
                    .and_then(|u| yandex_cover_url(u, "200x200"));
                out.entities.push(MusicEntity {
                    kind: EntityKind::Artist,
                    id,
                    title: name,
                    subtitle: i18n::get("kind.artist"),
                    cover_url: cover,
                });
            }
        }
        if let Some(arr) = res
            .get("albums")
            .and_then(|a| a.get("results"))
            .and_then(Value::as_array)
        {
            for al in arr.iter().take(8) {
                let id = json_id(al, "id");
                let title = json_str(al, "title").to_owned();
                if id.is_empty() || title.is_empty() {
                    continue;
                }
                let artist = join_artists(al.get("artists"));
                let cover = json_str(al, "coverUri");
                out.entities.push(MusicEntity {
                    kind: EntityKind::Album,
                    id,
                    title,
                    subtitle: artist,
                    cover_url: yandex_cover_url(cover, "200x200"),
                });
            }
        }
        out.entities.push(MusicEntity::mine());
        if let Some(arr) = res
            .get("playlists")
            .and_then(|p| p.get("results"))
            .and_then(Value::as_array)
        {
            for pl in arr.iter().take(8) {
                let kind = json_id(pl, "kind");
                let owner_uid = pl
                    .get("owner")
                    .map(|o| json_id(o, "uid"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| json_id(pl, "uid"));
                let title = json_str(pl, "title").to_owned();
                if kind.is_empty() || owner_uid.is_empty() || title.is_empty() {
                    continue;
                }
                let cover = pl
                    .get("cover")
                    .and_then(|c| c.get("uri"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        pl.get("cover")
                            .and_then(|c| c.get("itemsUri"))
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                            .and_then(Value::as_str)
                    })
                    .and_then(|u| yandex_cover_url(u, "200x200"));
                let sub = pl
                    .get("owner")
                    .and_then(|o| o.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                out.entities.push(MusicEntity {
                    kind: EntityKind::Playlist,
                    id: format!("{owner_uid}:{kind}"),
                    title,
                    subtitle: sub,
                    cover_url: cover,
                });
            }
        }
        Ok(out)
    }

    fn liked_tracks(&self) -> Result<Vec<Track>, String> {
        let uid = self.uid()?;
        let val = self.get_json(&format!("/users/{uid}/likes/tracks"))?;
        let mut ids = Vec::new();
        if let Some(arr) = val
            .get("result")
            .and_then(|r| r.get("library"))
            .and_then(|l| l.get("tracks"))
            .and_then(Value::as_array)
        {
            for entry in arr {
                let id = json_id(entry, "id");
                if !id.is_empty() {
                    ids.push(id);
                }
            }
        }
        self.fetch_tracks(&ids)
    }

    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String> {
        let uid = self.uid()?;
        let val = self.get_json(&format!("/users/{uid}/playlists/list"))?;
        let mut out = Vec::new();
        if let Some(arr) = val.get("result").and_then(Value::as_array) {
            for pl in arr {
                let kind = json_id(pl, "kind");
                let owner_uid = pl
                    .get("owner")
                    .map(|o| json_id(o, "uid"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| uid.clone());
                let title = json_str(pl, "title").to_owned();
                if kind.is_empty() || title.is_empty() {
                    continue;
                }
                let cnt = json_u64(pl, "trackCount");
                let cover = pl
                    .get("cover")
                    .and_then(|c| c.get("uri"))
                    .and_then(Value::as_str)
                    .or_else(|| pl.get("ogImage").and_then(Value::as_str))
                    .and_then(|u| yandex_cover_url(u, "200x200"));
                out.push(MusicEntity {
                    kind: EntityKind::Playlist,
                    id: format!("{owner_uid}:{kind}"),
                    title,
                    subtitle: i18n::tracks(cnt as usize),
                    cover_url: cover,
                });
            }
        }
        Ok(out)
    }

    fn open(&self, entity: &MusicEntity) -> Result<Page, String> {
        match entity.kind {
            EntityKind::Artist => {
                let resolved = self.resolve_entity_from_track(entity)?;
                let val = self.get_json(&format!(
                    "/artists/{}/brief-info",
                    net::url_encode(&resolved.id)
                ))?;
                let res = val.get("result").ok_or("Empty artist info")?;
                let mut tracks = Vec::new();
                if let Some(arr) = res.get("popularTracks").and_then(Value::as_array) {
                    for t in arr {
                        if let Some(tr) = parse_track(t) {
                            tracks.push(tr);
                        }
                    }
                }
                if tracks.is_empty()
                    && let Ok(tval) = self.get_json(&format!(
                        "/artists/{}/tracks?page=0&page-size=50",
                        net::url_encode(&resolved.id)
                    ))
                    && let Some(arr) = tval
                        .get("result")
                        .and_then(|r| r.get("tracks"))
                        .and_then(Value::as_array)
                {
                    for t in arr {
                        if let Some(tr) = parse_track(t) {
                            tracks.push(tr);
                        }
                    }
                }
                let mut entities = Vec::new();
                let direct_albums = self
                    .get_json(&format!(
                        "/artists/{}/direct-albums?page=0&page-size=50&sort-by=year",
                        net::url_encode(&resolved.id)
                    ))
                    .ok()
                    .and_then(|v| {
                        v.get("result")
                            .and_then(|r| r.get("albums"))
                            .and_then(Value::as_array)
                            .cloned()
                    })
                    .filter(|a| !a.is_empty());
                let albums_src = direct_albums.as_deref().or_else(|| {
                    res.get("albums")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                });
                if let Some(arr) = albums_src {
                    for al in arr.iter().take(50) {
                        let id = json_id(al, "id");
                        let title = json_str(al, "title").to_owned();
                        if !id.is_empty() && !title.is_empty() {
                            entities.push(MusicEntity {
                                kind: EntityKind::Album,
                                id,
                                title,
                                subtitle: json_u64(al, "year").to_string(),
                                cover_url: yandex_cover_url(json_str(al, "coverUri"), "200x200"),
                            });
                        }
                    }
                }
                Ok(Page {
                    source: resolved,
                    tracks,
                    entities,
                })
            }
            EntityKind::Album => {
                let resolved = self.resolve_entity_from_track(entity)?;
                let val = self.get_json(&format!(
                    "/albums/{}/with-tracks",
                    net::url_encode(&resolved.id)
                ))?;
                let res = val.get("result").ok_or("Empty album info")?;
                let mut tracks = Vec::new();
                if let Some(vols) = res.get("volumes").and_then(Value::as_array) {
                    for vol in vols {
                        if let Some(arr) = vol.as_array() {
                            for t in arr {
                                if let Some(tr) = parse_track(t) {
                                    tracks.push(tr);
                                }
                            }
                        }
                    }
                }
                Ok(Page {
                    source: resolved,
                    tracks,
                    entities: Vec::new(),
                })
            }
            EntityKind::Playlist => {
                if entity.is_mine() {
                    return Ok(Page {
                        source: entity.clone(),
                        tracks: Vec::new(),
                        entities: self.my_playlists()?,
                    });
                }
                let (owner, kind) = entity
                    .id
                    .split_once(':')
                    .ok_or_else(|| "Invalid Yandex playlist id".to_owned())?;
                let val = self.get_json(&format!(
                    "/users/{}/playlists/{}?rich-tracks=true",
                    net::url_encode(owner),
                    net::url_encode(kind)
                ))?;
                let mut tracks = Vec::new();
                let mut bare = Vec::new();
                if let Some(arr) = val
                    .get("result")
                    .and_then(|r| r.get("tracks"))
                    .and_then(Value::as_array)
                {
                    for item in arr {
                        if let Some(tr_val) = item.get("track")
                            && tr_val.is_object()
                        {
                            if let Some(tr) = parse_track(tr_val) {
                                tracks.push(tr);
                            }
                        } else if let Some(tr) = parse_track(item) {
                            tracks.push(tr);
                        } else {
                            let tid = json_id(item, "id");
                            if !tid.is_empty() {
                                let aid = json_id(item, "albumId");
                                bare.push(if aid.is_empty() {
                                    tid
                                } else {
                                    format!("{tid}:{aid}")
                                });
                            }
                        }
                    }
                }
                if tracks.is_empty() && !bare.is_empty() {
                    tracks.extend(self.fetch_tracks(&bare)?);
                }
                Ok(Page {
                    source: entity.clone(),
                    tracks,
                    entities: Vec::new(),
                })
            }
            EntityKind::Station => {
                let start = self.wave_start(&entity.id)?;
                Ok(Page {
                    source: entity.clone(),
                    tracks: start.tracks,
                    entities: Vec::new(),
                })
            }
        }
    }

    fn fetch_lyrics(&self, track: &Track) -> PlatformLyrics {
        if track.id.trim().is_empty() {
            return PlatformLyrics::default();
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let sign = Self::sign_lyrics_query(&track.id, ts);
        for fmt in ["LRC", "TEXT"] {
            let path = format!(
                "/tracks/{}/lyrics?format={fmt}&timeStamp={ts}&sign={}",
                net::url_encode(&track.id),
                net::url_encode(&sign)
            );
            let Ok(val) = self.get_json(&path) else {
                continue;
            };
            let Some(dl_url) = val
                .get("result")
                .and_then(|r| r.get("downloadUrl"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Ok(text) = net::get_text(dl_url, &[("User-Agent", USER_AGENT)]) else {
                continue;
            };
            if fmt == "LRC" {
                let synced = parse_lrc(&text);
                if !synced.is_empty() {
                    return PlatformLyrics {
                        synced,
                        plain: Vec::new(),
                    };
                }
            }
            let plain = parse_plain_lines(&text);
            if !plain.is_empty() {
                return PlatformLyrics {
                    synced: Vec::new(),
                    plain,
                };
            }
        }
        PlatformLyrics::default()
    }

    fn set_liked(&self, track: &Track, liked: bool) -> Result<(), String> {
        let uid = self.uid()?;
        let op = if liked { "add-multiple" } else { "remove" };
        let path = format!("/users/{uid}/likes/tracks/{op}");
        let _ = self.post_form_api(&path, &[("track-ids", &track.id)])?;
        Ok(())
    }

    fn load_audio(&self, track: &Track) -> Result<PlaybackSource, String> {
        let data = self.download_bytes(&track.id)?;
        let format = AudioFormat::detect(&data).unwrap_or(AudioFormat::Mp3);
        Ok(PlaybackSource::Bytes { format, data })
    }

    fn on_queue_change(&self, upcoming_ids: &[String], current_id: Option<&str>) {
        let mut keep: HashSet<String> = upcoming_ids.iter().cloned().collect();
        if let Some(c) = current_id {
            keep.insert(c.to_owned());
        }
        self.cache.set_keep(keep, current_id.map(str::to_owned));
        if !*self.prefetch_enabled.lock().unwrap() {
            return;
        }
        let (lock, cvar) = &*self.prefetch_state;
        let mut st = lock.lock().unwrap();
        for id in upcoming_ids {
            if !self.cache.has(id) && !st.in_flight.contains(id) && !st.queue.contains(id) {
                st.queue.push_back(id.clone());
            }
        }
        if !st.queue.is_empty() {
            cvar.notify_all();
        }
    }
}

fn download_resumable(url: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut total_expected: Option<usize> = None;
    let mut offset = 0usize;
    let mut stalls = 0usize;

    loop {
        let end = offset + RANGE_CHUNK - 1;
        let range_val = format!("bytes={offset}-{end}");
        let reply = net::send(
            "GET",
            url,
            &[("User-Agent", USER_AGENT), ("Range", &range_val)],
            None,
            MAX_DOWNLOAD_BYTES,
        );
        let Ok(r) = reply else {
            stalls += 1;
            if stalls > 3 {
                return reply.map(|_| unreachable!());
            }
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        if r.status == 416 {
            break;
        }
        if !(200..300).contains(&r.status) {
            return Err(format!("Yandex audio HTTP {}", r.status));
        }
        if r.status == 200 {
            return Ok(r.body);
        }
        if let Some(ref cr) = r.content_range
            && let Some((_, total_str)) = cr.split_once('/')
            && let Ok(tot) = total_str.trim().parse::<usize>()
        {
            total_expected = Some(tot);
        }
        if r.body.is_empty() {
            break;
        }
        out.extend_from_slice(&r.body);
        offset = out.len();
        stalls = 0;
        if let Some(tot) = total_expected
            && offset >= tot
        {
            break;
        }
        if r.body.len() < RANGE_CHUNK && total_expected.is_none() {
            break;
        }
    }
    Ok(out)
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(xml[s..e].trim().to_owned())
}

fn join_artists(v: Option<&Value>) -> String {
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

fn parse_rotor_sequence(v: Option<&Value>) -> Vec<Track> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let obj = item.get("track").unwrap_or(item);
        if let Some(tr) = parse_track(obj) {
            out.push(tr);
        }
    }
    out
}

pub(crate) fn parse_track(t: &Value) -> Option<Track> {
    let id = json_id(t, "id");
    let title = json_str(t, "title").trim().to_owned();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    if t.get("available").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let artist = join_artists(t.get("artists"));
    let album = t
        .get("albums")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|al| al.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let duration_ms = json_u64(t, "durationMs");
    let cover_raw = json_str(t, "coverUri");
    let cover_url = if !cover_raw.is_empty() {
        yandex_cover_url(cover_raw, "200x200")
    } else {
        t.get("ogImage")
            .and_then(Value::as_str)
            .and_then(|u| yandex_cover_url(u, "200x200"))
    };
    Some(Track {
        id,
        title,
        artist,
        album,
        duration_ms,
        cover_url,
        provider: Platform::Yandex.id().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_download_xml_and_lyrics_hmac() {
        let xml = "<download-info><host>s123vla.storage.yandex.net</host><path>/rmusic/U2FsdGVk/file.mp3</path><ts>001234abcd</ts><s>secret_salt</s></download-info>";
        let url = YandexMusicProvider::sign_download_url(xml).unwrap();
        assert!(url.starts_with("https://s123vla.storage.yandex.net/get-mp3/"));
        assert!(url.ends_with("/001234abcd/rmusic/U2FsdGVk/file.mp3"));

        let sig1 = YandexMusicProvider::sign_lyrics_query("123456", 1700000000);
        let sig2 = YandexMusicProvider::sign_lyrics_query("123456", 1700000000);
        assert_eq!(sig1, sig2);
        assert!(!sig1.is_empty());
    }

    #[test]
    fn parses_yandex_track_json() {
        let v = json!({
            "id": 987654,
            "title": "Вино и сигареты",
            "durationMs": 154000,
            "artists": [{"name": "алёна швец."}],
            "albums": [{"title": "Королева отстоя"}],
            "coverUri": "avatars.yandex.net/get-music-content/1/2/%%"
        });
        let tr = parse_track(&v).unwrap();
        assert_eq!(tr.id, "987654");
        assert_eq!(tr.artist, "алёна швец.");
        assert_eq!(
            tr.cover_url.as_deref(),
            Some("https://avatars.yandex.net/get-music-content/1/2/200x200")
        );
    }
}
