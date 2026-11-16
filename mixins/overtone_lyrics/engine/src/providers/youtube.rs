//! YouTube & YouTube Music InnerTube provider (no external binaries / no yt-dlp required).

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::audio::AudioFormat;
use crate::config::{TokenStore, atomic_write_bytes, data_dir};
use crate::i18n;
use crate::model::{
    EntityKind, LyricLine, MusicEntity, Page, Platform, Track, WordTiming, split_track_name,
};
use crate::net::{self, MAX_DOWNLOAD_BYTES};
use crate::providers::{
    MusicProvider, PlatformLyrics, PlaybackSource, SearchResult, json_str, json_u64,
};

const YTM_BASE: &str = "https://music.youtube.com/youtubei/v1";
const YT_BASE: &str = "https://www.youtube.com/youtubei/v1";
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

struct PlayerClientProfile {
    name: &'static str,
    version: &'static str,
    x_client_id: &'static str,
    ua: &'static str,
    sdk: Option<u32>,
}

const STREAM_CLIENTS: &[PlayerClientProfile] = &[
    PlayerClientProfile {
        name: "ANDROID",
        version: "20.10.38",
        x_client_id: "3",
        ua: "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip",
        sdk: Some(34),
    },
    PlayerClientProfile {
        name: "IOS",
        version: "20.10.4",
        x_client_id: "5",
        ua: "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
        sdk: None,
    },
    PlayerClientProfile {
        name: "ANDROID_VR",
        version: "1.60.19",
        x_client_id: "28",
        ua: "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
        sdk: Some(32),
    },
    PlayerClientProfile {
        name: "TVHTML5",
        version: "7.20260101.00.00",
        x_client_id: "7",
        ua: WEB_UA,
        sdk: None,
    },
];

#[derive(Clone)]
pub struct YouTubeProvider {
    tokens: Arc<Mutex<TokenStore>>,
    likes_lock: Arc<Mutex<()>>,
    visitor_data: Arc<Mutex<String>>,
}

impl YouTubeProvider {
    pub fn new(tokens: Arc<Mutex<TokenStore>>) -> Self {
        Self {
            tokens,
            likes_lock: Arc::new(Mutex::new(())),
            visitor_data: Arc::new(Mutex::new(String::new())),
        }
    }

    fn cookie(&self) -> String {
        self.tokens.lock().unwrap().youtube_cookie.trim().to_owned()
    }

    pub fn has_auth(&self) -> bool {
        let c = self.cookie();
        !c.is_empty() && extract_sapisid(&c).is_some()
    }

    pub fn clear_auth(&self) {
        let mut ts = self.tokens.lock().unwrap();
        ts.youtube_cookie.clear();
        ts.youtube_user.clear();
        ts.save();
    }

    pub fn verify_and_save_cookie(&self, raw_input: &str) -> Result<String, String> {
        let cookie = normalize_cookie_input(raw_input);
        let Some(sapisid) = extract_sapisid(&cookie) else {
            return Err(i18n::get("err.yt_bad_cookie"));
        };
        let mut acct_body = json!({});
        if let Some(obj) = acct_body.as_object_mut() {
            obj.insert("context".to_owned(), Self::ytm_context());
        }
        let menu = self.post_json_with_cookie(
            &format!("{YTM_BASE}/account/account_menu?prettyPrint=false"),
            &acct_body,
            "https://music.youtube.com",
            WEB_UA,
            Some((&cookie, &sapisid)),
        )?;

        // Also check private Liked Music browse to ensure session is genuinely signed in
        let mut lm_body = json!({ "browseId": "LM" });
        if let Some(obj) = lm_body.as_object_mut() {
            obj.insert("context".to_owned(), Self::ytm_context());
        }
        let lm_ok = self
            .post_json_with_cookie(
                &format!("{YTM_BASE}/browse?prettyPrint=false"),
                &lm_body,
                "https://music.youtube.com",
                WEB_UA,
                Some((&cookie, &sapisid)),
            )
            .is_ok();

        let account_name = extract_ytm_account_name(&menu);
        if account_name.is_empty() && !lm_ok {
            return Err(i18n::get("err.yt_bad_cookie"));
        }
        let label = if account_name.is_empty() {
            "YouTube".to_owned()
        } else {
            account_name
        };

        let mut ts = self.tokens.lock().unwrap();
        ts.youtube_cookie = cookie;
        ts.youtube_user = label.clone();
        ts.save();
        Ok(label)
    }

    fn likes_path() -> PathBuf {
        data_dir().join("youtube_likes.json")
    }

