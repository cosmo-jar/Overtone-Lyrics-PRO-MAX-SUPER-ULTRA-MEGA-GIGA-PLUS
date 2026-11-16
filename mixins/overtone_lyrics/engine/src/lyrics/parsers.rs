use flate2::read::ZlibDecoder;
use serde_json::Value;
use std::io::Read;

use crate::model::{LyricLine, WordTiming};

pub fn parse_lrc(raw: &str) -> Vec<LyricLine> {
    let mut global_offset_ms: i64 = 0;
    let mut entries: Vec<(u64, String, Vec<WordTiming>)> = Vec::new();

    for raw_line in raw.lines() {
        let line = raw_line.trim().trim_start_matches('\u{FEFF}');
        if line.is_empty() {
            continue;
        }
        if let Some(off) = parse_offset_tag(line) {
            global_offset_ms = off;
            continue;
        }
        let mut rest = line;
        let mut line_times: Vec<u64> = Vec::new();
        while rest.starts_with('[') {
            if let Some(close) = rest.find(']') {
                let tag = &rest[1..close];
                if let Some(ms) = parse_timestamp_ms(tag) {
                    line_times.push(ms);
                    rest = &rest[close + 1..];
                    continue;
                } else if line_times.is_empty()
                    && rest.ends_with(']')
                    && !rest[close + 1..].contains('[')
                {
                    // Metadata tag like [ar:...], [ti:...]
                    rest = "";
                    break;
                }
            }
            break;
        }
        if line_times.is_empty() {
            continue;
        }
        let (clean_text, words) = extract_elrc_words(rest);
        for t in line_times {
            let shifted = (t as i64 - global_offset_ms).max(0) as u64;
            let shifted_words = words
                .iter()
                .map(|w| WordTiming {
                    time_ms: (w.time_ms as i64 - global_offset_ms).max(0) as u64,
                    end_ms: w
                        .end_ms
                        .map(|e| (e as i64 - global_offset_ms).max(0) as u64),
                    text: w.text.clone(),
                })
                .collect();
            entries.push((shifted, clean_text.clone(), shifted_words));
        }
    }

    entries.sort_by_key(|e| e.0);
    // Drop trailing empty instrumental marker only if followed immediately, keep intentional pauses
    entries
        .into_iter()
        .map(|(ms, text, words)| LyricLine::with_words(ms, text, words))
        .collect()
}

