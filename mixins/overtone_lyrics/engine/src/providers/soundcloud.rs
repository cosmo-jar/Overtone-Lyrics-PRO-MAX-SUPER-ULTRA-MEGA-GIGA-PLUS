//! SoundCloud provider, ported 1:1 from `SoundCloudProvider.java`.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::Value;

use crate::audio::AudioFormat;
use crate::config::TokenStore;
use crate::i18n;
use crate::local::AudioCache;
use crate::model::{EntityKind, MusicEntity, Page, Platform, Track};
use crate::net::{self, MAX_DOWNLOAD_BYTES};
use crate::providers::{
    MusicProvider, PlatformLyrics, PlaybackSource, SearchResult, json_id, json_str, json_u64,
};

const API: &str = "https://api-v2.soundcloud.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

struct PrefetchState {
    queue: VecDeque<String>,
    in_flight: HashSet<String>,
    stop: bool,
}

#[derive(Clone)]
pub struct SoundCloudProvider {
    tokens: Arc<Mutex<TokenStore>>,
    cache: AudioCache,
    client_id_lock: Arc<Mutex<String>>,
    prefetch_enabled: Arc<Mutex<bool>>,
    prefetch_state: Arc<(Mutex<PrefetchState>, Condvar)>,
}

impl SoundCloudProvider {
    pub fn new(tokens: Arc<Mutex<TokenStore>>, cache: AudioCache) -> Self {
        let initial_cid = tokens.lock().unwrap().soundcloud_client_id.clone();
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
            client_id_lock: Arc::new(Mutex::new(initial_cid)),
            prefetch_enabled: Arc::new(Mutex::new(true)),
            prefetch_state,
        };
        p.spawn_prefetch_workers();
        p
    }

    pub fn set_prefetch_enabled(&self, enabled: bool) {
        *self.prefetch_enabled.lock().unwrap() = enabled;
    }

    fn oauth(&self) -> String {
        self.tokens
            .lock()
            .unwrap()
            .soundcloud_oauth
            .trim()
            .to_owned()
    }

    fn user_urn(&self) -> String {
        self.tokens
            .lock()
            .unwrap()
            .soundcloud_user
            .trim()
            .to_owned()
    }

    pub fn is_valid_oauth_token(tok: &str) -> bool {
        let t = tok.trim();
        let clean = t
            .strip_prefix("OAuth ")
            .or_else(|| t.strip_prefix("Bearer "))
            .unwrap_or(t)
            .trim();
        if clean.is_empty() {
            return false;
        }
        // JWT or legacy `2-12345-67890-abcdef`
        (clean.starts_with("eyJ") && clean.matches('.').count() >= 2)
            || clean.contains('-') && clean.len() >= 16
    }

    pub fn verify_and_save_token(&self, raw_token: &str) -> Result<String, String> {
        let t = raw_token.trim();
        let clean = t
            .strip_prefix("OAuth ")
            .or_else(|| t.strip_prefix("Bearer "))
            .unwrap_or(t)
            .trim()
            .to_owned();
        if !Self::is_valid_oauth_token(&clean) {
            return Err("Invalid SoundCloud OAuth token format".to_owned());
        }
        let cid = self.client_id()?;
        let url = format!("{API}/me?client_id={}", net::url_encode(&cid));
        let auth_hdr = format!("OAuth {clean}");
        let me = net::get_json(
            &url,
            &[("Authorization", &auth_hdr), ("User-Agent", USER_AGENT)],
        )?;
        let uid = json_id(&me, "id");
        if uid.is_empty() {
            return Err("Could not resolve SoundCloud user profile".to_owned());
        }
        let username = json_str(&me, "username").to_owned();
        {
            let mut ts = self.tokens.lock().unwrap();
            ts.soundcloud_oauth = clean;
            ts.soundcloud_user = uid.clone();
            ts.save();
        }
        Ok(if username.is_empty() { uid } else { username })
    }

    pub fn set_profile_url(&self, input: &str) -> Result<String, String> {
        let mut s = input.trim().to_owned();
        if s.is_empty() {
            return Err("Empty SoundCloud profile URL".to_owned());
        }
        if !s.starts_with("https://") && !s.starts_with("http://") {
            let clean_slug = s.trim_start_matches('/');
            s = format!("https://soundcloud.com/{clean_slug}");
        }
        if s.starts_with("http://") {
            s = format!("https://{}", &s[7..]);
        }
        let resolved_url = if s.contains("on.soundcloud.com") || s.contains("snd.sc") {
            let reply = net::send("GET", &s, &[("User-Agent", USER_AGENT)], None, 512 * 1024)?;
            reply.final_url
        } else {
            s
        };
        let cid = self.client_id()?;
        let api_url = format!(
            "{API}/resolve?url={}&client_id={}",
            net::url_encode(&resolved_url),
            net::url_encode(&cid)
        );
        let obj = net::get_json(&api_url, &[("User-Agent", USER_AGENT)])?;
        let uid = json_id(&obj, "id");
        if uid.is_empty() {
            return Err("SoundCloud profile not found".to_owned());
        }
        let username = json_str(&obj, "username").to_owned();
        {
            let mut ts = self.tokens.lock().unwrap();
            ts.soundcloud_user = uid.clone();
            ts.save();
        }
        Ok(if username.is_empty() { uid } else { username })
    }

    pub fn clear_auth(&self) {
        let mut ts = self.tokens.lock().unwrap();
        ts.soundcloud_oauth.clear();
        ts.soundcloud_user.clear();
        ts.save();
    }

    pub fn client_id(&self) -> Result<String, String> {
        {
            let guard = self.client_id_lock.lock().unwrap();
            if !guard.is_empty() {
                return Ok(guard.clone());
            }
        }
        self.scrape_client_id()
    }

    fn scrape_client_id(&self) -> Result<String, String> {
        let html = net::get_text(
            "https://soundcloud.com/discover",
            &[("User-Agent", USER_AGENT)],
        )?;
        let mut script_urls = Vec::new();
        let mut pos = 0usize;
        while let Some(found) = html[pos..].find("https://a-v2.sndcdn.com/assets/") {
            let start = pos + found;
            let rest = &html[start..];
            if let Some(end) = rest.find(['"', '\'']) {
                let candidate = &rest[..end];
                if candidate.ends_with(".js") {
                    script_urls.push(candidate.to_owned());
                }
                pos = start + end;
            } else {
                break;
            }
        }
        for js_url in script_urls.into_iter().rev().take(6) {
            let Ok(js) = net::get_text(&js_url, &[("User-Agent", USER_AGENT)]) else {
                continue;
            };
            if let Some(cid) = extract_client_id_from_js(&js) {
                *self.client_id_lock.lock().unwrap() = cid.clone();
                let mut ts = self.tokens.lock().unwrap();
                ts.soundcloud_client_id = cid.clone();
                ts.save();
                return Ok(cid);
            }
        }
        Err("Failed to discover SoundCloud client_id".to_owned())
    }

    fn get_api_json(&self, path_with_query: &str) -> Result<Value, String> {
        let cid = self.client_id()?;
        let sep = if path_with_query.contains('?') {
            '&'
        } else {
            '?'
        };
        let url = format!(
            "{API}{path_with_query}{sep}client_id={}",
            net::url_encode(&cid)
        );
        let tok = self.oauth();
        let mut hdrs: Vec<(&str, String)> = vec![("User-Agent", USER_AGENT.to_owned())];
        if !tok.is_empty() {
            hdrs.push(("Authorization", format!("OAuth {tok}")));
        }
        let h_refs: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        match net::get_json(&url, &h_refs) {
            Ok(v) => Ok(v),
            Err(e) if e.contains("401") || e.contains("403") => {
                // Retry once after re-scraping client_id
                let fresh_cid = self.scrape_client_id()?;
                let url2 = format!(
                    "{API}{path_with_query}{sep}client_id={}",
                    net::url_encode(&fresh_cid)
                );
                net::get_json(&url2, &h_refs)
            }
            Err(e) => Err(e),
        }
    }

    fn download_bytes(&self, track_id: &str) -> Result<Vec<u8>, String> {
        if let Some(cached) = self.cache.get(track_id) {
            return Ok(cached);
        }
        let obj = self.get_api_json(&format!("/tracks/{}", net::url_encode(track_id)))?;
        let media = obj
            .get("media")
            .and_then(|m| m.get("transcodings"))
            .and_then(Value::as_array)
            .ok_or_else(|| "No SoundCloud transcodings available".to_owned())?;

        let mut ranked: Vec<(i32, &Value)> = media
            .iter()
            .filter_map(|tc| Some((score_transcoding(tc)?, tc)))
            .collect();
        ranked.sort_by_key(|(s, _)| *s);

        let cid = self.client_id()?;
        let track_auth = json_str(&obj, "track_authorization");
        let tok = self.oauth();

        let mut last_err = "No playable SoundCloud stream".to_owned();
        for (_, tc) in ranked {
            let tc_url = json_str(tc, "url");
            if tc_url.is_empty() {
                continue;
            }
            let sep = if tc_url.contains('?') { '&' } else { '?' };
            let mut stream_req = format!("{tc_url}{sep}client_id={}", net::url_encode(&cid));
            if !track_auth.is_empty() {
                stream_req.push_str("&track_authorization=");
                stream_req.push_str(&net::url_encode(track_auth));
            }
            let mut hdrs: Vec<(&str, String)> = vec![("User-Agent", USER_AGENT.to_owned())];
            if !tok.is_empty() {
                hdrs.push(("Authorization", format!("OAuth {tok}")));
            }
            let h_refs: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let resolved = match net::get_json(&stream_req, &h_refs) {
                Ok(v) => v,
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            let stream_url = json_str(&resolved, "url");
            if stream_url.is_empty() {
                continue;
            }
            let proto = tc
                .get("format")
                .and_then(|f| f.get("protocol"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let data = if proto.eq_ignore_ascii_case("hls") {
                download_hls(stream_url)
            } else {
                net::get_bytes(stream_url, &[("User-Agent", USER_AGENT)])
            };
            match data {
                Ok(bytes) if bytes.len() > 512 => {
                    self.cache.put(track_id, bytes.clone());
                    return Ok(bytes);
                }
                Ok(_) => last_err = "Downloaded empty SoundCloud stream".to_owned(),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn spawn_prefetch_workers(&self) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-sc-prefetch".to_string())
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

impl Drop for SoundCloudProvider {
    fn drop(&mut self) {
        if Arc::strong_count(&self.prefetch_state) == 1 {
            let (lock, cvar) = &*self.prefetch_state;
            lock.lock().unwrap().stop = true;
            cvar.notify_all();
        }
    }
}

impl MusicProvider for SoundCloudProvider {
    fn platform(&self) -> Platform {
        Platform::SoundCloud
    }

    fn has_auth(&self) -> bool {
        !self.oauth().is_empty() || !self.user_urn().is_empty()
    }

    fn search(&self, query: &str) -> Result<SearchResult, String> {
        let q = net::url_encode(query.trim());
        let tracks_json = self.get_api_json(&format!("/search/tracks?q={q}&limit=40"))?;
        let mut out = SearchResult::default();
        if let Some(arr) = tracks_json.get("collection").and_then(Value::as_array) {
            for t in arr {
                if let Some(tr) = parse_sc_track(t) {
                    out.tracks.push(tr);
                }
            }
        }
        if let Ok(users_json) = self.get_api_json(&format!("/search/users?q={q}&limit=6"))
            && let Some(arr) = users_json.get("collection").and_then(Value::as_array)
        {
            for u in arr {
                let id = json_id(u, "id");
                let name = json_str(u, "username").to_owned();
                if !id.is_empty() && !name.is_empty() {
                    out.entities.push(MusicEntity {
                        kind: EntityKind::Artist,
                        id,
                        title: name,
                        subtitle: i18n::get("kind.artist"),
                        cover_url: clean_artwork(json_str(u, "avatar_url")),
                    });
                }
            }
        }
        if let Ok(pl_json) = self.get_api_json(&format!("/search/playlists?q={q}&limit=8"))
            && let Some(arr) = pl_json.get("collection").and_then(Value::as_array)
        {
            for pl in arr {
                if let Some(ent) = parse_sc_playlist(pl) {
                    out.entities.push(ent);
                }
            }
        }
        Ok(out)
    }

    fn liked_tracks(&self) -> Result<Vec<Track>, String> {
        let uid = self.user_urn();
        if uid.is_empty() {
            return Err("Connect a SoundCloud profile or OAuth token first".to_owned());
        }
        let val =
            self.get_api_json(&format!("/users/{}/likes?limit=100", net::url_encode(&uid)))?;
        let mut out = Vec::new();
        if let Some(arr) = val.get("collection").and_then(Value::as_array) {
            for item in arr {
                let obj = item.get("track").unwrap_or(item);
                if let Some(tr) = parse_sc_track(obj) {
                    out.push(tr);
                }
            }
        }
        Ok(out)
    }

    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String> {
        let uid = self.user_urn();
        if uid.is_empty() {
            return Ok(Vec::new());
        }
        let val = self.get_api_json(&format!(
            "/users/{}/playlists?limit=50",
            net::url_encode(&uid)
        ))?;
        let mut out = Vec::new();
        if let Some(arr) = val.get("collection").and_then(Value::as_array) {
            for pl in arr {
                if let Some(ent) = parse_sc_playlist(pl) {
                    out.push(ent);
                }
            }
        }
        Ok(out)
    }

    fn open(&self, entity: &MusicEntity) -> Result<Page, String> {
        match entity.kind {
            EntityKind::Artist => {
                let val = self.get_api_json(&format!(
                    "/users/{}/tracks?limit=50",
                    net::url_encode(&entity.id)
                ))?;
                let mut tracks = Vec::new();
                if let Some(arr) = val.get("collection").and_then(Value::as_array) {
                    for t in arr {
                        if let Some(tr) = parse_sc_track(t) {
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
            EntityKind::Playlist | EntityKind::Album => {
                if entity.is_mine() {
                    return Ok(Page {
                        source: entity.clone(),
                        tracks: self.liked_tracks()?,
                        entities: Vec::new(),
                    });
                }
                let val =
                    self.get_api_json(&format!("/playlists/{}", net::url_encode(&entity.id)))?;
                let mut tracks = Vec::new();
                let mut missing_ids = Vec::new();
                if let Some(arr) = val.get("tracks").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(tr) = parse_sc_track(item) {
                            tracks.push(tr);
                        } else {
                            let id = json_id(item, "id");
                            if !id.is_empty() {
                                missing_ids.push(id);
                            }
                        }
                    }
                }
                for chunk in missing_ids.chunks(50) {
                    let ids = chunk.join(",");
                    if let Ok(batch) =
                        self.get_api_json(&format!("/tracks?ids={}", net::url_encode(&ids)))
                        && let Some(arr) = batch.as_array()
                    {
                        for t in arr {
                            if let Some(tr) = parse_sc_track(t) {
                                tracks.push(tr);
                            }
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
        let tok = self.oauth();
        let uid = self.user_urn();
        if tok.is_empty() || uid.is_empty() {
            return Err("SoundCloud OAuth token required for liking tracks".to_owned());
        }
        let cid = self.client_id()?;
        let url = format!(
            "{API}/users/{}/track_likes/{}?client_id={}",
            net::url_encode(&uid),
            net::url_encode(&track.id),
            net::url_encode(&cid)
        );
        let auth_hdr = format!("OAuth {tok}");
        let method = if liked { "PUT" } else { "DELETE" };
        let reply = net::send(
            method,
            &url,
            &[("Authorization", &auth_hdr), ("User-Agent", USER_AGENT)],
            None,
            64 * 1024,
        )?;
        if !(200..300).contains(&reply.status) {
            return Err(format!("SoundCloud like HTTP {}", reply.status));
        }
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

fn extract_client_id_from_js(js: &str) -> Option<String> {
    let mut idx = 0;
    while let Some(found) = js[idx..].find("client_id") {
        let start = idx + found;
        let window = &js[start..js.len().min(start + 64)];
        for sep in ["client_id:\"", "client_id=\"", "client_id:'", "client_id='"] {
            if let Some(s) = window.find(sep) {
                let val_start = s + sep.len();
                let quote = sep.chars().last().unwrap();
                if let Some(end) = window[val_start..].find(quote) {
                    let cand = &window[val_start..val_start + end];
                    if cand.len() == 32 && cand.bytes().all(|b| b.is_ascii_alphanumeric()) {
                        return Some(cand.to_owned());
                    }
                }
            }
        }
        idx = start + 9;
    }
    None
}

pub(crate) fn score_transcoding(tc: &Value) -> Option<i32> {
    let fmt = tc.get("format")?;
    let mime = json_str(fmt, "mime_type").to_ascii_lowercase();
    let proto = json_str(fmt, "protocol").to_ascii_lowercase();
    let base = if mime.contains("mpeg") {
        if proto == "progressive" {
            0
        } else if proto == "hls" {
            1
        } else {
            return None;
        }
    } else if mime.contains("mp4") || mime.contains("aac") {
        if proto == "progressive" {
            2
        } else if proto == "hls" {
            3
        } else {
            return None;
        }
    } else {
        return None;
    };
    let snipped = tc.get("snipped").and_then(Value::as_bool).unwrap_or(false);
    Some(base + if snipped { 10 } else { 0 })
}

pub(crate) fn parse_m3u8_segments(playlist: &str, base_url: &str) -> Vec<String> {
    let Ok(base) = url::Url::parse(base_url) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw_line in playlist.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some(u_start) = rest.find("URI=\"") {
                let s = &rest[u_start + 5..];
                if let Some(u_end) = s.find('"')
                    && let Ok(joined) = base.join(&s[..u_end])
                {
                    out.push(joined.to_string());
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Ok(joined) = base.join(line) {
            out.push(joined.to_string());
        }
    }
    out
}

fn download_hls(m3u8_url: &str) -> Result<Vec<u8>, String> {
    let playlist = net::get_text(m3u8_url, &[("User-Agent", USER_AGENT)])?;
    let segs = parse_m3u8_segments(&playlist, m3u8_url);
    if segs.is_empty() {
        return Err("Empty SoundCloud HLS playlist".to_owned());
    }
    let mut combined = Vec::new();
    for seg_url in segs {
        let part = net::send(
            "GET",
            &seg_url,
            &[("User-Agent", USER_AGENT)],
            None,
            MAX_DOWNLOAD_BYTES,
        )?;
        if !(200..300).contains(&part.status) {
            return Err(format!("HLS segment HTTP {}", part.status));
        }
        if combined.len() + part.body.len() > MAX_DOWNLOAD_BYTES {
            return Err("HLS audio exceeded download cap".to_owned());
        }
        combined.extend_from_slice(&part.body);
    }
    Ok(combined)
}

fn clean_artwork(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let fixed = s.replace("-large.", "-t500x500.");
    if fixed.starts_with("https://") {
        Some(fixed)
    } else {
        fixed
            .strip_prefix("http://")
            .map(|rest| format!("https://{rest}"))
    }
}

fn parse_sc_track(t: &Value) -> Option<Track> {
    let id = json_id(t, "id");
    let title = json_str(t, "title").trim().to_owned();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let artist = t
        .get("publisher_metadata")
        .and_then(|p| p.get("artist"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            t.get("user")
                .and_then(|u| u.get("username"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .trim()
        .to_owned();
    let duration_ms = json_u64(t, "duration");
    let cover = clean_artwork(json_str(t, "artwork_url")).or_else(|| {
        t.get("user")
            .and_then(|u| u.get("avatar_url"))
            .and_then(Value::as_str)
            .and_then(clean_artwork)
    });
    Some(Track {
        id,
        title,
        artist,
        album: String::new(),
        duration_ms,
        cover_url: cover,
        provider: Platform::SoundCloud.id().to_owned(),
    })
}

fn parse_sc_playlist(pl: &Value) -> Option<MusicEntity> {
    let id = json_id(pl, "id");
    let title = json_str(pl, "title").trim().to_owned();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let kind = if pl.get("is_album").and_then(Value::as_bool) == Some(true) {
        EntityKind::Album
    } else {
        EntityKind::Playlist
    };
    let sub = pl
        .get("user")
        .and_then(|u| u.get("username"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let cover = clean_artwork(json_str(pl, "artwork_url"));
    Some(MusicEntity {
        kind,
        id,
        title,
        subtitle: sub,
        cover_url: cover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ranks_progressive_mp3_over_hls_and_aac_and_snipped() {
        let p_mp3 =
            json!({"format":{"mime_type":"audio/mpeg","protocol":"progressive"},"snipped":false});
        let h_mp3 = json!({"format":{"mime_type":"audio/mpeg","protocol":"hls"},"snipped":false});
        let h_aac = json!({"format":{"mime_type":"audio/mp4; codecs=\"mp4a.40.2\"","protocol":"hls"},"snipped":false});
        let snip =
            json!({"format":{"mime_type":"audio/mpeg","protocol":"progressive"},"snipped":true});
        assert_eq!(score_transcoding(&p_mp3), Some(0));
        assert_eq!(score_transcoding(&h_mp3), Some(1));
        assert_eq!(score_transcoding(&h_aac), Some(3));
        assert_eq!(score_transcoding(&snip), Some(10));
    }

    #[test]
    fn parses_m3u8_init_map_and_segments() {
        let m3u8 = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\nseg1.m4s\nhttps://cf-hls-media.sndcdn.com/seg2.m4s\n";
        let segs = parse_m3u8_segments(m3u8, "https://cf-hls-media.sndcdn.com/playlist/p.m3u8");
        assert_eq!(
            segs,
            vec![
                "https://cf-hls-media.sndcdn.com/playlist/init.mp4",
                "https://cf-hls-media.sndcdn.com/playlist/seg1.m4s",
                "https://cf-hls-media.sndcdn.com/seg2.m4s",
            ]
        );
    }
}