    fn read_likes(&self) -> Vec<Track> {
        let _g = self.likes_lock.lock().unwrap();
        let path = Self::likes_path();
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Track>>(&s).ok())
            .unwrap_or_default()
    }

    fn write_likes(&self, tracks: &[Track]) -> Result<(), String> {
        let _g = self.likes_lock.lock().unwrap();
        let bytes = serde_json::to_vec_pretty(tracks).map_err(|e| e.to_string())?;
        atomic_write_bytes(&Self::likes_path(), &bytes).map_err(|e| e.to_string())
    }

    fn yt_hl() -> &'static str {
        match i18n::current_language() {
            i18n::Language::RuRu => "ru",
            i18n::Language::JaJp => "ja",
            i18n::Language::EnUs => "en",
        }
    }

    fn ytm_context() -> Value {
        json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20260101.01.00",
                "hl": Self::yt_hl()
            }
        })
    }

    fn web_context() -> Value {
        json!({
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20260101.00.00",
                "hl": Self::yt_hl()
            }
        })
    }

    fn post_json_with_cookie(
        &self,
        url: &str,
        body: &Value,
        origin: &str,
        ua: &str,
        auth: Option<(&str, &str)>,
    ) -> Result<Value, String> {
        let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let mut hdrs: Vec<(&str, String)> = vec![
            ("Content-Type", "application/json".to_owned()),
            ("User-Agent", ua.to_owned()),
            ("Origin", origin.to_owned()),
            ("Referer", format!("{origin}/")),
        ];
        if let Some((cookie, sapisid)) = auth {
            hdrs.push(("Cookie", cookie.to_owned()));
            hdrs.push(("X-Origin", origin.to_owned()));
            hdrs.push(("X-Goog-AuthUser", "0".to_owned()));
            hdrs.push(("Authorization", sapisid_hash(sapisid, origin)));
        }
        let borrowed: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let reply = net::send("POST", url, &borrowed, Some(&payload), 8 << 20)?;
        if !(200..300).contains(&reply.status) {
            return Err(format!("YouTube HTTP {}", reply.status));
        }
        serde_json::from_slice(&reply.body).map_err(|e| format!("YouTube JSON error: {e}"))
    }

    fn post_json(&self, url: &str, body: &Value, origin: &str, ua: &str) -> Result<Value, String> {
        let cookie = self.cookie();
        if let Some(sapisid) = extract_sapisid(&cookie) {
            self.post_json_with_cookie(url, body, origin, ua, Some((&cookie, &sapisid)))
        } else {
            self.post_json_with_cookie(url, body, origin, ua, None)
        }
    }

    fn capture_visitor(&self, v: &Value) {
        if let Some(vd) = v
            .get("responseContext")
            .and_then(|r| r.get("visitorData"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            *self.visitor_data.lock().unwrap() = vd.to_owned();
        }
    }

    fn ensure_visitor_data(&self) -> String {
        let cur = self.visitor_data.lock().unwrap().clone();
        if !cur.is_empty() {
            return cur;
        }
        let _ = self.post_yt_web("search", json!({ "query": "music" }));
        self.visitor_data.lock().unwrap().clone()
    }

    fn post_ytm(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("context".to_owned(), Self::ytm_context());
        }
        let v = self.post_json(
            &format!("{YTM_BASE}/{endpoint}?prettyPrint=false"),
            &body,
            "https://music.youtube.com",
            WEB_UA,
        )?;
        self.capture_visitor(&v);
        Ok(v)
    }

    fn post_yt_web(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("context".to_owned(), Self::web_context());
        }
        let v = self.post_json(
            &format!("{YT_BASE}/{endpoint}?prettyPrint=false"),
            &body,
            "https://www.youtube.com",
            WEB_UA,
        )?;
        self.capture_visitor(&v);
        Ok(v)
    }

    fn post_player(&self, video_id: &str, profile: &PlayerClientProfile) -> Result<Value, String> {
        let vdata = self.ensure_visitor_data();
        let mut client = json!({
            "clientName": profile.name,
            "clientVersion": profile.version,
            "hl": "en",
            "gl": "US"
        });
        if let Some(obj) = client.as_object_mut() {
            if let Some(sdk) = profile.sdk {
                obj.insert("androidSdkVersion".to_owned(), json!(sdk));
                obj.insert("osName".to_owned(), json!("Android"));
                obj.insert("osVersion".to_owned(), json!("14"));
            } else if profile.name == "IOS" {
                obj.insert("deviceMake".to_owned(), json!("Apple"));
                obj.insert("deviceModel".to_owned(), json!("iPhone16,2"));
                obj.insert("osName".to_owned(), json!("iPhone"));
                obj.insert("osVersion".to_owned(), json!("18.3.2"));
            }
            if !vdata.is_empty() {
                obj.insert("visitorData".to_owned(), json!(vdata));
            }
        }
        let body = json!({
            "context": { "client": client },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyOk": true
        });
        let payload = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let mut hdrs: Vec<(&str, &str)> = vec![
            ("Content-Type", "application/json"),
            ("User-Agent", profile.ua),
            ("X-YouTube-Client-Name", profile.x_client_id),
            ("X-YouTube-Client-Version", profile.version),
        ];
        if !vdata.is_empty() {
            hdrs.push(("X-Goog-Visitor-Id", &vdata));
        }
        let reply = net::send(
            "POST",
            &format!("{YT_BASE}/player?prettyPrint=false"),
            &hdrs,
            Some(&payload),
            8 << 20,
        )?;
        if !(200..300).contains(&reply.status) {
            return Err(format!("YouTube HTTP {}", reply.status));
        }
        serde_json::from_slice(&reply.body).map_err(|e| format!("YouTube JSON error: {e}"))
    }

    fn fetch_single_video(&self, video_id: &str) -> Result<Track, String> {
        let mut last_err = String::from("Video unavailable");
        for profile in STREAM_CLIENTS {
            match self.post_player(video_id, profile) {
                Ok(val) => {
                    if let Some(det) = val.get("videoDetails") {
                        let raw_title = json_str(det, "title");
                        let author = json_str(det, "author");
                        if !raw_title.is_empty() {
                            let (artist, title) = split_track_name(raw_title, author);
                            let secs = json_u64(det, "lengthSeconds");
                            let thumb = det
                                .get("thumbnail")
                                .and_then(|t| t.get("thumbnails"))
                                .and_then(Value::as_array)
                                .and_then(|arr| arr.last())
                                .and_then(|t| t.get("url"))
                                .and_then(Value::as_str)
                                .map(upscale_yt_thumb)
                                .or_else(|| {
                                    Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))
                                });
                            return Ok(Track {
                                id: video_id.to_owned(),
                                title,
                                artist,
                                album: String::new(),
                                duration_ms: secs * 1000,
                                cover_url: thumb,
                                provider: Platform::YouTube.id().to_owned(),
                            });
                        }
                    }
                    if let Some(reason) = val
                        .get("playabilityStatus")
                        .and_then(|p| p.get("reason"))
                        .and_then(Value::as_str)
                    {
                        last_err = reason.to_owned();
                    }
                }
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    /// Loads YouTube Music charts (top tracks & playlists) for quick browsing without a search query.
    pub fn charts_page(&self) -> Result<Page, String> {
        let val = self.post_ytm("browse", json!({ "browseId": "FEmusic_charts" }))?;
        let mut tracks = Vec::new();
        let mut entities = Vec::new();
        let mut seen_t = HashSet::new();
        let mut seen_e = HashSet::new();

        for it in collect_key(&val, "musicResponsiveListItemRenderer") {
            match parse_ytm_item(it) {
                Some(ParsedYtmItem::Track(tr)) if seen_t.insert(tr.id.clone()) => tracks.push(tr),
                Some(ParsedYtmItem::Entity(en)) if seen_e.insert((en.kind, en.id.clone())) => {
                    entities.push(en);
                }
                _ => {}
            }
        }
        for it in collect_key(&val, "musicTwoRowItemRenderer") {
            match parse_ytm_two_row(it) {
                Some(ParsedYtmItem::Track(tr)) if seen_t.insert(tr.id.clone()) => tracks.push(tr),
                Some(ParsedYtmItem::Entity(en)) if seen_e.insert((en.kind, en.id.clone())) => {
                    entities.push(en);
                }
                _ => {}
            }
        }
        Ok(Page {
            source: MusicEntity {
                kind: EntityKind::Playlist,
                id: "FEmusic_charts".to_owned(),
                title: i18n::get("player.yt_charts"),
                subtitle: "YouTube Music".to_owned(),
                cover_url: None,
            },
            tracks,
            entities,
        })
    }

    fn resolve_track_entity(&self, entity: &MusicEntity) -> Result<MusicEntity, String> {
        let Some(video_id) = entity.id.strip_prefix("track:") else {
            return Ok(entity.clone());
        };

        if let Ok(nxt) = self.post_ytm("next", json!({ "videoId": video_id })) {
            for r in collect_key(&nxt, "playlistPanelVideoRenderer") {
                if json_str(r, "videoId") != video_id && !json_str(r, "videoId").is_empty() {
                    continue;
                }
                if let Some(runs) = r
                    .get("longBylineText")
                    .and_then(|l| l.get("runs"))
                    .and_then(Value::as_array)
                {
                    for run in runs {
                        let bid = run
                            .get("navigationEndpoint")
                            .and_then(|n| n.get("browseEndpoint"))
                            .and_then(|b| b.get("browseId"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if bid.is_empty() {
                            continue;
                        }
                        let wanted = match entity.kind {
                            EntityKind::Artist => bid.starts_with("UC"),
                            EntityKind::Album => {
                                bid.starts_with("MPREb_") || bid.starts_with("OLAK")
                            }
                            _ => false,
                        };
                        if wanted {
                            return Ok(MusicEntity {
                                kind: entity.kind,
                                id: bid.to_owned(),
                                title: json_str(run, "text").to_owned(),
                                subtitle: entity.subtitle.clone(),
                                cover_url: entity.cover_url.clone(),
                            });
                        }
                    }
                }
            }
        }

        // Fallback to catalog search by artist or album name
        let q = if entity.kind == EntityKind::Album && !entity.subtitle.is_empty() {
            format!("{} {}", entity.subtitle, entity.title)
        } else {
            entity.title.clone()
        };
        if !q.trim().is_empty()
            && let Ok(found) = self.search(&q)
            && let Some(matched) = found.entities.into_iter().find(|e| e.kind == entity.kind)
        {
            return Ok(matched);
        }

        Err("Entity browseId not found".to_owned())
    }
}

impl MusicProvider for YouTubeProvider {
    fn platform(&self) -> Platform {
        Platform::YouTube
    }

    fn has_auth(&self) -> bool {
        self.has_auth()
    }

    fn search(&self, query: &str) -> Result<SearchResult, String> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(SearchResult::default());
        }

        // Support pasting direct YouTube / YouTube Music track or playlist URLs
        if let Some(target) = extract_video_or_list(q) {
            match target {
                YtLinkTarget::Video(vid) => {
                    let tr = self.fetch_single_video(&vid)?;
                    return Ok(SearchResult {
                        tracks: vec![tr],
                        entities: Vec::new(),
                    });
                }
                YtLinkTarget::Playlist(pid) => {
                    let browse_id = if pid.starts_with("VL") {
                        pid
                    } else {
                        format!("VL{pid}")
                    };
                    let page = self.open(&MusicEntity {
                        kind: EntityKind::Playlist,
                        id: browse_id,
                        title: "Playlist".to_owned(),
                        subtitle: "YouTube".to_owned(),
                        cover_url: None,
                    })?;
                    return Ok(SearchResult {
                        tracks: page.tracks,
                        entities: vec![page.source],
                    });
                }
            }
        }

        let mut out = SearchResult::default();
        let mut seen_tracks = HashSet::new();
        let mut seen_entities = HashSet::new();

        if let Ok(ytm) = self.post_ytm("search", json!({ "query": q })) {
            for item in collect_key(&ytm, "musicResponsiveListItemRenderer") {
                match parse_ytm_item(item) {
                    Some(ParsedYtmItem::Track(tr)) if seen_tracks.insert(tr.id.clone()) => {
                        out.tracks.push(tr);
                    }
                    Some(ParsedYtmItem::Entity(en))
                        if seen_entities.insert((en.kind, en.id.clone())) =>
                    {
                        out.entities.push(en);
                    }
                    _ => {}
                }
            }
        }

        // Supplement with standard YouTube video search so covers/live versions/videos appear too.
        if let Ok(yt_web) = self.post_yt_web("search", json!({ "query": q })) {
            for vr in collect_key(&yt_web, "videoRenderer").into_iter().take(18) {
                if let Some(tr) = parse_yt_video_renderer(vr) {
                    if seen_tracks.insert(tr.id.clone()) {
                        out.tracks.push(tr);
                    } else if tr.duration_ms > 0
                        && let Some(existing) = out
                            .tracks
                            .iter_mut()
                            .find(|t| t.id == tr.id && t.duration_ms == 0)
                    {
                        existing.duration_ms = tr.duration_ms;
                    }
                }
            }
        }

        if out.tracks.is_empty() && out.entities.is_empty() {
            return Err("No results on YouTube".to_owned());
        }
        Ok(out)
    }

    fn liked_tracks(&self) -> Result<Vec<Track>, String> {
        if self.has_auth() {
            let val = self.post_ytm("browse", json!({ "browseId": "LM" }))?;
            let mut tracks = Vec::new();
            let mut seen = HashSet::new();
            for item in collect_key(&val, "musicResponsiveListItemRenderer") {
                if let Some(ParsedYtmItem::Track(tr)) = parse_ytm_item(item)
                    && seen.insert(tr.id.clone())
                {
                    tracks.push(tr);
                }
            }
            return Ok(tracks);
        }
        Ok(self.read_likes())
    }

    fn my_playlists(&self) -> Result<Vec<MusicEntity>, String> {
        if self.has_auth() {
            let val = self.post_ytm("browse", json!({ "browseId": "FEmusic_liked_playlists" }))?;
            let mut entities = Vec::new();
            let mut seen = HashSet::new();
            for item in collect_key(&val, "musicTwoRowItemRenderer") {
                if let Some(ParsedYtmItem::Entity(en)) = parse_ytm_two_row(item)
                    && en.id != "VLLM"
                    && seen.insert((en.kind, en.id.clone()))
                {
                    entities.push(en);
                }
            }
            for item in collect_key(&val, "musicResponsiveListItemRenderer") {
                if let Some(ParsedYtmItem::Entity(en)) = parse_ytm_item(item)
                    && en.id != "VLLM"
                    && seen.insert((en.kind, en.id.clone()))
                {
                    entities.push(en);
                }
            }
            return Ok(entities);
        }
        let p = self.charts_page()?;
        Ok(p.entities)
    }

    fn open(&self, entity: &MusicEntity) -> Result<Page, String> {
        if entity.is_mine() {
            return Ok(Page {
                source: entity.clone(),
                tracks: self.read_likes(),
                entities: Vec::new(),
            });
        }

        let resolved = self.resolve_track_entity(entity)?;
        let val = self.post_ytm("browse", json!({ "browseId": resolved.id }))?;

        let mut tracks = Vec::new();
        let mut entities = Vec::new();
        let mut seen_t = HashSet::new();
        let mut seen_e = HashSet::new();

        let fallback_album = if resolved.kind == EntityKind::Album {
            resolved.title.clone()
        } else {
            String::new()
        };
        let fallback_artist = if resolved.kind == EntityKind::Artist {
            resolved.title.clone()
        } else if resolved.kind == EntityKind::Album {
            resolved.subtitle.clone()
        } else {
            String::new()
        };
        let fallback_cover = resolved.cover_url.clone();

        for item in collect_key(&val, "musicResponsiveListItemRenderer") {
            match parse_ytm_item(item) {
                Some(ParsedYtmItem::Track(mut tr)) if seen_t.insert(tr.id.clone()) => {
                    if tr.artist.is_empty() {
                        tr.artist = fallback_artist.clone();
                    }
                    if tr.album.is_empty() {
                        tr.album = fallback_album.clone();
                    }
                    if tr.cover_url.is_none() {
                        tr.cover_url = fallback_cover.clone();
                    }
                    tracks.push(tr);
                }
                Some(ParsedYtmItem::Entity(en))
                    if en.id != resolved.id && seen_e.insert((en.kind, en.id.clone())) =>
                {
                    entities.push(en);
                }
                _ => {}
            }
        }
        for item in collect_key(&val, "musicTwoRowItemRenderer") {
            match parse_ytm_two_row(item) {
                Some(ParsedYtmItem::Track(tr)) if seen_t.insert(tr.id.clone()) => tracks.push(tr),
                Some(ParsedYtmItem::Entity(en))
                    if en.id != resolved.id && seen_e.insert((en.kind, en.id.clone())) =>
                {
                    entities.push(en);
                }
                _ => {}
            }
        }

        Ok(Page {
            source: resolved,
            tracks,
            entities,
        })
    }

    fn fetch_lyrics(&self, track: &Track) -> PlatformLyrics {
        let mut out = PlatformLyrics::default();

        // 1. Try YouTube Music lyrics (MPLYt_... via /next -> /browse)
        if let Ok(nxt) = self.post_ytm("next", json!({ "videoId": track.id }))
            && let Some(browse_id) = find_first_string(&nxt, |s| s.starts_with("MPLYt_"))
            && let Ok(lyr) = self.post_ytm("browse", json!({ "browseId": browse_id }))
        {
            let timed = parse_ytm_timed_lyrics(&lyr);
            if !timed.is_empty() {
                out.synced = timed;
                return out;
            }
            out.plain = parse_ytm_plain_lyrics(&lyr);
        }

        // 2. Fallback/supplement: fetch YouTube captionTracks (prefer IOS, then ANDROID)
        for idx in [1usize, 0] {
            let Some(profile) = STREAM_CLIENTS.get(idx) else {
                continue;
            };
            if let Ok(pl) = self.post_player(&track.id, profile)
                && let Some(tracks) = pl
                    .get("captions")
                    .and_then(|c| c.get("playerCaptionsTracklistRenderer"))
                    .and_then(|r| r.get("captionTracks"))
                    .and_then(Value::as_array)
                && let Some((base_url, is_asr)) = pick_caption_track(tracks)
            {
                let json3_url = if base_url.contains("fmt=") {
                    base_url
                } else if base_url.contains('?') {
                    format!("{base_url}&fmt=json3")
                } else {
                    format!("{base_url}?fmt=json3")
                };
                if let Ok(txt) = net::get_text(&json3_url, &[("User-Agent", profile.ua)]) {
                    let parsed = parse_json3_captions(&txt);
                    if !parsed.is_empty() && (!is_asr || out.plain.is_empty()) {
                        out.synced = parsed;
                        break;
                    }
                }
            }
        }

        out
    }

    fn set_liked(&self, track: &Track, liked: bool) -> Result<(), String> {
        if self.has_auth() {
            let ep = if liked {
                "like/like"
            } else {
                "like/removelike"
            };
            self.post_ytm(ep, json!({ "target": { "videoId": track.id } }))?;
            return Ok(());
        }
        let mut list = self.read_likes();
        list.retain(|t| t.id != track.id);
        if liked {
            list.insert(0, track.clone());
        }
        self.write_likes(&list)
    }

    fn load_audio(&self, track: &Track) -> Result<PlaybackSource, String> {
        let mut last_err = String::from("No playable YouTube audio stream found");
        for profile in STREAM_CLIENTS {
            let val = match self.post_player(&track.id, profile) {
                Ok(v) => v,
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            let status = val
                .get("playabilityStatus")
                .and_then(|p| p.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "OK" {
                let reason = val
                    .get("playabilityStatus")
                    .and_then(|p| p.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or(status);
                if !reason.is_empty() {
                    last_err = format!("YouTube ({}): {reason}", profile.name);
                }
                continue;
            }
            let candidates = collect_mp4_audio_urls(&val);
            for url in candidates {
                match net::send(
                    "GET",
                    &url,
                    &[("User-Agent", profile.ua)],
                    None,
                    MAX_DOWNLOAD_BYTES,
                ) {
                    Ok(rep) if (200..300).contains(&rep.status) && !rep.body.is_empty() => {
                        let fmt = AudioFormat::detect(&rep.body).unwrap_or(AudioFormat::Aac);
                        return Ok(PlaybackSource::Bytes {
                            format: fmt,
                            data: rep.body,
                        });
                    }
                    Ok(rep) => {
                        last_err = format!("YouTube stream HTTP {}", rep.status);
                    }
                    Err(e) => {
                        last_err = e;
                    }
                }
            }
        }
        Err(last_err)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum YtLinkTarget {
    Video(String),
    Playlist(String),
}

fn extract_video_or_list(input: &str) -> Option<YtLinkTarget> {
    let s = input.trim();
    if !(s.contains("youtube.com") || s.contains("youtu.be") || s.contains("music.youtube.com")) {
        return None;
    }
    let full = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    let url = url::Url::parse(&full).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if host == "youtu.be" {
        let id = url.path().trim_matches('/').split('/').next()?;
        if is_valid_yt_id(id) {
            return Some(YtLinkTarget::Video(id.to_owned()));
        }
    }
    for (k, v) in url.query_pairs() {
        if k == "v" && is_valid_yt_id(&v) {
            return Some(YtLinkTarget::Video(v.into_owned()));
        }
    }
    if let Some(rest) = url.path().strip_prefix("/shorts/") {
        let id = rest.split('/').next().unwrap_or("");
        if is_valid_yt_id(id) {
            return Some(YtLinkTarget::Video(id.to_owned()));
        }
    }
    for (k, v) in url.query_pairs() {
        if k == "list" && !v.is_empty() {
            return Some(YtLinkTarget::Playlist(v.into_owned()));
        }
    }
    None
}

fn is_valid_yt_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn collect_mp4_audio_urls(player: &Value) -> Vec<String> {
    let mut ranked: Vec<(i64, String)> = Vec::new();
    let sd = player.get("streamingData");
    for key in ["formats", "adaptiveFormats"] {
        if let Some(arr) = sd.and_then(|s| s.get(key)).and_then(Value::as_array) {
            for f in arr {
                let url = json_str(f, "url");
                if url.is_empty() {
                    continue;
                }
                let mime = json_str(f, "mimeType").to_ascii_lowercase();
                let is_audio_mp4 = mime.starts_with("audio/mp4");
                let is_muxed_mp4_aac =
                    key == "formats" && mime.starts_with("video/mp4") && mime.contains("mp4a");
                if !is_audio_mp4 && !is_muxed_mp4_aac {
                    continue;
                }
                let len = json_u64(f, "contentLength");
                if len > MAX_DOWNLOAD_BYTES as u64 {
                    continue;
                }
                let itag = json_u64(f, "itag");
                let bitrate = json_u64(f, "bitrate") as i64;
                // Muxed `itag 18` downloads reliably end-to-end (200 OK) with ANDROID + visitorData;
                // Mp4Audio extracts its `soun` AAC track cleanly.
                let bonus = match itag {
                    18 => 20_000_000,
                    140 => 10_000_000,
                    141 => 8_000_000,
                    139 => 4_000_000,
                    _ if is_muxed_mp4_aac => 12_000_000,
                    _ => 0,
                };
                ranked.push((bonus + bitrate, url.to_owned()));
            }
        }
    }
    ranked.sort_by_key(|b| std::cmp::Reverse(b.0));
    ranked.into_iter().map(|(_, u)| u).collect()
}

enum ParsedYtmItem {
    Track(Track),
    Entity(MusicEntity),
}

fn parse_ytm_item(it: &Value) -> Option<ParsedYtmItem> {
    let cols = it.get("flexColumns").and_then(Value::as_array)?;
    let col_runs = |idx: usize| -> Vec<&Value> {
        cols.get(idx)
            .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
            .and_then(|r| r.get("text"))
            .and_then(|t| t.get("runs"))
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    };

    let runs0 = col_runs(0);
    let raw_title: String = runs0.iter().map(|r| json_str(r, "text")).collect();
    let raw_title = raw_title.trim();
    if raw_title.is_empty() {
        return None;
    }

    let thumb = it
        .get("thumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|r| r.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(upscale_yt_thumb);

    // Track / Video item when videoId exists
    let vid = it
        .get("playlistItemData")
        .and_then(|p| p.get("videoId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            it.get("overlay")
                .and_then(|o| o.get("musicItemThumbnailOverlayRenderer"))
                .and_then(|r| r.get("content"))
                .and_then(|c| c.get("musicPlayButtonRenderer"))
                .and_then(|b| b.get("playNavigationEndpoint"))
                .and_then(|e| e.get("watchEndpoint"))
                .and_then(|w| w.get("videoId"))
                .and_then(Value::as_str)
        })
        .unwrap_or("");

    let runs1 = col_runs(1);
    let mut parts1: Vec<String> = Vec::new();
    let mut artist_parts: Vec<String> = Vec::new();
    let mut album_name = String::new();
    let mut dur_ms = 0u64;

    for r in runs1 {
        let t = json_str(r, "text").trim();
        if t.is_empty() || t == "•" || t == "," || t == "&" {
            continue;
        }
        if let Some(ms) = parse_mm_ss(t) {
            dur_ms = ms;
            continue;
        }
        let bid = r
            .get("navigationEndpoint")
            .and_then(|n| n.get("browseEndpoint"))
            .and_then(|b| b.get("browseId"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if bid.starts_with("UC") {
            artist_parts.push(t.to_owned());
        } else if bid.starts_with("MPREb_") || bid.starts_with("OLAK") {
            if album_name.is_empty() {
                album_name = t.to_owned();
            }
        } else if !is_ytm_kind_label(t) && !looks_like_view_count(t) {
            parts1.push(t.to_owned());
        }
    }

    if dur_ms == 0
        && let Some(fixed) = it.get("fixedColumns").and_then(Value::as_array)
    {
        for fc in fixed {
            if let Some(runs) = fc
                .get("musicResponsiveListItemFixedColumnRenderer")
                .and_then(|r| r.get("text"))
                .and_then(|t| t.get("runs"))
                .and_then(Value::as_array)
            {
                let s: String = runs.iter().map(|r| json_str(r, "text")).collect();
                if let Some(ms) = parse_mm_ss(&s) {
                    dur_ms = ms;
                    break;
                }
            }
        }
    }

    if !vid.is_empty() {
        let uploader = if !artist_parts.is_empty() {
            artist_parts.join(", ")
        } else {
            parts1.into_iter().next().unwrap_or_default()
        };
        let (artist, title) = if uploader.is_empty() {
            split_track_name(raw_title, "")
        } else {
            split_track_name(raw_title, &uploader)
        };
        return Some(ParsedYtmItem::Track(Track {
            id: vid.to_owned(),
            title,
            artist,
            album: album_name,
            duration_ms: dur_ms,
            cover_url: thumb
                .or_else(|| Some(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"))),
            provider: Platform::YouTube.id().to_owned(),
        }));
    }

    // Entity (Artist, Album, Playlist)
    let browse_id = it
        .get("navigationEndpoint")
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if browse_id.is_empty() {
        return None;
    }
    let kind = entity_kind_from_browse_id(browse_id)?;
    let subtitle = col_runs(1)
        .into_iter()
        .map(|r| json_str(r, "text"))
        .collect::<String>()
        .trim()
        .to_owned();
    Some(ParsedYtmItem::Entity(MusicEntity {
        kind,
        id: browse_id.to_owned(),
        title: raw_title.to_owned(),
        subtitle,
        cover_url: thumb,
    }))
}

fn parse_ytm_two_row(it: &Value) -> Option<ParsedYtmItem> {
    let title: String = it
        .get("title")
        .and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|r| json_str(r, "text")).collect())
        .unwrap_or_default();
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    let subtitle: String = it
        .get("subtitle")
        .and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|r| json_str(r, "text")).collect())
        .unwrap_or_default();
    let thumb = it
        .get("thumbnailRenderer")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|r| r.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(upscale_yt_thumb);

    let nav = it.get("navigationEndpoint")?;
    if let Some(vid) = nav
        .get("watchEndpoint")
        .and_then(|w| w.get("videoId"))
        .and_then(Value::as_str)
        && !vid.is_empty()
    {
        let (artist, clean) = split_track_name(title, subtitle.trim());
        return Some(ParsedYtmItem::Track(Track {
            id: vid.to_owned(),
            title: clean,
            artist,
            album: String::new(),
            duration_ms: 0,
            cover_url: thumb
                .or_else(|| Some(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"))),
            provider: Platform::YouTube.id().to_owned(),
        }));
    }
    let bid = nav
        .get("browseEndpoint")
        .and_then(|b| b.get("browseId"))
        .and_then(Value::as_str)?;
    let kind = entity_kind_from_browse_id(bid)?;
    Some(ParsedYtmItem::Entity(MusicEntity {
        kind,
        id: bid.to_owned(),
        title: title.to_owned(),
        subtitle: subtitle.trim().to_owned(),
        cover_url: thumb,
    }))
}

fn parse_yt_video_renderer(vr: &Value) -> Option<Track> {
    let vid = json_str(vr, "videoId");
    if vid.is_empty() {
        return None;
    }
    let raw_title: String = vr
        .get("title")
        .and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|r| json_str(r, "text")).collect())
        .unwrap_or_default();
    if raw_title.trim().is_empty() {
        return None;
    }
    let owner: String = vr
        .get("ownerText")
        .and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|r| json_str(r, "text")).collect())
        .unwrap_or_default();
    let len_txt = vr
        .get("lengthText")
        .and_then(|l| l.get("simpleText"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let dur_ms = parse_mm_ss(len_txt).unwrap_or(0);
    // Skip livestreams (0 duration without lengthText) or ultra-long (>30 min) non-track podcasts
    if dur_ms == 0 || dur_ms > 30 * 60_000 {
        return None;
    }
    let (artist, title) = split_track_name(raw_title.trim(), owner.trim());
    let cover = vr
        .get("thumbnail")
        .and_then(|t| t.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(upscale_yt_thumb)
        .or_else(|| Some(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg")));
    Some(Track {
        id: vid.to_owned(),
        title,
        artist,
        album: String::new(),
        duration_ms: dur_ms,
        cover_url: cover,
        provider: Platform::YouTube.id().to_owned(),
    })
}

fn parse_ytm_timed_lyrics(val: &Value) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for item in collect_key(val, "timedLyricsData") {
        let Some(arr) = item.as_array() else {
            continue;
        };
        for entry in arr {
            let line = json_str(entry, "lyricLine").trim();
            let start = entry
                .get("cueRange")
                .and_then(|c| c.get("startTimeMilliseconds"))
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| {
                    entry
                        .get("cueRange")
                        .and_then(|c| c.get("startTimeMilliseconds"))
                        .and_then(Value::as_u64)
                });
            if !line.is_empty()
                && let Some(ms) = start
            {
                out.push(LyricLine::new(ms, line));
            }
        }
    }
    out.sort_by_key(|l| l.time_ms);
    out
}

fn parse_ytm_plain_lyrics(val: &Value) -> Vec<String> {
    let mut best = String::new();
    for shelf in collect_key(val, "musicDescriptionShelfRenderer") {
        if let Some(runs) = shelf
            .get("description")
            .and_then(|d| d.get("runs"))
            .and_then(Value::as_array)
        {
            let text: String = runs.iter().map(|r| json_str(r, "text")).collect();
            if text.len() > best.len() {
                best = text;
            }
        }
    }
    if best.trim().is_empty() {
        return Vec::new();
    }
    best.lines()
        .map(|l| l.trim().to_owned())
        .collect::<Vec<_>>()
}

fn pick_caption_track(tracks: &[Value]) -> Option<(String, bool)> {
    let mut best: Option<(i32, String, bool)> = None;
    for tr in tracks {
        let url = json_str(tr, "baseUrl");
        if url.is_empty() {
            continue;
        }
        let is_asr = json_str(tr, "kind") == "asr";
        let lang = json_str(tr, "languageCode").to_ascii_lowercase();
        let mut score = if is_asr { 0 } else { 100 };
        if lang.starts_with("ru") {
            score += 30;
        } else if lang.starts_with("en") {
            score += 20;
        }
        if best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, url.to_owned(), is_asr));
        }
    }
    best.map(|(_, u, a)| (u, a))
}

fn parse_json3_captions(body: &str) -> Vec<LyricLine> {
    let Ok(val) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(events) = val.get("events").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in events {
        let Some(t_start) = ev.get("tStartMs").and_then(Value::as_u64) else {
            continue;
        };
        let dur_ms = ev
            .get("dDurationMs")
            .and_then(Value::as_u64)
            .unwrap_or(2500);
        let Some(segs) = ev.get("segs").and_then(Value::as_array) else {
            continue;
        };
        let mut full = String::new();
        let mut word_pairs: Vec<(u64, String)> = Vec::new();
        for seg in segs {
            let txt = json_str(seg, "utf8");
            if txt.trim().is_empty() {
                if !full.ends_with(' ') && !full.is_empty() {
                    full.push(' ');
                }
                continue;
            }
            let off = seg.get("tOffsetMs").and_then(Value::as_u64).unwrap_or(0);
            let cleaned = txt.replace('\n', " ");
            for tok in cleaned.split_whitespace() {
                word_pairs.push((t_start + off, tok.to_owned()));
            }
            full.push_str(&cleaned);
        }
        let line_text = full.split_whitespace().collect::<Vec<_>>().join(" ");
        if line_text.is_empty() || is_caption_noise(&line_text) {
            continue;
        }
        let mut words = Vec::new();
        if word_pairs.len() > 1 {
            for (i, (ws, wt)) in word_pairs.iter().enumerate() {
                let we = word_pairs
                    .get(i + 1)
                    .map(|(nw, _)| *nw)
                    .unwrap_or_else(|| t_start.saturating_add(dur_ms))
                    .max(ws + 100);
                words.push(WordTiming {
                    time_ms: *ws,
                    end_ms: Some(we),
                    text: wt.clone(),
                });
            }
        }
        out.push(LyricLine::with_words(t_start, line_text, words));
    }
    out
}

fn is_caption_noise(s: &str) -> bool {
    let t = s.trim();
    if t.starts_with('[') && t.ends_with(']') {
        return true;
    }
    t.chars().all(|c| !c.is_alphanumeric())
}

fn entity_kind_from_browse_id(id: &str) -> Option<EntityKind> {
    if id.starts_with("UC") {
        Some(EntityKind::Artist)
    } else if id.starts_with("MPREb_") || id.starts_with("OLAK") {
        Some(EntityKind::Album)
    } else if id.starts_with("VL") || id.starts_with("PL") || id.starts_with("RDCLAK") {
        Some(EntityKind::Playlist)
    } else {
        None
    }
}

fn is_ytm_kind_label(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "song"
            | "video"
            | "album"
            | "artist"
            | "playlist"
            | "single"
            | "ep"
            | "композиция"
            | "песня"
            | "видео"
            | "альбом"
            | "сингл"
            | "исполнитель"
            | "плейлист"
    )
}

fn looks_like_view_count(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    low.contains("просмотр")
        || low.contains("прослуш")
        || low.contains("views")
        || low.contains("plays")
        || low.contains("слушател")
}

fn parse_mm_ss(s: &str) -> Option<u64> {
    let t = s.trim();
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() == 2 {
        let m = parts[0].parse::<u64>().ok()?;
        let sec = parts[1].parse::<u64>().ok()?;
        if sec < 60 {
            return Some((m * 60 + sec) * 1000);
        }
    } else if parts.len() == 3 {
        let h = parts[0].parse::<u64>().ok()?;
        let m = parts[1].parse::<u64>().ok()?;
        let sec = parts[2].parse::<u64>().ok()?;
        if m < 60 && sec < 60 {
            return Some((h * 3600 + m * 60 + sec) * 1000);
        }
    }
    None
}

fn upscale_yt_thumb(raw: &str) -> String {
    let u = raw.trim();
    if let Some(pos) = u.find("=w")
        && (u.contains("googleusercontent.com") || u.contains("ggpht.com"))
    {
        return format!("{}=w544-h544-l90-rj", &u[..pos]);
    }
    u.to_owned()
}

fn collect_key<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    fn walk<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    if k == key {
                        out.push(child);
                    }
                    walk(child, key, out);
                }
            }
            Value::Array(arr) => {
                for child in arr {
                    walk(child, key, out);
                }
            }
            _ => {}
        }
    }
    walk(v, key, &mut out);
    out
}