pub fn parse_plain_lines(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw_line in raw.lines() {
        let trimmed = raw_line.trim().trim_start_matches('\u{FEFF}');
        if trimmed.is_empty() {
            if !out.is_empty() && !matches!(out.last(), Some(s) if s.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        // Strip accidental LRC tags if plain endpoint returned LRC
        let mut s = trimmed;
        while s.starts_with('[') {
            if let Some(end) = s.find(']') {
                s = s[end + 1..].trim_start();
            } else {
                break;
            }
        }
        if !s.is_empty() {
            out.push(s.to_owned());
        }
    }
    while matches!(out.last(), Some(s) if s.is_empty()) {
        out.pop();
    }
    out
}

fn parse_offset_tag(line: &str) -> Option<i64> {
    let inner = line.strip_prefix("[offset:")?.strip_suffix(']')?;
    inner.trim().parse::<i64>().ok()
}

pub fn parse_timestamp_ms(tag: &str) -> Option<u64> {
    let t = tag.trim();
    let parts: Vec<&str> = t.split(':').collect();
    match parts.len() {
        2 => {
            let m = parts[0].parse::<u64>().ok()?;
            let sec_ms = parse_sec_frac(parts[1])?;
            Some(m.saturating_mul(60_000) + sec_ms)
        }
        3 => {
            let h = parts[0].parse::<u64>().ok()?;
            let m = parts[1].parse::<u64>().ok()?;
            let sec_ms = parse_sec_frac(parts[2])?;
            Some(h.saturating_mul(3_600_000) + m.saturating_mul(60_000) + sec_ms)
        }
        _ => None,
    }
}

fn parse_sec_frac(s: &str) -> Option<u64> {
    let normalized = s.replace(':', ".");
    if let Some((sec_str, frac_str)) = normalized.split_once('.') {
        let sec = sec_str.parse::<u64>().ok()?;
        let digits: String = frac_str.chars().take(3).collect();
        let val = digits.parse::<u64>().ok()?;
        let ms = match digits.len() {
            1 => val * 100,
            2 => val * 10,
            3 => val,
            _ => 0,
        };
        Some(sec.saturating_mul(1000) + ms)
    } else {
        let sec = normalized.parse::<u64>().ok()?;
        Some(sec.saturating_mul(1000))
    }
}

fn extract_elrc_words(rest: &str) -> (String, Vec<WordTiming>) {
    let mut clean = String::with_capacity(rest.len());
    let mut words = Vec::new();
    let mut i = 0usize;
    let mut pending_time: Option<u64> = None;
    let mut seg_start = 0usize;

    while i < rest.len() {
        if rest.as_bytes()[i] == b'<'
            && let Some(end_rel) = rest[i + 1..].find('>')
        {
            let tag = &rest[i + 1..i + 1 + end_rel];
            if let Some(ms) = parse_timestamp_ms(tag) {
                let piece = &rest[seg_start..i];
                clean.push_str(piece);
                if let Some(prev_t) = pending_time
                    && !piece.trim().is_empty()
                {
                    words.push(WordTiming {
                        time_ms: prev_t,
                        end_ms: Some(ms),
                        text: piece.trim().to_owned(),
                    });
                }
                pending_time = Some(ms);
                i += end_rel + 2;
                seg_start = i;
                continue;
            }
        }
        let ch_len = rest[i..].chars().next().map_or(1, |c| c.len_utf8());
        i += ch_len;
    }

    let tail = &rest[seg_start..];
    clean.push_str(tail);
    if let Some(prev_t) = pending_time
        && !tail.trim().is_empty()
    {
        words.push(WordTiming {
            time_ms: prev_t,
            end_ms: None,
            text: tail.trim().to_owned(),
        });
    }
    (clean.trim().to_owned(), words)
}

pub fn parse_ttml(raw: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    let mut pos = 0usize;
    let lower = raw.to_ascii_lowercase();

    while let Some(rel) = lower[pos..].find("<p") {
        let p_start = pos + rel;
        let after_tag_name = p_start + 2;
        let next_ch = lower.as_bytes().get(after_tag_name).copied();
        if !matches!(next_ch, Some(b' ' | b'>' | b'\t' | b'\n' | b'\r')) {
            pos = after_tag_name;
            continue;
        }
        let Some(gt_rel) = raw[p_start..].find('>') else {
            break;
        };
        let open_end = p_start + gt_rel;
        if raw.as_bytes().get(open_end.saturating_sub(1)) == Some(&b'/') {
            pos = open_end + 1;
            continue;
        }
        let p_attrs = &raw[p_start..open_end];
        let Some(close_rel) = lower[open_end + 1..].find("</p>") else {
            break;
        };
        let body_start = open_end + 1;
        let body_end = body_start + close_rel;
        let p_body = &raw[body_start..body_end];
        pos = body_end + 4;

        let begin_ms = extract_attr(p_attrs, "begin").and_then(|v| parse_ttml_time(&v));
        let (text, words) = parse_ttml_spans(p_body);
        if text.is_empty() && words.is_empty() {
            continue;
        }
        let final_begin = begin_ms.or_else(|| words.first().map(|w| w.time_ms));
        if let Some(ms) = final_begin {
            let final_text = if text.is_empty() {
                words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                text
            };
            lines.push(LyricLine::with_words(ms, final_text, words));
        }
    }

    lines.sort_by_key(|l| l.time_ms);
    lines
}

fn parse_ttml_spans(body: &str) -> (String, Vec<WordTiming>) {
    let mut text = String::new();
    let mut words = Vec::new();
    let mut i = 0usize;
    let lower = body.to_ascii_lowercase();

    while i < body.len() {
        if lower[i..].starts_with("<span")
            && let Some(gt_rel) = body[i..].find('>')
        {
            let open_end = i + gt_rel;
            let attrs = &body[i..open_end];
            if body.as_bytes().get(open_end.saturating_sub(1)) == Some(&b'/') {
                i = open_end + 1;
                continue;
            }
            let role = extract_attr(attrs, "ttm:role").unwrap_or_default();
            if let Some(close_rel) = find_matching_span_end(&lower[open_end + 1..]) {
                let inner_start = open_end + 1;
                let inner_end = inner_start + close_rel;
                let inner = &body[inner_start..inner_end];
                i = inner_end + "</span>".len();

                if role.eq_ignore_ascii_case("x-translation")
                    || role.eq_ignore_ascii_case("x-roman")
                {
                    continue;
                }
                let begin = extract_attr(attrs, "begin").and_then(|v| parse_ttml_time(&v));
                let end = extract_attr(attrs, "end").and_then(|v| parse_ttml_time(&v));
                let inner_text = unescape_xml(&strip_xml_tags(inner));
                if let Some(ms) = begin
                    && !inner_text.trim().is_empty()
                {
                    words.push(WordTiming {
                        time_ms: ms,
                        end_ms: end,
                        text: inner_text.trim().to_owned(),
                    });
                }
                text.push_str(&inner_text);
                continue;
            }
        }
        if body.as_bytes()[i] == b'<'
            && let Some(gt_rel) = body[i..].find('>')
        {
            i += gt_rel + 1;
            continue;
        }
        let next_lt = body[i..].find('<').map_or(body.len(), |r| i + r);
        let chunk = unescape_xml(&body[i..next_lt]);
        text.push_str(&chunk);
        i = next_lt;
    }

    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (cleaned, words)
}

fn find_matching_span_end(lower_slice: &str) -> Option<usize> {
    let bytes = lower_slice.as_bytes();
    let mut depth = 1i32;
    let mut pos = 0usize;
    while pos < bytes.len() {
        if bytes[pos..].starts_with(b"<span") {
            depth += 1;
            pos += 5;
        } else if bytes[pos..].starts_with(b"</span>") {
            depth -= 1;
            if depth == 0 {
                return Some(pos);
            }
            pos += 7;
        } else {
            pos += 1;
        }
    }
    None
}

fn strip_xml_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let low = attrs.to_ascii_lowercase();
    let needle = format!("{name}=");
    let pos = low.find(&needle)?;
    let val_start = pos + needle.len();
    let quote = attrs.as_bytes().get(val_start).copied()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let rest = &attrs[val_start + 1..];
    let end = rest.find(quote as char)?;
    Some(unescape_xml(&rest[..end]))
}

pub fn parse_ttml_time(s: &str) -> Option<u64> {
    let t = s.trim();
    if let Some(stripped) = t.strip_suffix("ms") {
        return stripped
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| v.max(0.0) as u64);
    }
    if let Some(stripped) = t.strip_suffix('s') {
        return stripped
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * 1000.0).max(0.0).round() as u64);
    }
    parse_timestamp_ms(t)
}

