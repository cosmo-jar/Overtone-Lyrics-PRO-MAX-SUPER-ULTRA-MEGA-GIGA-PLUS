use std::fs;
use std::time::SystemTime;

use base64::Engine;
use serde_json::Value;

use crate::config::{atomic_write_bytes, data_dir};
use crate::lyrics::match_track::{
    MatchLevel, clean_title, duration_diff_secs, duration_fits, first_artist, match_level, norm,
    queries, script_ok, tempo_tag,
};
use crate::lyrics::parsers::{
    decode_krc, parse_lrc, parse_netease_yrc, parse_plain_lines, parse_ttml,
};
use crate::model::{LyricLine, Track};
use crate::net::{build_query_url, get_json, get_text, url_encode};

const AMLL_INDEX_URL: &str =
    "https://raw.githubusercontent.com/Steve-xmh/amll-ttml-db/main/metadata/raw-lyrics-index.jsonl";
const AMLL_RAW_BASE: &str =
    "https://raw.githubusercontent.com/Steve-xmh/amll-ttml-db/main/raw-lyrics/";
const AMLL_CACHE_TTL_SEC: u64 = 72 * 3600;

#[derive(Clone, Debug, Default)]
pub struct SourceFetch {
    pub synced: Vec<LyricLine>,
    pub plain: Vec<String>,
}

pub fn fetch_amll(t: &Track) -> SourceFetch {
    let Ok(index_txt) = load_amll_index() else {
        return SourceFetch::default();
    };
    let mut best_file: Option<String> = None;
    let mut best_score = 0i32;

    for raw_line in index_txt.lines() {
        let Ok(v) = serde_json::from_str::<Value>(raw_line) else {
            continue;
        };
        let Some(fname) = v.get("rawLyricFile").and_then(Value::as_str) else {
            continue;
        };
        let meta = v.get("metadata").and_then(Value::as_array);
        let Some(meta) = meta else { continue };

        let mut titles: Vec<&str> = Vec::new();
        let mut artists: Vec<&str> = Vec::new();
        for entry in meta {
            if let Some(arr) = entry.as_array()
                && arr.len() >= 2
                && let (Some(k), Some(vals)) = (arr[0].as_str(), arr[1].as_array())
            {
                if k == "musicName" {
                    for s in vals.iter().filter_map(Value::as_str) {
                        titles.push(s);
                    }
                } else if k == "artists" {
                    for s in vals.iter().filter_map(Value::as_str) {
                        artists.push(s);
                    }
                }
            }
        }
        let artist_joined = artists.join(", ");
        for ft in &titles {
            let lvl = match_level(ft, &artist_joined, t);
            let score = match lvl {
                MatchLevel::Full => 100,
                MatchLevel::Loose => 40,
                MatchLevel::None => 0,
            };
            if score > best_score {
                best_score = score;
                best_file = Some(fname.to_owned());
            }
        }
    }

    let Some(file) = best_file else {
        return SourceFetch::default();
    };
    let url = format!("{AMLL_RAW_BASE}{}", url_encode(&file).replace("%2F", "/"));
    let Ok(ttml) = get_text(&url, &[]) else {
        return SourceFetch::default();
    };
    let lines = parse_ttml(&ttml);
    if !lines.is_empty() && script_ok(&t.title, &lines) {
        SourceFetch {
            synced: lines,
            plain: Vec::new(),
        }
    } else {
        SourceFetch::default()
    }
}

fn load_amll_index() -> Result<String, String> {
    let cache_path = data_dir().join("lyrics_cache").join("amll_index.jsonl");
    if let Ok(meta) = fs::metadata(&cache_path)
        && let Ok(mod_t) = meta.modified()
        && let Ok(age) = SystemTime::now().duration_since(mod_t)
        && age.as_secs() < AMLL_CACHE_TTL_SEC
        && let Ok(txt) = fs::read_to_string(&cache_path)
    {
        return Ok(txt);
    }
    let body = get_text(AMLL_INDEX_URL, &[])?;
    let _ = atomic_write_bytes(&cache_path, body.as_bytes());
    Ok(body)
}

