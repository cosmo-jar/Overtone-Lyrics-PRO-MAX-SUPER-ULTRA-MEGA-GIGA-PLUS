use crate::model::{LyricLine, Track, split_track_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchLevel {
    None,
    Loose,
    Full,
}

#[derive(Clone, Debug)]
pub struct QueryPair {
    pub title: String,
    pub artist: String,
}

pub fn queries(t: &Track) -> Vec<QueryPair> {
    let mut out = vec![QueryPair {
        title: t.title.clone(),
        artist: t.artist.clone(),
    }];
    let (parsed_artist, parsed_title) = split_track_name(&t.title, &t.artist);
    if !parsed_artist.is_empty()
        && !parsed_title.is_empty()
        && (parsed_title != t.title || parsed_artist != t.artist)
    {
        out.push(QueryPair {
            title: parsed_title,
            artist: parsed_artist,
        });
    }
    out
}

/// Internal normalization ONLY (never shown in UI): lowercases, maps Russian 'ё' -> 'е',
/// strips punctuation to single spaces. Display titles like "алёна швец" stay untouched.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        for lc in ch.to_lowercase() {
            let mapped = if lc == 'ё' { 'е' } else { lc };
            if mapped.is_alphanumeric() {
                out.push(mapped);
                last_space = false;
            } else if !last_space {
                out.push(' ');
                last_space = true;
            }
        }
    }
    out.trim().to_owned()
}

pub fn first_artist(artist: &str) -> String {
    let mut best = artist.trim();
    for sep in [",", "&", " feat.", " ft.", " featuring ", " x ", " X "] {
        if let Some(i) = best.to_ascii_lowercase().find(sep.trim_start()) {
            let candidate = best[..i].trim();
            if !candidate.is_empty() {
                best = candidate;
            }
        }
    }
    best.trim().to_owned()
}

pub fn tempo_tag(title: &str) -> i8 {
    let low = norm(title);
    for kw in ["slowed", "daycore", "reverb slowed"] {
        if has_word(&low, kw) {
            return -1;
        }
    }
    for kw in ["sped up", "speed up", "nightcore"] {
        if low.contains(kw) {
            return 1;
        }
    }
    0
}

pub fn clean_title(title: &str) -> String {
    let mut s = String::with_capacity(title.len());
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    for ch in title.chars() {
        match ch {
            '(' => depth_paren += 1,
            ')' if depth_paren > 0 => depth_paren -= 1,
            ')' => {}
            '[' => depth_bracket += 1,
            ']' if depth_bracket > 0 => depth_bracket -= 1,
            ']' => {}
            _ if depth_paren == 0 && depth_bracket == 0 => s.push(ch),
            _ => {}
        }
    }
    let low = s.to_lowercase();
    for feat in [" feat.", " ft.", " featuring "] {
        if let Some(idx) = low.find(feat) {
            s.truncate(idx);
            break;
        }
    }
    for dash in [" - ", " – ", " — "] {
        if let Some(idx) = s.find(dash) {
            let tail = norm(&s[idx + dash.len()..]);
            if arrangement_mask(&tail) != 0
                || [
                    "remaster",
                    "remastered",
                    "radio edit",
                    "edit",
                    "version",
                    "mono",
                    "stereo",
                ]
                .iter()
                .any(|w| tail.contains(w))
            {
                s.truncate(idx);
                break;
            }
        }
    }
    let trimmed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        title.trim().to_owned()
    } else {
        trimmed
    }
}

const ARRANGEMENT_TAGS: &[(&str, u32)] = &[
    ("remix", 1 << 0),
    ("live", 1 << 1),
    ("acoustic", 1 << 2),
    ("instrumental", 1 << 3),
    ("karaoke", 1 << 4),
    ("cover", 1 << 5),
    ("slowed", 1 << 6),
    ("daycore", 1 << 6),
    ("sped up", 1 << 7),
    ("speed up", 1 << 7),
    ("nightcore", 1 << 7),
    ("vip", 1 << 8),
    ("mashup", 1 << 9),
];

fn arrangement_mask(normalized: &str) -> u32 {
    let mut mask = 0u32;
    for &(tag, bit) in ARRANGEMENT_TAGS {
        if tag.contains(' ') {
            if normalized.contains(tag) {
                mask |= bit;
            }
        } else if has_word(normalized, tag) {
            mask |= bit;
        }
    }
    mask
}

fn has_word(haystack: &str, word: &str) -> bool {
    haystack.split_whitespace().any(|w| w == word)
}

pub fn arrangement_compatible(found_title: &str, wanted_title: &str) -> bool {
    let fm = arrangement_mask(&norm(found_title));
    let wm = arrangement_mask(&norm(wanted_title));
    fm == wm
}

pub fn similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    longer.contains(shorter) && shorter.chars().count() * 2 >= longer.chars().count()
}

pub fn title_ok(found: &str, wanted: &str) -> bool {
    if !arrangement_compatible(found, wanted) {
        return false;
    }
    similar(&norm(&clean_title(found)), &norm(&clean_title(wanted)))
        || similar(&norm(found), &norm(wanted))
}