fn find_first_string<F>(v: &Value, pred: F) -> Option<String>
where
    F: Fn(&str) -> bool + Copy,
{
    match v {
        Value::String(s) if pred(s) => Some(s.clone()),
        Value::Object(map) => map.values().find_map(|c| find_first_string(c, pred)),
        Value::Array(arr) => arr.iter().find_map(|c| find_first_string(c, pred)),
        _ => None,
    }
}

fn normalize_cookie_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Support full Request Headers dump or curl snippet containing `cookie: ...`
    for line in trimmed.lines() {
        let l = line.trim();
        if l.len() > 7 && l[..7].eq_ignore_ascii_case("cookie:") {
            return l[7..]
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_owned();
        }
    }
    for prefix in [
        "-H 'cookie:",
        "-H \"cookie:",
        "-H 'Cookie:",
        "-H \"Cookie:",
        "-b '",
        "-b \"",
    ] {
        if let Some(pos) = trimmed.find(prefix) {
            let rest = &trimmed[pos + prefix.len()..];
            let quote = if prefix.ends_with('\'') { '\'' } else { '"' };
            if let Some(end) = rest.find(quote) {
                return rest[..end].trim().to_owned();
            }
        }
    }
    trimmed
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn extract_sapisid(cookie: &str) -> Option<String> {
    for target in ["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"] {
        for part in cookie.split(';') {
            let p = part.trim();
            if let Some((k, v)) = p.split_once('=')
                && k.trim() == target
            {
                let val = v.trim();
                if !val.is_empty() {
                    return Some(val.to_owned());
                }
            }
        }
    }
    None
}