pub fn fetch_lrclib(t: &Track) -> SourceFetch {
    // Stage 1: /api/get exact lookup when metadata + duration look clean
    if !t.artist.trim().is_empty() && t.duration_ms > 0 {
        let dur_s = (t.duration_ms / 1000).to_string();
        let mut params = vec![
            ("artist_name", t.artist.as_str()),
            ("track_name", t.title.as_str()),
            ("duration", dur_s.as_str()),
        ];
        if !t.album.trim().is_empty() {
            params.push(("album_name", t.album.as_str()));
        }
        let url = build_query_url("https://lrclib.net/api/get", &params);
        if let Ok(v) = get_json(&url, &[]) {
            let fetch = extract_lrclib_item(&v, t, false);
            if !fetch.synced.is_empty() {
                return fetch;
            }
        }
    }

    // Stage 2: /api/search variants including cleaned titles and Artist - Title split
    let mut best_plain: Vec<String> = Vec::new();
    let track_tempo = tempo_tag(&t.title);

    for q in queries(t) {
        let mut search_urls = Vec::new();
        if !q.artist.trim().is_empty() {
            search_urls.push(build_query_url(
                "https://lrclib.net/api/search",
                &[
                    ("track_name", q.title.as_str()),
                    ("artist_name", q.artist.as_str()),
                ],
            ));
            let clean_t = clean_title(&q.title);
            let first_a = first_artist(&q.artist);
            if clean_t != q.title || first_a != q.artist {
                search_urls.push(build_query_url(
                    "https://lrclib.net/api/search",
                    &[
                        ("track_name", clean_t.as_str()),
                        ("artist_name", first_a.as_str()),
                    ],
                ));
            }
        }
        let query_combined = if q.artist.trim().is_empty() {
            q.title.clone()
        } else {
            format!("{} {}", q.artist, q.title)
        };
        search_urls.push(build_query_url(
            "https://lrclib.net/api/search",
            &[("q", query_combined.as_str())],
        ));

        for url in search_urls {
            let Ok(Value::Array(items)) = get_json(&url, &[]) else {
                continue;
            };
            let mut best_synced: Option<(MatchLevel, f64, Vec<LyricLine>)> = None;
            for item in items {
                let ft = item.get("trackName").and_then(Value::as_str).unwrap_or("");
                let fa = item.get("artistName").and_then(Value::as_str).unwrap_or("");
                let found_dur = item.get("duration").and_then(Value::as_f64).unwrap_or(0.0);

                let lvl = match_level(ft, fa, t);
                if lvl == MatchLevel::None {
                    continue;
                }
                let allow_tempo_scale = track_tempo != 0 && tempo_tag(ft) == 0;
                let diff = duration_diff_secs(found_dur, t);
                let fits = if allow_tempo_scale {
                    let ratio = if found_dur > 0.0 && t.duration_ms > 0 {
                        (t.duration_ms as f64 / 1000.0) / found_dur
                    } else {
                        1.0
                    };
                    (0.65..=1.55).contains(&ratio)
                } else {
                    duration_fits(diff, 5.0, lvl)
                };
                if !fits {
                    continue;
                }

                let cand = extract_lrclib_item(&item, t, allow_tempo_scale);
                if !cand.synced.is_empty() {
                    let d = diff.unwrap_or(1.0);
                    let replace = match &best_synced {
                        None => true,
                        Some((bl, bd, _)) => lvl > *bl || (lvl == *bl && d < *bd),
                    };
                    if replace {
                        best_synced = Some((lvl, d, cand.synced));
                    }
                } else if best_plain.is_empty() && !cand.plain.is_empty() {
                    best_plain = cand.plain;
                }
            }
            if let Some((_, _, synced)) = best_synced {
                return SourceFetch {
                    synced,
                    plain: best_plain,
                };
            }
        }
    }

    SourceFetch {
        synced: Vec::new(),
        plain: best_plain,
    }
}

fn extract_lrclib_item(v: &Value, t: &Track, scale_tempo: bool) -> SourceFetch {
    if v.get("instrumental").and_then(Value::as_bool) == Some(true) {
        return SourceFetch::default();
    }
    let found_dur = v.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(synced_raw) = v.get("syncedLyrics").and_then(Value::as_str) {
        let mut lines = parse_lrc(synced_raw);
        if scale_tempo && found_dur > 0.0 && t.duration_ms > 0 {
            let ratio = (t.duration_ms as f64 / 1000.0) / found_dur;
            for l in &mut lines {
                l.time_ms = ((l.time_ms as f64) * ratio).round() as u64;
                for w in &mut l.words {
                    w.time_ms = ((w.time_ms as f64) * ratio).round() as u64;
                    w.end_ms = w.end_ms.map(|e| ((e as f64) * ratio).round() as u64);
                }
            }
        }
        if !lines.is_empty() && script_ok(&t.title, &lines) {
            return SourceFetch {
                synced: lines,
                plain: Vec::new(),
            };
        }
    }
    if let Some(plain_raw) = v.get("plainLyrics").and_then(Value::as_str) {
        let plain = parse_plain_lines(plain_raw);
        let dummy: Vec<LyricLine> = plain.iter().map(|s| LyricLine::new(0, s)).collect();
        if !plain.is_empty() && script_ok(&t.title, &dummy) {
            return SourceFetch {
                synced: Vec::new(),
                plain,
            };
        }
    }
    SourceFetch::default()
}