fn unescape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        if s.as_bytes()[i] == b'&'
            && let Some(semi) = s[i..].find(';')
        {
            let entity = &s[i + 1..i + semi];
            let rep = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with('#') => {
                    let num_str = &entity[1..];
                    let code = if let Some(hex) = num_str
                        .strip_prefix('x')
                        .or_else(|| num_str.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num_str.parse::<u32>().ok()
                    };
                    code.and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = rep {
                out.push(ch);
                i += semi + 1;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// NetEase YRC parser: each line looks like `[startMs,durMs](wStart,wDur,0)word...` or JSON metadata line.
pub fn parse_netease_yrc(raw: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed)
                && let Some(t_ms) = v.get("t").and_then(Value::as_u64)
                && let Some(c_arr) = v.get("c").and_then(Value::as_array)
            {
                let msg: String = c_arr
                    .iter()
                    .filter_map(|x| x.get("tx").and_then(Value::as_str))
                    .collect();
                if !msg.trim().is_empty() {
                    lines.push(LyricLine::new(t_ms, msg.trim()));
                }
            }
            continue;
        }
        if !trimmed.starts_with('[') {
            continue;
        }
        let Some(close) = trimmed.find(']') else {
            continue;
        };
        let header = &trimmed[1..close];
        let mut parts = header.split(',');
        let Some(start_ms) = parts.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let mut rest = &trimmed[close + 1..];
        let mut words = Vec::new();
        let mut text = String::new();

        while let Some(open) = rest.find('(') {
            if open > 0 {
                text.push_str(&rest[..open]);
            }
            let Some(end_p) = rest[open..].find(')') else {
                break;
            };
            let wmeta = &rest[open + 1..open + end_p];
            let after_meta = &rest[open + end_p + 1..];
            let next_open = after_meta.find('(').unwrap_or(after_meta.len());
            let wtext = &after_meta[..next_open];
            let mut wparts = wmeta.split(',');
            if let (Some(ws), Some(wd)) = (
                wparts.next().and_then(|s| s.parse::<u64>().ok()),
                wparts.next().and_then(|s| s.parse::<u64>().ok()),
            ) && !wtext.is_empty()
            {
                words.push(WordTiming {
                    time_ms: ws,
                    end_ms: Some(ws.saturating_add(wd)),
                    text: wtext.to_owned(),
                });
            }
            text.push_str(wtext);
            rest = &after_meta[next_open..];
        }
        let clean = text.trim().to_owned();
        if !clean.is_empty() {
            lines.push(LyricLine::with_words(start_ms, clean, words));
        }
    }
    lines.sort_by_key(|l| l.time_ms);
    lines
}