fn sapisid_hash(sapisid: &str, origin: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let input = format!("{ts} {sapisid} {origin}");
    format!("SAPISIDHASH {ts}_{}", sha1_hex(input.as_bytes()))
}

fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    format!(
        "{:08x}{:08x}{:08x}{:08x}{:08x}",
        h[0], h[1], h[2], h[3], h[4]
    )
}

fn extract_ytm_account_name(menu: &Value) -> String {
    for renderer in collect_key(menu, "activeAccountHeaderRenderer") {
        let name = renderer
            .get("accountName")
            .and_then(|v| v.get("runs"))
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !name.is_empty() {
            return name.to_owned();
        }
        let handle = renderer
            .get("channelHandle")
            .and_then(|v| v.get("runs"))
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !handle.is_empty() {
            return handle.to_owned();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_links_durations_and_captions() {
        assert_eq!(
            extract_video_or_list("https://www.youtube.com/watch?v=R9k3P-f4qxU&t=10"),
            Some(YtLinkTarget::Video("R9k3P-f4qxU".to_owned()))
        );
        assert_eq!(
            extract_video_or_list("https://youtu.be/dQw4w9WgXcQ"),
            Some(YtLinkTarget::Video("dQw4w9WgXcQ".to_owned()))
        );
        assert_eq!(
            extract_video_or_list("https://music.youtube.com/playlist?list=PL4fGSI1pDJn6"),
            Some(YtLinkTarget::Playlist("PL4fGSI1pDJn6".to_owned()))
        );
        assert_eq!(parse_mm_ss("1:42"), Some(102_000));
        assert_eq!(parse_mm_ss("bad"), None);

        let j3 = r#"{
          "events": [
            {"tStartMs": 1000, "dDurationMs": 1500, "segs": [{"utf8": "[♪♪♪]"}]},
            {"tStartMs": 3200, "dDurationMs": 2200, "segs": [
              {"utf8": "Hello", "tOffsetMs": 0},
              {"utf8": " world", "tOffsetMs": 800}
            ]}
          ]
        }"#;
        let caps = parse_json3_captions(j3);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].time_ms, 3200);
        assert_eq!(caps[0].text, "Hello world");
        assert_eq!(caps[0].words.len(), 2);
        assert_eq!(caps[0].words[1].time_ms, 4000);

        let player = json!({
            "streamingData": {
                "formats": [
                    {
                        "itag": 18,
                        "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"",
                        "bitrate": 500000,
                        "url": "https://rr1.googlevideo.com/videoplayback?itag=18"
                    }
                ],
                "adaptiveFormats": [
                    {
                        "itag": 140,
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "bitrate": 128000,
                        "url": "https://rr1.googlevideo.com/videoplayback?itag=140"
                    }
                ]
            }
        });
        let urls = collect_mp4_audio_urls(&player);
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("itag=18"));
        assert!(urls[1].contains("itag=140"));

        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let parsed_cookie =
            normalize_cookie_input("Accept: */*\nCookie: VISITOR=1; SAPISID=xyz123; SID=abc");
        assert_eq!(parsed_cookie, "VISITOR=1; SAPISID=xyz123; SID=abc");
        assert_eq!(extract_sapisid(&parsed_cookie).as_deref(), Some("xyz123"));
        assert!(sapisid_hash("xyz123", "https://music.youtube.com").starts_with("SAPISIDHASH "));
        assert!(YouTubeProvider::ytm_context()["client"].get("gl").is_none());
    }
}