pub fn fetch_netease(t: &Track) -> SourceFetch {
    let mut best_plain: Vec<String> = Vec::new();
    for q in queries(t) {
        let kw = if q.artist.trim().is_empty() {
            q.title.clone()
        } else {
            format!("{} {}", first_artist(&q.artist), clean_title(&q.title))
        };
        let search_url = build_query_url(
            "https://music.163.com/api/search/get/web",
            &[
                ("s", kw.as_str()),
                ("type", "1"),
                ("offset", "0"),
                ("limit", "8"),
            ],
        );
        let Ok(v) = get_json(&search_url, &[("Referer", "https://music.163.com/")]) else {
            continue;
        };
        let Some(songs) = v
            .get("result")
            .and_then(|r| r.get("songs"))
            .and_then(Value::as_array)
        else {
            continue;
        };

        for song in songs {
            let Some(id) = song.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let ft = song.get("name").and_then(Value::as_str).unwrap_or("");
            let fa = song
                .get("artists")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let dur_s = song.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
            let lvl = match_level(ft, &fa, t);
            if lvl == MatchLevel::None || !duration_fits(duration_diff_secs(dur_s, t), 5.0, lvl) {
                continue;
            }
            let lyric_url =
                format!("https://music.163.com/api/song/lyric?os=pc&id={id}&lv=-1&tv=-1&yv=-1");
            let Ok(lv) = get_json(&lyric_url, &[("Referer", "https://music.163.com/")]) else {
                continue;
            };
            if let Some(yrc) = lv
                .get("yrc")
                .and_then(|y| y.get("lyric"))
                .and_then(Value::as_str)
            {
                let lines = parse_netease_yrc(yrc);
                if !lines.is_empty() && script_ok(&t.title, &lines) {
                    return SourceFetch {
                        synced: lines,
                        plain: Vec::new(),
                    };
                }
            }
            if let Some(lrc) = lv
                .get("lrc")
                .and_then(|y| y.get("lyric"))
                .and_then(Value::as_str)
            {
                let lines = parse_lrc(lrc);
                if !lines.is_empty() && script_ok(&t.title, &lines) {
                    return SourceFetch {
                        synced: lines,
                        plain: Vec::new(),
                    };
                }
                if best_plain.is_empty() {
                    let plain = parse_plain_lines(lrc);
                    if !plain.is_empty() {
                        best_plain = plain;
                    }
                }
            }
        }
    }
    SourceFetch {
        synced: Vec::new(),
        plain: best_plain,
    }
}