/// KuGou KRC format decoder ("krc1" header + 16-byte XOR key + zlib deflate).
pub fn decode_krc(raw: &[u8]) -> Option<Vec<LyricLine>> {
    const KEY: [u8; 16] = [
        64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105,
    ];
    if raw.len() <= 4 || &raw[..4] != b"krc1" {
        return None;
    }
    let xored: Vec<u8> = raw[4..]
        .iter()
        .enumerate()
        .map(|(i, &b)| b ^ KEY[i % 16])
        .collect();
    let mut dec = ZlibDecoder::new(xored.as_slice());
    let mut text = String::new();
    if dec.read_to_string(&mut text).is_err() {
        return None;
    }
    let _ = xored.len();
    Some(parse_krc_text(&text))
}

fn parse_krc_text(raw: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }
        let Some(close) = trimmed.find(']') else {
            continue;
        };
        let header = &trimmed[1..close];
        let Some((s_str, _d_str)) = header.split_once(',') else {
            continue;
        };
        let Ok(line_start) = s_str.parse::<u64>() else {
            continue;
        };
        let mut rest = &trimmed[close + 1..];
        let mut words = Vec::new();
        let mut clean = String::new();

        while let Some(open) = rest.find('<') {
            let Some(end_gt) = rest[open..].find('>') else {
                break;
            };
            let meta = &rest[open + 1..open + end_gt];
            let after = &rest[open + end_gt + 1..];
            let next_open = after.find('<').unwrap_or(after.len());
            let wtext = &after[..next_open];
            let mut parts = meta.split(',');
            if let (Some(off), Some(dur)) = (
                parts.next().and_then(|s| s.parse::<u64>().ok()),
                parts.next().and_then(|s| s.parse::<u64>().ok()),
            ) && !wtext.is_empty()
            {
                let ws = line_start + off;
                words.push(WordTiming {
                    time_ms: ws,
                    end_ms: Some(ws + dur),
                    text: wtext.to_owned(),
                });
            }
            clean.push_str(wtext);
            rest = &after[next_open..];
        }
        let final_text = clean.trim().to_owned();
        if !final_text.is_empty() {
            out.push(LyricLine::with_words(line_start, final_text, words));
        }
    }
    out.sort_by_key(|l| l.time_ms);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_and_elrc_and_offset() {
        let lrc = "[offset:100]\n[00:10.50]Hello <00:10.50>world <00:11.00>again\n[00:20.00][00:40.00]Chorus";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].time_ms, 10_400);
        assert_eq!(lines[0].text, "Hello world again");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[1].time_ms, 19_900);
        assert_eq!(lines[2].time_ms, 39_900);
    }

    #[test]
    fn parses_ttml_amll_with_word_spans() {
        let ttml = r#"
        <tt><body><div>
          <p begin="00:05.10" end="00:08.50"><span begin="5.1s" end="5.9s">First </span><span begin="6.0s" end="7.2s">Word &amp; Test</span><span ttm:role="x-translation">Перевод</span></p>
        </div></body></tt>"#;
        let lines = parse_ttml(ttml);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time_ms, 5100);
        assert_eq!(lines[0].text, "First Word & Test");
        assert_eq!(lines[0].words.len(), 2);
    }

    #[test]
    fn parses_netease_yrc_word_timings() {
        let yrc = "[12000,3000](12000,500,0)Привет (12500,900,0)мир";
        let lines = parse_netease_yrc(yrc);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time_ms, 12000);
        assert_eq!(lines[0].text, "Привет мир");
        assert_eq!(lines[0].words.len(), 2);
    }
}