pub fn artist_ok(found: &str, wanted: &str) -> bool {
    let f = norm(found);
    if f.is_empty() {
        return false;
    }
    for part in split_artists(wanted) {
        let p = norm(part);
        if !p.is_empty() && similar(&f, &p) {
            return true;
        }
    }
    for part in split_artists(found) {
        let fp = norm(part);
        for wpart in split_artists(wanted) {
            let wp = norm(wpart);
            if !fp.is_empty() && !wp.is_empty() && similar(&fp, &wp) {
                return true;
            }
        }
    }
    similar(&f, &norm(wanted))
}

fn split_artists(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < s.len() {
        let rem = &s[i..];
        let matched = if rem.starts_with(',') || rem.starts_with('&') || rem.starts_with('/') {
            Some(1)
        } else if rem.to_ascii_lowercase().starts_with(" feat. ") {
            Some(7)
        } else if rem.to_ascii_lowercase().starts_with(" ft. ") {
            Some(5)
        } else if rem.to_ascii_lowercase().starts_with(" featuring ") {
            Some(11)
        } else if rem.to_ascii_lowercase().starts_with(" x ") {
            Some(3)
        } else {
            None
        };
        if let Some(len) = matched {
            out.push(s[start..i].trim());
            i += len;
            start = i;
        } else {
            let ch_len = rem.chars().next().map_or(1, |c| c.len_utf8());
            i += ch_len;
            let _ = bytes;
        }
    }
    if start <= s.len() {
        out.push(s[start..].trim());
    }
    out
}

pub fn match_level(found_title: &str, found_artist: &str, t: &Track) -> MatchLevel {
    let mut best = MatchLevel::None;
    for q in queries(t) {
        if !title_ok(found_title, &q.title) {
            continue;
        }
        if artist_ok(found_artist, &q.artist) {
            return MatchLevel::Full;
        }
        if norm(&clean_title(found_title)) == norm(&clean_title(&q.title)) {
            best = MatchLevel::Loose;
        }
    }
    best
}

pub fn duration_diff_secs(found_seconds: f64, t: &Track) -> Option<f64> {
    if t.duration_ms == 0 || found_seconds <= 0.0 || !found_seconds.is_finite() {
        return None;
    }
    Some((found_seconds - (t.duration_ms as f64 / 1000.0)).abs())
}

pub fn duration_fits(diff: Option<f64>, max_sec: f64, level: MatchLevel) -> bool {
    match level {
        MatchLevel::None => false,
        MatchLevel::Loose => diff.is_some_and(|d| d <= max_sec.min(3.0)),
        MatchLevel::Full => diff.is_none_or(|d| d <= max_sec),
    }
}

fn is_cyrillic(c: char) -> bool {
    matches!(c as u32, 0x0400..=0x052F)
}

pub fn has_cyrillic(s: &str) -> bool {
    s.chars().any(is_cyrillic)
}

pub fn script_ok(title: &str, lines: &[LyricLine]) -> bool {
    if !has_cyrillic(title) {
        return true;
    }
    let mut cyr = 0usize;
    let mut letters = 0usize;
    for l in lines {
        for c in l.text.chars() {
            if is_cyrillic(c) {
                cyr += 1;
                letters += 1;
            } else if c.is_alphabetic() {
                letters += 1;
            }
        }
    }
    letters == 0 || (cyr as f64) >= (letters as f64) * 0.2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_track(title: &str, artist: &str, dur_s: u64) -> Track {
        Track {
            id: "1".into(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_ms: dur_s * 1000,
            cover_url: None,
            provider: "yandex".into(),
        }
    }

    #[test]
    fn normalizes_yo_only_in_match_keys_never_display() {
        let t = mk_track("Мальчик с ёлкой", "Алёна Швец", 150);
        // Display remains intact
        assert_eq!(t.artist, "Алёна Швец");
        assert_eq!(
            match_level("Мальчик с елкой", "Алена Швец", &t),
            MatchLevel::Full
        );
    }

    #[test]
    fn rejects_remix_when_original_wanted_and_checks_script() {
        let orig = mk_track("Ночь", "Кино", 200);
        assert_eq!(match_level("Ночь (Remix)", "Кино", &orig), MatchLevel::None);
        assert_eq!(match_level("Ночь (Live)", "Кино", &orig), MatchLevel::None);

        let lines_en = vec![LyricLine::new(1000, "Wrong English translation only")];
        assert!(!script_ok(&orig.title, &lines_en));
        let lines_ru = vec![LyricLine::new(1000, "Тихая ночь")];
        assert!(script_ok(&orig.title, &lines_ru));
    }

    #[test]
    fn enforces_strict_loose_duration_bounds() {
        assert!(!duration_fits(Some(4.5), 5.0, MatchLevel::Loose));
        assert!(duration_fits(Some(2.5), 5.0, MatchLevel::Loose));
        assert!(duration_fits(Some(4.5), 5.0, MatchLevel::Full));
        assert!(!duration_fits(None, 5.0, MatchLevel::Loose));
    }
}