pub fn fetch_qq_music(t: &Track) -> SourceFetch {
    let mut best_plain: Vec<String> = Vec::new();
    for q in queries(t) {
        let kw = if q.artist.trim().is_empty() {
            q.title.clone()
        } else {
            format!("{} {}", first_artist(&q.artist), clean_title(&q.title))
        };
        let search_url = build_query_url(
            "https://c.y.qq.com/soso/fcgi-bin/client_search_cp",
            &[
                ("format", "json"),
                ("n", "8"),
                ("p", "1"),
                ("w", kw.as_str()),
            ],
        );
        let Ok(v) = get_json(&search_url, &[("Referer", "https://y.qq.com/")]) else {
            continue;
        };
        let Some(list) = v
            .get("data")
            .and_then(|d| d.get("song"))
            .and_then(|s| s.get("list"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for item in list {
            let Some(mid) = item.get("songmid").and_then(Value::as_str) else {
                continue;
            };
            let ft = item.get("songname").and_then(Value::as_str).unwrap_or("");
            let fa = item
                .get("singer")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let dur_s = item.get("interval").and_then(Value::as_f64).unwrap_or(0.0);
            let lvl = match_level(ft, &fa, t);
            if lvl == MatchLevel::None || !duration_fits(duration_diff_secs(dur_s, t), 5.0, lvl) {
                continue;
            }
            let lyric_url = build_query_url(
                "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg",
                &[("songmid", mid), ("format", "json"), ("nobase64", "1")],
            );
            let Ok(lv) = get_json(&lyric_url, &[("Referer", "https://y.qq.com/")]) else {
                continue;
            };
            let Some(raw_lrc) = lv.get("lyric").and_then(Value::as_str) else {
                continue;
            };
            let decoded_lrc = if raw_lrc.contains('[') {
                raw_lrc.to_owned()
            } else {
                base64::engine::general_purpose::STANDARD
                    .decode(raw_lrc.as_bytes())
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_else(|| raw_lrc.to_owned())
            };
            let lines = parse_lrc(&decoded_lrc);
            if !lines.is_empty() && script_ok(&t.title, &lines) {
                return SourceFetch {
                    synced: lines,
                    plain: Vec::new(),
                };
            }
            if best_plain.is_empty() {
                let plain = parse_plain_lines(&decoded_lrc);
                if !plain.is_empty() {
                    best_plain = plain;
                }
            }
        }
    }
    SourceFetch {
        synced: Vec::new(),
        plain: best_plain,
    }
}

pub fn fetch_kugou(t: &Track) -> SourceFetch {
    for q in queries(t) {
        let kw = if q.artist.trim().is_empty() {
            clean_title(&q.title)
        } else {
            format!("{} - {}", first_artist(&q.artist), clean_title(&q.title))
        };
        let dur_ms_str = t.duration_ms.to_string();
        let search_url = build_query_url(
            "https://lyrics.kugou.com/search",
            &[
                ("ver", "1"),
                ("man", "yes"),
                ("client", "pc"),
                ("keyword", kw.as_str()),
                ("duration", dur_ms_str.as_str()),
            ],
        );
        let Ok(v) = get_json(&search_url, &[]) else {
            continue;
        };
        let Some(cands) = v.get("candidates").and_then(Value::as_array) else {
            continue;
        };
        for cand in cands.iter().take(6) {
            let song = cand.get("song").and_then(Value::as_str).unwrap_or("");
            let singer = cand.get("singer").and_then(Value::as_str).unwrap_or("");
            let dur_s = cand.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
            let lvl = match_level(song, singer, t);
            if lvl == MatchLevel::None || !duration_fits(duration_diff_secs(dur_s, t), 5.0, lvl) {
                continue;
            }
            let Some(id) = cand.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(key) = cand.get("accesskey").and_then(Value::as_str) else {
                continue;
            };
            for fmt in ["krc", "lrc"] {
                let dl_url = build_query_url(
                    "https://lyrics.kugou.com/download",
                    &[
                        ("ver", "1"),
                        ("client", "pc"),
                        ("id", id),
                        ("accesskey", key),
                        ("fmt", fmt),
                        ("charset", "utf8"),
                    ],
                );
                let Ok(dv) = get_json(&dl_url, &[]) else {
                    continue;
                };
                let Some(b64) = dv.get("content").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())
                else {
                    continue;
                };
                let lines = if fmt == "krc" {
                    decode_krc(&bytes).unwrap_or_default()
                } else {
                    parse_lrc(&String::from_utf8_lossy(&bytes))
                };
                if !lines.is_empty() && script_ok(&t.title, &lines) {
                    return SourceFetch {
                        synced: lines,
                        plain: Vec::new(),
                    };
                }
            }
        }
    }
    SourceFetch::default()
}

pub fn fetch_musixmatch(t: &Track, user_token: &str) -> SourceFetch {
    let tok = user_token.trim();
    if tok.is_empty() {
        return SourceFetch::default();
    }
    let artist = first_artist(&t.artist);
    let title = clean_title(&t.title);
    let dur_s = (t.duration_ms / 1000).to_string();
    let mut params = vec![
        ("format", "json"),
        ("subtitle_format", "lrc"),
        ("q_track", title.as_str()),
        ("q_artist", artist.as_str()),
        ("usertoken", tok),
        ("app_id", "web-desktop-app-v1.0"),
    ];
    if t.duration_ms > 0 {
        params.push(("q_duration", dur_s.as_str()));
    }
    let url = build_query_url(
        "https://apic-desktop.musixmatch.com/ws/1.1/matcher.subtitle.get",
        &params,
    );
    let Ok(v) = get_json(&url, &[]) else {
        return SourceFetch::default();
    };
    let Some(sub_body) = v
        .pointer("/message/body/subtitle/subtitle_body")
        .and_then(Value::as_str)
    else {
        return SourceFetch::default();
    };
    let lines = parse_lrc(sub_body);
    if !lines.is_empty() && script_ok(&t.title, &lines) {
        SourceFetch {
            synced: lines,
            plain: Vec::new(),
        }
    } else {
        SourceFetch::default()
    }
}

pub fn fetch_better_lyrics(t: &Track) -> SourceFetch {
    let artist = first_artist(&t.artist);
    let title = clean_title(&t.title);
    let dur_s = (t.duration_ms / 1000).to_string();
    let url = build_query_url(
        "https://lyrics-api.boidu.dev/getLyrics",
        &[
            ("s", title.as_str()),
            ("a", artist.as_str()),
            ("d", dur_s.as_str()),
        ],
    );
    let Ok(v) = get_json(&url, &[]) else {
        return SourceFetch::default();
    };
    if let Some(ttml) = v.get("ttml").and_then(Value::as_str) {
        let lines = parse_ttml(ttml);
        if !lines.is_empty() && script_ok(&t.title, &lines) {
            return SourceFetch {
                synced: lines,
                plain: Vec::new(),
            };
        }
    }
    if let Some(lrc) = v.get("lrc").and_then(Value::as_str) {
        let lines = parse_lrc(lrc);
        if !lines.is_empty() && script_ok(&t.title, &lines) {
            return SourceFetch {
                synced: lines,
                plain: Vec::new(),
            };
        }
    }
    SourceFetch::default()
}

pub fn fetch_lyrics_ovh_plain(t: &Track) -> Vec<String> {
    let artist = first_artist(&t.artist);
    let title = clean_title(&t.title);
    if artist.is_empty() || title.is_empty() {
        return Vec::new();
    }
    let url = format!(
        "https://api.lyrics.ovh/v1/{}/{}",
        url_encode(&artist),
        url_encode(&title)
    );
    let Ok(v) = get_json(&url, &[]) else {
        return Vec::new();
    };
    let Some(lyrics) = v.get("lyrics").and_then(Value::as_str) else {
        return Vec::new();
    };
    filter_plain_lyric_lines(parse_plain_lines(lyrics), t)
}

const BROWSER_HEADERS: &[(&str, &str)] = &[
    (
        "User-Agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    ),
    (
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("Accept-Language", "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7"),
];

pub fn fetch_genius_plain(t: &Track) -> Vec<String> {
    let artist = first_artist(&t.artist);
    let title = clean_title(&t.title);
    if title.is_empty() {
        return Vec::new();
    }
    let q = if artist.is_empty() {
        title.clone()
    } else {
        format!("{artist} {title}")
    };
    let search_url = build_query_url("https://genius.com/api/search/multi", &[("q", &q)]);
    let Ok(v) = get_json(&search_url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    let mut song_url: Option<String> = None;
    let mut best_lvl = MatchLevel::None;
    if let Some(sections) = v
        .get("response")
        .and_then(|r| r.get("sections"))
        .and_then(Value::as_array)
    {
        for sec in sections {
            let Some(hits) = sec.get("hits").and_then(Value::as_array) else {
                continue;
            };
            for hit in hits {
                let Some(res) = hit.get("result") else {
                    continue;
                };
                let Some(u) = res.get("url").and_then(Value::as_str) else {
                    continue;
                };
                if !u.starts_with("https://genius.com/") {
                    continue;
                }
                let ht = res.get("title").and_then(Value::as_str).unwrap_or("");
                let ha = res
                    .get("primary_artist")
                    .and_then(|a| a.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let lvl = match_level(ht, ha, t);
                let rank = |m: MatchLevel| match m {
                    MatchLevel::Full => 2,
                    MatchLevel::Loose => 1,
                    MatchLevel::None => 0,
                };
                if rank(lvl) > rank(best_lvl) {
                    best_lvl = lvl;
                    song_url = Some(u.to_owned());
                    if lvl == MatchLevel::Full {
                        break;
                    }
                }
            }
        }
    }
    let Some(page_url) = song_url else {
        return Vec::new();
    };
    let Ok(html) = get_text(&page_url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    let lines = extract_genius_containers(&html);
    filter_plain_lyric_lines(lines, t)
}

pub fn fetch_rulyrics_plain(t: &Track) -> Vec<String> {
    let q = plain_search_query(t);
    if q.is_empty() {
        return Vec::new();
    }
    let url = build_query_url("https://rulyrics.ru/", &[("s", &q)]);
    let Ok(html) = get_text(&url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    for page_url in extract_matching_hrefs(&html, "https://rulyrics.ru", t)
        .into_iter()
        .take(2)
    {
        let Ok(page) = get_text(&page_url, BROWSER_HEADERS) else {
            continue;
        };
        if let Some(frag) = extract_nested_tag_by_markers(
            &page,
            "div",
            &[
                "class=\"entry-content",
                "class='entry-content",
                "class=\"lyrics",
                "id=\"lyrics",
            ],
        ) {
            let lines = filter_plain_lyric_lines(html_fragment_to_plain_lines(frag), t);
            if lines.len() >= 4 {
                return lines;
            }
        }
    }
    Vec::new()
}

pub fn fetch_text_pesenok_plain(t: &Track) -> Vec<String> {
    let q = plain_search_query(t);
    if q.is_empty() {
        return Vec::new();
    }
    let url = build_query_url("https://text-pesenok.ru/search/", &[("q", &q)]);
    let Ok(html) = get_text(&url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    for page_url in extract_matching_hrefs(&html, "https://text-pesenok.ru", t)
        .into_iter()
        .take(2)
    {
        let Ok(page) = get_text(&page_url, BROWSER_HEADERS) else {
            continue;
        };
        if let Some(frag) = extract_nested_tag_by_markers(
            &page,
            "div",
            &[
                "class=\"text\"",
                "class=\"text ",
                "id=\"lyrics",
                "class=\"lyrics",
                "class=\"pesnya",
            ],
        ) {
            let lines = filter_plain_lyric_lines(html_fragment_to_plain_lines(frag), t);
            if lines.len() >= 4 {
                return lines;
            }
        }
    }
    Vec::new()
}

pub fn fetch_azlyrics_plain(t: &Track) -> Vec<String> {
    let artist_slug: String = first_artist(&t.artist)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let title_slug: String = clean_title(&t.title)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if !artist_slug.is_empty() && !title_slug.is_empty() {
        let direct = format!("https://www.azlyrics.com/lyrics/{artist_slug}/{title_slug}.html");
        if let Ok(html) = get_text(&direct, BROWSER_HEADERS)
            && let Some(frag) = extract_azlyrics_div(&html)
        {
            let lines = filter_plain_lyric_lines(html_fragment_to_plain_lines(frag), t);
            if lines.len() >= 4 {
                return lines;
            }
        }
    }
    let q = plain_search_query(t);
    if q.is_empty() {
        return Vec::new();
    }
    let search_url = build_query_url("https://search.azlyrics.com/search.php", &[("q", &q)]);
    let Ok(html) = get_text(&search_url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    for page_url in extract_matching_hrefs(&html, "https://www.azlyrics.com", t)
        .into_iter()
        .filter(|u| u.contains("/lyrics/"))
        .take(2)
    {
        if let Ok(page) = get_text(&page_url, BROWSER_HEADERS)
            && let Some(frag) = extract_azlyrics_div(&page)
        {
            let lines = filter_plain_lyric_lines(html_fragment_to_plain_lines(frag), t);
            if lines.len() >= 4 {
                return lines;
            }
        }
    }
    Vec::new()
}

pub fn fetch_lyrics_com_plain(t: &Track) -> Vec<String> {
    let q = plain_search_query(t);
    if q.is_empty() {
        return Vec::new();
    }
    let url = build_query_url("https://www.lyrics.com/serp.php", &[("st", &q)]);
    let Ok(html) = get_text(&url, BROWSER_HEADERS) else {
        return Vec::new();
    };
    for page_url in extract_matching_hrefs(&html, "https://www.lyrics.com", t)
        .into_iter()
        .filter(|u| u.contains("/lyric/"))
        .take(2)
    {
        if let Ok(page) = get_text(&page_url, BROWSER_HEADERS)
            && let Some(frag) =
                extract_nested_tag_by_markers(&page, "pre", &["id=\"lyric-body-text"])
        {
            let lines = filter_plain_lyric_lines(html_fragment_to_plain_lines(frag), t);
            if lines.len() >= 4 {
                return lines;
            }
        }
    }
    Vec::new()
}

/// Runs all external plain-text lyrics catalogs (Genius, ruLyrics, text-pesenok, AZLyrics,
/// Lyrics.com, lyrics.ovh) in parallel and returns the highest-priority valid result.
pub fn fetch_all_plain_cascade(t: &Track) -> Vec<String> {
    let fetchers: [fn(&Track) -> Vec<String>; 6] = [
        fetch_genius_plain,
        fetch_rulyrics_plain,
        fetch_text_pesenok_plain,
        fetch_azlyrics_plain,
        fetch_lyrics_com_plain,
        fetch_lyrics_ovh_plain,
    ];
    let handles: Vec<_> = fetchers
        .into_iter()
        .map(|f| {
            let tc = t.clone();
            std::thread::spawn(move || f(&tc))
        })
        .collect();
    let mut best: Option<Vec<String>> = None;
    for h in handles {
        if let Ok(lines) = h.join()
            && !lines.is_empty()
            && best.is_none()
        {
            best = Some(lines);
        }
    }
    best.unwrap_or_default()
}

fn plain_search_query(t: &Track) -> String {
    let artist = first_artist(&t.artist);
    let title = clean_title(&t.title);
    if title.is_empty() {
        String::new()
    } else if artist.is_empty() {
        title
    } else {
        format!("{artist} {title}")
    }
}

fn filter_plain_lyric_lines(lines: Vec<String>, t: &Track) -> Vec<String> {
    let out: Vec<String> = lines
        .into_iter()
        .filter(|l| {
            let s = l.trim();
            if s.is_empty() {
                return false;
            }
            if s.starts_with('[') && s.ends_with(']') {
                return false;
            }
            let low = s.to_ascii_lowercase();
            !low.starts_with("contributors")
                && !low.starts_with("translations")
                && !low.contains("you might also like")
                && !low.ends_with(" lyrics")
                && !low.starts_with("embed")
        })
        .collect();
    if out.is_empty() || norm(&clean_title(&t.title)).is_empty() {
        return Vec::new();
    }
    let dummy: Vec<LyricLine> = out.iter().map(|s| LyricLine::new(0, s)).collect();
    if script_ok(&t.title, &dummy) {
        out
    } else {
        Vec::new()
    }
}

pub(crate) fn extract_genius_containers(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    let marker = "data-lyrics-container=\"true\"";
    while let Some(pos) = rest.find(marker) {
        let tag_start = rest[..pos].rfind('<').unwrap_or(pos);
        if let Some((inner, consumed)) = slice_balanced_tag(&rest[tag_start..], "div") {
            out.extend(html_fragment_to_plain_lines(inner));
            rest = &rest[tag_start + consumed..];
        } else {
            rest = &rest[pos + marker.len()..];
        }
    }
    out
}

pub(crate) fn extract_azlyrics_div(html: &str) -> Option<&str> {
    let comment_pos = html.find("<!-- Usage of azlyrics.com content")?;
    let before = &html[..comment_pos];
    let div_pos = before.rfind("<div")?;
    slice_balanced_tag(&html[div_pos..], "div").map(|(inner, _)| inner)
}

pub(crate) fn extract_nested_tag_by_markers<'a>(
    html: &'a str,
    tag: &str,
    markers: &[&str],
) -> Option<&'a str> {
    let open_pat = format!("<{tag}");
    let mut off = 0usize;
    while let Some(rel) = html[off..].find(&open_pat) {
        let start = off + rel;
        let tag_end_rel = html[start..].find('>')?;
        let open_slice = &html[start..start + tag_end_rel + 1];
        if markers.iter().any(|m| open_slice.contains(m)) {
            return slice_balanced_tag(&html[start..], tag).map(|(inner, _)| inner);
        }
        off = start + tag_end_rel + 1;
    }
    None
}

fn slice_balanced_tag<'a>(from_open: &'a str, tag: &str) -> Option<(&'a str, usize)> {
    let first_gt = from_open.find('>')?;
    let body_start = first_gt + 1;
    let open_tok = format!("<{tag}");
    let close_tok = format!("</{tag}>");
    let mut depth = 1i32;
    let mut i = body_start;
    while i < from_open.len() {
        let next_open = from_open[i..].find(&open_tok).map(|p| i + p);
        let next_close = from_open[i..].find(&close_tok).map(|p| i + p);
        match (next_open, next_close) {
            (_, None) => return None,
            (Some(o), Some(c)) if o < c => {
                let after = from_open.as_bytes().get(o + open_tok.len()).copied();
                if matches!(after, Some(b' ' | b'>' | b'\t' | b'\r' | b'\n')) {
                    depth += 1;
                }
                i = o + open_tok.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some((&from_open[body_start..c], c + close_tok.len()));
                }
                i = c + close_tok.len();
            }
        }
    }
    None
}

pub(crate) fn extract_matching_hrefs(html: &str, base_url: &str, t: &Track) -> Vec<String> {
    let want_title = norm(&clean_title(&t.title));
    let want_artist = norm(&first_artist(&t.artist));
    if want_title.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i32, String)> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<a ") {
        let after = &rest[pos..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let Some(close_rel) = after[tag_end + 1..].find("</a>") else {
            break;
        };
        let open_tag = &after[..tag_end];
        let anchor_html = &after[tag_end + 1..tag_end + 1 + close_rel];
        let anchor_txt = norm(&html_fragment_to_plain_lines(anchor_html).join(" "));
        if anchor_txt.contains(&want_title)
            && let Some(href) = extract_attr(open_tag, "href")
        {
            let full_url = if href.starts_with("https://") {
                Some(href)
            } else if href.starts_with('/') {
                Some(format!("{base_url}{href}"))
            } else {
                None
            };
            if let Some(u) = full_url {
                let mut score = 10;
                if !want_artist.is_empty() && anchor_txt.contains(&want_artist) {
                    score += 20;
                }
                if !scored.iter().any(|(_, existing)| existing == &u) {
                    scored.push((score, u));
                }
            }
        }
        rest = &after[tag_end + 1 + close_rel + 4..];
    }
    scored.sort_by_key(|b| std::cmp::Reverse(b.0));
    scored.into_iter().map(|(_, u)| u).collect()
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        if let Some(p) = tag.find(&key) {
            let val = &tag[p + key.len()..];
            if let Some(end) = val.find(quote) {
                return Some(val[..end].to_owned());
            }
        }
    }
    None
}

pub(crate) fn html_fragment_to_plain_lines(fragment: &str) -> Vec<String> {
    let mut s = strip_tag_block(fragment, "<script", "</script>");
    s = strip_tag_block(&s, "<style", "</style>");
    s = strip_tag_block(&s, "<!--", "-->");
    for br in [
        "<br>", "<br/>", "<br />", "<BR>", "<BR/>", "</p>", "</div>", "</li>", "</h1>", "</h2>",
        "</h3>",
    ] {
        s = s.replace(br, "\n");
    }
    let mut text = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let decoded = decode_html_entities(&text);
    parse_plain_lines(&decoded)
}

fn strip_tag_block(input: &str, open_pat: &str, close_pat: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open_pat) {
        out.push_str(&rest[..start]);
        let after = &rest[start + open_pat.len()..];
        if let Some(end) = after.find(close_pat) {
            rest = &after[end + close_pat.len()..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn decode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';').filter(|&p| p <= 10) {
            let ent = &after[..semi];
            let repl = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = repl {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(title: &str, artist: &str) -> Track {
        Track {
            id: "1".into(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_ms: 180_000,
            cover_url: None,
            provider: "yandex".into(),
        }
    }

    #[test]
    fn extracts_genius_and_strips_section_tags() {
        let html = r#"
            <div data-lyrics-container="true">[Куплет 1]<br/>Первая строка &amp; свет<br><div>Вторая строка</div></div>
            <div data-lyrics-container="true">[Припев]<br/>Третья &#39;строка&#39;</div>
        "#;
        let raw = extract_genius_containers(html);
        let filtered = filter_plain_lyric_lines(raw, &sample_track("Первая", "Исполнитель"));
        assert_eq!(
            filtered,
            vec!["Первая строка & свет", "Вторая строка", "Третья 'строка'"]
        );
    }

    #[test]
    fn extracts_azlyrics_and_lyrics_com_and_hrefs() {
        let az = r#"<div class="ringtone"><b>"Song"</b></div><div><!-- Usage of azlyrics.com content by any third-party lyrics provider is prohibited -->Line A<br>Line B<br/></div>"#;
        let az_frag = extract_azlyrics_div(az).expect("azlyrics div");
        assert_eq!(
            html_fragment_to_plain_lines(az_frag),
            vec!["Line A", "Line B"]
        );

        let lc = r#"<pre id="lyric-body-text" class="lyric-body">Hello world<br>Second line</pre>"#;
        let lc_frag =
            extract_nested_tag_by_markers(lc, "pre", &["id=\"lyric-body-text"]).expect("pre");
        assert_eq!(
            html_fragment_to_plain_lines(lc_frag),
            vec!["Hello world", "Second line"]
        );

        let search_html =
            r#"<a href="/wrong-song">Other</a><a href="/pesni/test-song">Artist — Test Song</a>"#;
        let hrefs = extract_matching_hrefs(
            search_html,
            "https://rulyrics.ru",
            &sample_track("Test Song", "Artist"),
        );
        assert_eq!(hrefs, vec!["https://rulyrics.ru/pesni/test-song"]);
    }
}
