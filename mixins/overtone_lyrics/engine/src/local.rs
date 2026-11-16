//! Local music library indexer, ID3v1/v2 + FLAC/OGG/MP4 tag reader, and audio byte cache.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config;
use crate::i18n;
use crate::model::{Platform, Track};

const MAX_TAG_BYTES: usize = 24 << 20;
const MAX_IMAGE_BYTES: usize = 8 << 20;
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 50_000;
const MAX_RESULTS: usize = 3000;
const DEFAULT_MEM_CAP: usize = 96 << 20;
const SHARED_MEM_CAP: usize = 24 << 20;
const DISK_CAP: u64 = 512 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    Mp3,
    Pcm,
    Flac,
    Ogg,
    Mp4,
}

impl AudioKind {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = extension(path);
        match ext.as_str() {
            "mp3" => Some(Self::Mp3),
            "wav" | "wave" | "aif" | "aiff" | "au" => Some(Self::Pcm),
            "flac" => Some(Self::Flac),
            "ogg" | "oga" => Some(Self::Ogg),
            "m4a" | "mp4" | "m4b" => Some(Self::Mp4),
            _ => None,
        }
    }
}

pub fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub struct AudioTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<Vec<u8>>,
    pub lyrics: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContainerInfo {
    pub duration_ms: i64,
    pub tags: AudioTags,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct LibraryEntry {
    id: String,
    path: String,
    title: String,
    artist: String,
    album: String,
    cover: Option<String>,
    size: u64,
    mtime: u64,
    duration_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct IndexFile {
    entries: Vec<LibraryEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SourcesFile {
    sources: Vec<String>,
    excluded: Vec<String>,
}

// Windows-1251 codepoints for 0x80..=0xFF.
const CP1251_HIGH: [char; 128] = [
    '\u{0402}', '\u{0403}', '\u{201A}', '\u{0453}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{20AC}', '\u{2030}', '\u{0409}', '\u{2039}', '\u{040A}', '\u{040C}', '\u{040B}', '\u{040F}',
    '\u{0452}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{0098}', '\u{2122}', '\u{0459}', '\u{203A}', '\u{045A}', '\u{045C}', '\u{045B}', '\u{045F}',
    '\u{00A0}', '\u{040E}', '\u{045E}', '\u{0408}', '\u{00A4}', '\u{0490}', '\u{00A6}', '\u{00A7}',
    '\u{0401}', '\u{00A9}', '\u{0404}', '\u{00AB}', '\u{00AC}', '\u{00AD}', '\u{00AE}', '\u{0407}',
    '\u{00B0}', '\u{00B1}', '\u{0406}', '\u{0456}', '\u{0491}', '\u{00B5}', '\u{00B6}', '\u{00B7}',
    '\u{0451}', '\u{2116}', '\u{0454}', '\u{00BB}', '\u{0458}', '\u{0405}', '\u{0455}', '\u{0457}',
    'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', 'Р', 'С', 'Т',
    'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы', 'Ь', 'Э', 'Ю', 'Я', 'а', 'б', 'в', 'г', 'д', 'е',
    'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о', 'п', 'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш',
    'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я',
];

fn decode_cp1251(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x80 {
            out.push(b as char);
        } else {
            out.push(CP1251_HIGH[(b - 0x80) as usize]);
        }
    }
    out
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn looks_like_cp1251(bytes: &[u8]) -> bool {
    let mut run = 0usize;
    for &b in bytes {
        if b >= 0x80 {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Decodes raw text file bytes (.lrc/.txt): UTF-8 (stripping BOM), falling back to Windows-1251.
pub fn decode_text_bytes(raw: &[u8]) -> String {
    let skip = if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        3
    } else {
        0
    };
    let slice = &raw[skip..];
    match std::str::from_utf8(slice) {
        Ok(s) => s.to_string(),
        Err(_) => decode_cp1251(slice),
    }
}

fn decode_id3_string(bytes: &[u8], enc: u8) -> String {
    match enc {
        1 => {
            if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
                let u16s: Vec<u16> = bytes[2..]
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&u16s)
            } else {
                let start = if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
                    2
                } else {
                    0
                };
                let u16s: Vec<u16> = bytes[start..]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&u16s)
            }
        }
        2 => {
            let u16s: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&u16s)
        }
        3 => String::from_utf8_lossy(bytes).into_owned(),
        _ => {
            if looks_like_cp1251(bytes) {
                decode_cp1251(bytes)
            } else {
                decode_latin1(bytes)
            }
        }
    }
}

fn synchsafe(b: &[u8]) -> usize {
    ((b[0] as usize & 0x7F) << 21)
        | ((b[1] as usize & 0x7F) << 14)
        | ((b[2] as usize & 0x7F) << 7)
        | (b[3] as usize & 0x7F)
}

fn be_u24(b: &[u8]) -> usize {
    ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | (b[2] as usize)
}

fn undo_unsync(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        if b[i] == 0xFF && i + 1 < b.len() && b[i + 1] == 0 {
            i += 1;
        }
        i += 1;
    }
    out
}

fn skip_terminated(b: &[u8], from: usize, enc: u8) -> usize {
    let wide = enc == 1 || enc == 2;
    let mut i = from;
    while i < b.len() {
        if !wide && b[i] == 0 {
            return i + 1;
        }
        if wide && i + 1 < b.len() && b[i] == 0 && b[i + 1] == 0 {
            return i + 2;
        }
        i += if wide { 2 } else { 1 };
    }
    b.len()
}

pub fn read_id3_tags(file: &Path) -> AudioTags {
    if let Some(tags) = read_id3_v2(file) {
        return tags;
    }
    read_id3_v1(file).unwrap_or_default()
}

fn read_id3_v2(file: &Path) -> Option<AudioTags> {
    let mut f = fs::File::open(file).ok()?;
    let mut head = [0u8; 10];
    f.read_exact(&mut head).ok()?;
    if &head[..3] != b"ID3" {
        return None;
    }
    let version = head[3];
    let flags = head[5];
    let size = synchsafe(&head[6..10]);
    if size == 0 || size > MAX_TAG_BYTES || !(2..=4).contains(&version) {
        return None;
    }
    let mut raw = vec![0u8; size];
    f.read_exact(&mut raw).ok()?;
    if (flags & 0x80) != 0 && version < 4 {
        raw = undo_unsync(&raw);
    }

    let mut pos = 0usize;
    if (flags & 0x40) != 0 && version >= 3 && raw.len() >= 4 {
        pos = if version == 4 {
            synchsafe(&raw[..4])
        } else {
            u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize + 4
        };
    }

    let id_len = if version == 2 { 3 } else { 4 };
    let head_len = if version == 2 { 6 } else { 10 };
    let mut tags = AudioTags::default();

    while pos + head_len <= raw.len() {
        let id = String::from_utf8_lossy(&raw[pos..pos + id_len]).into_owned();
        if id.starts_with('\0') {
            break;
        }
        let fsize = if version == 2 {
            be_u24(&raw[pos + 3..pos + 6])
        } else if version == 4 {
            synchsafe(&raw[pos + 4..pos + 8])
        } else {
            u32::from_be_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]]) as usize
        };
        let format_flags = if version == 2 {
            0
        } else {
            u16::from_be_bytes([raw[pos + 8], raw[pos + 9]])
        };
        let start = pos + head_len;
        let Some(end) = start.checked_add(fsize) else {
            break;
        };
        if end > raw.len() {
            break;
        }
        pos = end;

        let unreadable = if version == 3 {
            (format_flags & 0x00C0) != 0
        } else {
            version == 4 && (format_flags & 0x000C) != 0
        };
        if unreadable {
            continue;
        }

        let body_owned;
        let body = if version == 4 && (format_flags & 0x0001) != 0 && end - start > 4 {
            &raw[start + 4..end]
        } else {
            &raw[start..end]
        };
        let body = if version == 4 && (format_flags & 0x0002) != 0 {
            body_owned = undo_unsync(body);
            body_owned.as_slice()
        } else {
            body
        };

        match id.as_str() {
            "TIT2" | "TT2" if body.len() >= 2 => {
                let s = decode_id3_string(&body[1..], body[0]);
                tags.title = s.split('\0').next().unwrap_or("").trim().to_string();
            }
            "TPE1" | "TP1" if body.len() >= 2 => {
                let s = decode_id3_string(&body[1..], body[0]);
                tags.artist = s.split('\0').next().unwrap_or("").trim().to_string();
            }
            "TALB" | "TAL" if body.len() >= 2 => {
                let s = decode_id3_string(&body[1..], body[0]);
                tags.album = s.split('\0').next().unwrap_or("").trim().to_string();
            }
            "USLT" | "ULT" if tags.lyrics.is_empty() && body.len() >= 5 => {
                let at = skip_terminated(body, 4, body[0]);
                if at < body.len() {
                    tags.lyrics = decode_id3_string(&body[at..], body[0]).trim().to_string();
                }
            }
            "APIC" | "PIC" if tags.cover.is_none() && body.len() >= 8 => {
                let enc = body[0];
                let mut at = 1usize;
                if id == "PIC" {
                    at += 3;
                } else {
                    while at < body.len() && body[at] != 0 {
                        at += 1;
                    }
                    at += 1;
                }
                at += 1; // picture type byte
                at = skip_terminated(body, at, enc);
                if at < body.len() {
                    tags.cover = Some(body[at..].to_vec());
                }
            }
            _ => {}
        }
    }

    Some(tags)
}

fn read_id3_v1(file: &Path) -> Option<AudioTags> {
    let meta = fs::metadata(file).ok()?;
    let len = meta.len();
    if len < 128 {
        return None;
    }
    let mut f = fs::File::open(file).ok()?;
    f.seek(SeekFrom::End(-128)).ok()?;
    let mut buf = [0u8; 128];
    f.read_exact(&mut buf).ok()?;
    if &buf[..3] != b"TAG" {
        return None;
    }
    let field = |from: usize| -> String {
        let slice = &buf[from..from + 30];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(30);
        decode_id3_string(&slice[..end], 0).trim().to_string()
    };
    Some(AudioTags {
        title: field(3),
        artist: field(33),
        album: field(63),
        cover: None,
        lyrics: String::new(),
    })
}

pub fn read_container_info(file: &Path, ext: &str) -> ContainerInfo {
    let Ok(mut f) = fs::File::open(file) else {
        return ContainerInfo::default();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    match ext {
        "flac" => read_flac_container(&mut f, len).unwrap_or_default(),
        "ogg" | "oga" => read_ogg_container(&mut f, len).unwrap_or_default(),
        "m4a" | "mp4" | "m4b" => read_mp4_container(&mut f, len).unwrap_or_default(),
        _ => ContainerInfo::default(),
    }
}

fn read_at(f: &mut fs::File, offset: u64, n: usize) -> Option<Vec<u8>> {
    if n > MAX_META_READ {
        return None;
    }
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

const MAX_META_READ: usize = MAX_TAG_BYTES + 1024;

fn parse_flac_picture(b: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 4usize;
    for _ in 0..2 {
        let len = u32::from_be_bytes(b.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos = pos.checked_add(4 + len)?;
    }
    pos = pos.checked_add(16)?;
    let n = u32::from_be_bytes(b.get(pos..pos + 4)?.try_into().ok()?) as usize;
    pos += 4;
    if n == 0 || pos + n > b.len() {
        return None;
    }
    Some(b[pos..pos + n].to_vec())
}

fn parse_vorbis_comments(b: &[u8], start: usize, tags: &mut AudioTags) {
    let Some(slice) = b.get(start..) else {
        return;
    };
    let Some(vendor_len_b) = slice.get(..4) else {
        return;
    };
    let vendor_len = u32::from_le_bytes(vendor_len_b.try_into().unwrap()) as usize;
    let mut pos = match 4usize.checked_add(vendor_len) {
        Some(p) => p,
        None => return,
    };
    let Some(count_b) = slice.get(pos..pos + 4) else {
        return;
    };
    let count = u32::from_le_bytes(count_b.try_into().unwrap()) as usize;
    pos += 4;
    for _ in 0..count {
        let Some(len_b) = slice.get(pos..pos + 4) else {
            break;
        };
        let len = u32::from_le_bytes(len_b.try_into().unwrap()) as usize;
        pos += 4;
        let Some(kv_b) = slice.get(pos..pos + len) else {
            break;
        };
        pos += len;
        let kv = String::from_utf8_lossy(kv_b);
        if let Some((k, v)) = kv.split_once('=') {
            let k_up = k.to_ascii_uppercase();
            match k_up.as_str() {
                "TITLE" if tags.title.is_empty() => tags.title = v.to_string(),
                "ARTIST" => {
                    if tags.artist.is_empty() {
                        tags.artist = v.to_string();
                    } else {
                        tags.artist.push_str(", ");
                        tags.artist.push_str(v);
                    }
                }
                "ALBUMARTIST" if tags.artist.is_empty() => tags.artist = v.to_string(),
                "ALBUM" if tags.album.is_empty() => tags.album = v.to_string(),
                "LYRICS" | "UNSYNCEDLYRICS" if tags.lyrics.is_empty() => {
                    tags.lyrics = v.to_string()
                }
                "METADATA_BLOCK_PICTURE" if tags.cover.is_none() => {
                    if let Ok(dec) =
                        base64::engine::general_purpose::STANDARD.decode(v.trim().as_bytes())
                    {
                        tags.cover = parse_flac_picture(&dec);
                    }
                }
                _ => {}
            }
        }
    }
    tags.title = tags.title.trim().to_string();
    tags.artist = tags.artist.trim().to_string();
    tags.album = tags.album.trim().to_string();
}

fn read_flac_container(f: &mut fs::File, _len: u64) -> Option<ContainerInfo> {
    let mut p = 0u64;
    let head = read_at(f, 0, 10)?;
    if &head[..3] == b"ID3" {
        p = 10 + synchsafe(&head[6..10]) as u64;
        if (head[5] & 0x10) != 0 {
            p += 10;
        }
    }
    let magic = read_at(f, p, 4)?;
    if &magic != b"fLaC" {
        return None;
    }
    p += 4;
    let mut duration_ms = 0i64;
    let mut tags = AudioTags::default();
    let mut last = false;
    while !last {
        let h = read_at(f, p, 4)?;
        last = (h[0] & 0x80) != 0;
        let kind = h[0] & 0x7F;
        let len = be_u24(&h[1..4]);
        p += 4;
        if kind == 0 && len >= 18 {
            let si = read_at(f, p, 18)?;
            let v = u64::from_be_bytes(si[10..18].try_into().ok()?);
            let rate = v >> 44;
            let total = v & 0xF_FFFF_FFFF;
            if rate > 0 {
                duration_ms = ((total as u128 * 1000) / rate as u128) as i64;
            }
        } else if kind == 4 && len <= MAX_TAG_BYTES {
            let vb = read_at(f, p, len)?;
            parse_vorbis_comments(&vb, 0, &mut tags);
        } else if kind == 6 && len <= MAX_TAG_BYTES && tags.cover.is_none() {
            let pb = read_at(f, p, len)?;
            tags.cover = parse_flac_picture(&pb);
        }
        p += len as u64;
    }
    Some(ContainerInfo { duration_ms, tags })
}

struct OggPacketReader<'a> {
    f: &'a mut fs::File,
    pos: u64,
    size: u64,
    segs: Vec<u8>,
    seg_at: usize,
    data_at: u64,
}

impl<'a> OggPacketReader<'a> {
    fn new(f: &'a mut fs::File, size: u64) -> Self {
        Self {
            f,
            pos: 0,
            size,
            segs: Vec::new(),
            seg_at: 0,
            data_at: 0,
        }
    }

    fn next_packet(&mut self) -> Option<Vec<u8>> {
        let mut cur = Vec::new();
        loop {
            if self.seg_at >= self.segs.len() && !self.next_page() {
                return None;
            }
            while self.seg_at < self.segs.len() {
                let l = self.segs[self.seg_at] as usize;
                self.seg_at += 1;
                if cur.len() + l > MAX_TAG_BYTES {
                    return None;
                }
                let chunk = read_at(self.f, self.data_at, l)?;
                cur.extend_from_slice(&chunk);
                self.data_at += l as u64;
                if l < 255 {
                    return Some(cur);
                }
            }
        }
    }

    fn next_page(&mut self) -> bool {
        if self.pos + 27 > self.size {
            return false;
        }
        let Some(h) = read_at(self.f, self.pos, 27) else {
            return false;
        };
        if &h[..4] != b"OggS" {
            return false;
        }
        let n = h[26] as usize;
        let Some(segs) = read_at(self.f, self.pos + 27, n) else {
            return false;
        };
        let body: u64 = segs.iter().map(|&x| x as u64).sum();
        self.data_at = self.pos + 27 + n as u64;
        self.pos = self.data_at + body;
        self.segs = segs;
        self.seg_at = 0;
        true
    }
}

fn read_ogg_container(f: &mut fs::File, size: u64) -> Option<ContainerInfo> {
    let mut r = OggPacketReader::new(f, size);
    let ident = r.next_packet()?;
    if ident.len() < 16 || ident[0] != 1 || &ident[1..7] != b"vorbis" {
        return None;
    }
    let rate = u32::from_le_bytes(ident[12..16].try_into().ok()?) as i64;
    let mut tags = AudioTags::default();
    if let Some(comment) = r.next_packet()
        && comment.len() > 7
        && comment[0] == 3
    {
        parse_vorbis_comments(&comment, 7, &mut tags);
    }
    let tail = size.min(128 * 1024) as usize;
    let mut granule = 0i64;
    if tail >= 14
        && let Some(b) = read_at(f, size - tail as u64, tail)
    {
        for i in (0..=b.len().saturating_sub(14)).rev() {
            if &b[i..i + 4] == b"OggS" {
                let g = i64::from_le_bytes(b[i + 6..i + 14].try_into().ok()?);
                if g > 0 {
                    granule = g;
                    break;
                }
            }
        }
    }
    let duration_ms = if rate > 0 && granule > 0 {
        granule.saturating_mul(1000) / rate
    } else {
        0
    };
    Some(ContainerInfo { duration_ms, tags })
}

fn read_mp4_container(f: &mut fs::File, size: u64) -> Option<ContainerInfo> {
    let mut tags = AudioTags::default();
    let mut dur = 0i64;
    scan_mp4_atoms(f, 0, size, &mut tags, &mut dur, 0);
    Some(ContainerInfo {
        duration_ms: dur,
        tags,
    })
}

fn scan_mp4_atoms(
    f: &mut fs::File,
    from: u64,
    to: u64,
    tags: &mut AudioTags,
    dur: &mut i64,
    depth: usize,
) {
    let mut p = from;
    while p + 8 <= to && depth < 8 {
        let Some(h) = read_at(f, p, 8) else { break };
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let kind_bytes: [u8; 4] = h[4..8].try_into().unwrap();
        let mut hdr = 8u64;
        if size == 1 {
            let Some(big) = read_at(f, p + 8, 8) else {
                break;
            };
            size = u64::from_be_bytes(big.try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = to - p;
        }
        if size < hdr || p + size > to {
            break;
        }
        let body = p + hdr;
        let end = p + size;
        match &kind_bytes {
            b"moov" | b"udta" | b"ilst" => scan_mp4_atoms(f, body, end, tags, dur, depth + 1),
            b"meta" if body + 4 <= end => scan_mp4_atoms(f, body + 4, end, tags, dur, depth + 1),
            b"mvhd" => {
                let read_n = (end - body).min(32) as usize;
                if let Some(m) = read_at(f, body, read_n)
                    && !m.is_empty()
                {
                    let (scale, length) = if m[0] == 1 && m.len() >= 32 {
                        (
                            u32::from_be_bytes(m[20..24].try_into().unwrap()) as u64,
                            u64::from_be_bytes(m[24..32].try_into().unwrap()),
                        )
                    } else if m.len() >= 20 {
                        (
                            u32::from_be_bytes(m[12..16].try_into().unwrap()) as u64,
                            u32::from_be_bytes(m[16..20].try_into().unwrap()) as u64,
                        )
                    } else {
                        (0, 0)
                    };
                    if scale > 0 {
                        *dur = ((length as u128 * 1000) / scale as u128) as i64;
                    }
                }
            }
            b"\xA9nam" | b"\xA9ART" | b"aART" | b"\xA9alb" | b"\xA9lyr" | b"covr" => {
                if end - body <= MAX_TAG_BYTES as u64
                    && let Some(raw) = read_at(f, body, (end - body) as usize)
                    && raw.len() >= 16
                    && &raw[4..8] == b"data"
                {
                    let item_size = u32::from_be_bytes(raw[..4].try_into().unwrap()) as usize;
                    if item_size >= 16 && item_size <= raw.len() {
                        let val = &raw[16..item_size];
                        match &kind_bytes {
                            b"\xA9nam" => {
                                tags.title = String::from_utf8_lossy(val).trim().to_string()
                            }
                            b"\xA9ART" => {
                                tags.artist = String::from_utf8_lossy(val).trim().to_string()
                            }
                            b"aART" if tags.artist.is_empty() => {
                                tags.artist = String::from_utf8_lossy(val).trim().to_string()
                            }
                            b"\xA9alb" => {
                                tags.album = String::from_utf8_lossy(val).trim().to_string()
                            }
                            b"\xA9lyr" => {
                                tags.lyrics = String::from_utf8_lossy(val).trim().to_string()
                            }
                            b"covr" => tags.cover = Some(val.to_vec()),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
        p = end;
    }
}

pub fn from_filename(file: &Path) -> (String, String) {
    let mut n = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .replace('_', " ");
    // Strip leading track index like "01. " / "01 - " / "01) "
    let trimmed = n.trim_start();
    let digit_end = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if (1..=3).contains(&digit_end) {
        let rest = trimmed[digit_end..].trim_start();
        if let Some(after) = rest.strip_prefix(['.', ')', '-']) {
            n = after.trim().to_string();
        }
    }
    let clean = n.trim().to_string();
    if let Some((a, t)) = clean.split_once(" - ") {
        let a = a.trim();
        let t = t.trim();
        if !a.is_empty() && !t.is_empty() {
            return (a.to_string(), t.to_string());
        }
    }
    let fallback = if clean.is_empty() {
        file.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        clean
    };
    (String::new(), fallback)
}

fn norm_local(s: &str) -> String {
    s.to_lowercase().replace('ё', "е").trim().to_string()
}

fn alnum_norm(s: &str) -> String {
    let lower = norm_local(s);
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = true;
    for ch in lower.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

fn name_keys(file: &Path) -> HashSet<String> {
    let (a, t) = from_filename(file);
    let mut out = HashSet::new();
    let full = if a.is_empty() {
        t.clone()
    } else {
        format!("{a} {t}")
    };
    let k1 = alnum_norm(&full);
    let k2 = alnum_norm(&t);
    if !k1.is_empty() {
        out.insert(k1);
    }
    if !k2.is_empty() {
        out.insert(k2);
    }
    out
}

pub fn sidecar_lyrics_text(audio: &Path) -> String {
    if let Some(dir) = audio.parent() {
        let want = name_keys(audio);
        let mut best: Option<(bool, PathBuf)> = None;
        if let Ok(entries) = fs::read_dir(dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                let ext = extension(&p);
                if ext != "lrc" && ext != "txt" {
                    continue;
                }
                if name_keys(&p).is_disjoint(&want) {
                    continue;
                }
                let is_lrc = ext == "lrc";
                if best.as_ref().is_none_or(|(b_lrc, _)| !*b_lrc && is_lrc) {
                    best = Some((is_lrc, p));
                }
            }
        }
        if let Some((_, p)) = best
            && fs::metadata(&p)
                .map(|m| m.len() < (1 << 20))
                .unwrap_or(false)
            && let Ok(bytes) = fs::read(&p)
        {
            return decode_text_bytes(&bytes);
        }
    }
    let ext = extension(audio);
    match AudioKind::from_path(audio) {
        Some(AudioKind::Mp3) => read_id3_tags(audio).lyrics,
        Some(AudioKind::Flac | AudioKind::Ogg | AudioKind::Mp4) => {
            read_container_info(audio, &ext).tags.lyrics
        }
        _ => String::new(),
    }
}

#[derive(Default)]
struct LibraryState {
    by_id: HashMap<String, LibraryEntry>,
    sorted: Vec<LibraryEntry>,
    sources: Vec<String>,
    excluded: HashSet<String>,
    folder_covers: HashMap<PathBuf, Option<String>>,
    progress: String,
    scanning: bool,
    rescan_again: bool,
    loaded: bool,
    version: u64,
}

#[derive(Clone)]
pub struct LocalLibrary {
    inner: Arc<Mutex<LibraryState>>,
}

impl Default for LocalLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalLibrary {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LibraryState::default())),
        }
    }

    pub fn default_folder() -> PathBuf {
        config::data_dir().join("overtone-music")
    }

    fn dir() -> PathBuf {
        config::data_dir().join("musicplayer-local")
    }

    fn covers_dir() -> PathBuf {
        Self::dir().join("covers")
    }

    fn ensure_loaded(&self) {
        let mut st = self.inner.lock().unwrap();
        if st.loaded {
            return;
        }
        st.loaded = true;
        if let Ok(raw) = fs::read_to_string(Self::dir().join("sources.json"))
            && let Ok(sf) = serde_json::from_str::<SourcesFile>(&raw)
        {
            st.sources = sf.sources;
            st.excluded = sf.excluded.into_iter().collect();
        }
        if let Ok(raw) = fs::read_to_string(Self::dir().join("index.json"))
            && let Ok(idx) = serde_json::from_str::<IndexFile>(&raw)
        {
            for e in idx.entries {
                if !e.id.is_empty() && !e.path.is_empty() {
                    st.by_id.insert(e.id.clone(), e);
                }
            }
        }
        let _ = fs::create_dir_all(Self::default_folder());
        Self::publish_locked(&mut st);
        drop(st);
        self.rescan();
    }

    fn publish_locked(st: &mut LibraryState) {
        let mut list: Vec<LibraryEntry> = st.by_id.values().cloned().collect();
        list.sort_by(|a, b| {
            norm_local(&a.artist)
                .cmp(&norm_local(&b.artist))
                .then_with(|| norm_local(&a.title).cmp(&norm_local(&b.title)))
        });
        st.sorted = list;
        st.version = st.version.wrapping_add(1);
    }

    pub fn version(&self) -> u64 {
        self.inner.lock().unwrap().version
    }

    pub fn status(&self) -> String {
        self.ensure_loaded();
        let st = self.inner.lock().unwrap();
        if !st.progress.is_empty() {
            return st.progress.clone();
        }
        if st.by_id.is_empty() {
            i18n::get("local.empty")
        } else {
            i18n::tracks(st.by_id.len())
        }
    }

    pub fn path_of(&self, id: &str) -> Option<PathBuf> {
        self.ensure_loaded();
        self.inner
            .lock()
            .unwrap()
            .by_id
            .get(id)
            .map(|e| PathBuf::from(&e.path))
    }

    pub fn search(&self, query: &str) -> Vec<Track> {
        self.ensure_loaded();
        let norm_q = norm_local(query);
        let words: Vec<&str> = norm_q.split_whitespace().collect();
        let st = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for e in &st.sorted {
            if out.len() >= MAX_RESULTS {
                break;
            }
            let hay = norm_local(&format!("{} {} {}", e.title, e.artist, e.album));
            if words.iter().all(|w| hay.contains(w)) {
                out.push(Track {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    artist: e.artist.clone(),
                    album: e.album.clone(),
                    duration_ms: e.duration_ms.max(0) as u64,
                    cover_url: e.cover.clone(),
                    provider: Platform::Local.id().to_owned(),
                });
            }
        }
        out
    }

    pub fn add_paths(&self, dropped: &[PathBuf]) -> usize {
        self.ensure_loaded();
        let mut added = 0usize;
        {
            let mut st = self.inner.lock().unwrap();
            let def = Self::default_folder();
            for p in dropped {
                let abs = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                let ok = abs.is_dir() || (abs.is_file() && AudioKind::from_path(&abs).is_some());
                if !ok {
                    continue;
                }
                let s = abs.to_string_lossy().to_string();
                if st.excluded.remove(&s) {
                    added += 1;
                }
                let covered = abs.starts_with(&def)
                    || st.sources.iter().any(|src| {
                        let sp = Path::new(src);
                        abs == sp || (sp.is_dir() && abs.starts_with(sp))
                    });
                if !covered {
                    st.sources.push(s);
                    added += 1;
                }
            }
            if added > 0 {
                Self::save_sources_locked(&st);
            }
        }
        if added > 0 {
            self.rescan();
        }
        added
    }

    pub fn remove(&self, id: &str) -> bool {
        self.ensure_loaded();
        let mut st = self.inner.lock().unwrap();
        let Some(e) = st.by_id.remove(id) else {
            return false;
        };
        st.excluded.insert(e.path);
        Self::save_sources_locked(&st);
        Self::publish_locked(&mut st);
        Self::save_index_locked(&st);
        true
    }

    pub fn rescan(&self) {
        self.ensure_loaded();
        let should_spawn = {
            let mut st = self.inner.lock().unwrap();
            if st.scanning {
                st.rescan_again = true;
                false
            } else {
                st.scanning = true;
                true
            }
        };
        if !should_spawn {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-local-index".to_string())
            .spawn(move || me.scan_loop())
            .ok();
    }

    fn scan_loop(&self) {
        loop {
            let (roots, excluded, old_entries) = {
                let mut st = self.inner.lock().unwrap();
                st.folder_covers.clear();
                let mut roots = vec![Self::default_folder()];
                for s in &st.sources {
                    roots.push(PathBuf::from(s));
                }
                (roots, st.excluded.clone(), st.by_id.clone())
            };

            let files = collect_audio_files(&roots, &excluded);
            let total = files.len();
            let mut seen = HashSet::new();
            let mut updated = HashMap::new();
            let mut folder_covers = HashMap::new();

            for (idx, file) in files.into_iter().enumerate() {
                {
                    let mut st = self.inner.lock().unwrap();
                    st.progress = i18n::fmt("local.indexing", &[&format!("{}/{}", idx + 1, total)]);
                }
                if let Some(entry) = index_file(&file, &old_entries, &mut folder_covers) {
                    seen.insert(entry.id.clone());
                    updated.insert(entry.id.clone(), entry);
                }
                if (idx + 1) % 40 == 0 {
                    let mut st = self.inner.lock().unwrap();
                    for (k, v) in &updated {
                        st.by_id.insert(k.clone(), v.clone());
                    }
                    Self::publish_locked(&mut st);
                }
            }

            let again = {
                let mut st = self.inner.lock().unwrap();
                for (k, v) in updated {
                    st.by_id.insert(k, v);
                }
                st.by_id.retain(|k, _| seen.contains(k));
                st.progress.clear();
                Self::publish_locked(&mut st);
                Self::save_index_locked(&st);
                let again = st.rescan_again;
                st.rescan_again = false;
                if !again {
                    st.scanning = false;
                }
                again
            };
            if !again {
                break;
            }
        }
    }

    fn save_sources_locked(st: &LibraryState) {
        let sf = SourcesFile {
            sources: st.sources.clone(),
            excluded: st.excluded.iter().cloned().collect(),
        };
        let dir = Self::dir();
        if fs::create_dir_all(&dir).is_ok()
            && let Ok(json) = serde_json::to_string(&sf)
        {
            let _ = fs::write(dir.join("sources.json"), json);
        }
    }

    fn save_index_locked(st: &LibraryState) {
        let idx = IndexFile {
            entries: st.by_id.values().cloned().collect(),
        };
        let dir = Self::dir();
        if fs::create_dir_all(&dir).is_ok()
            && let Ok(json) = serde_json::to_string(&idx)
        {
            let tmp = dir.join("index.json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, dir.join("index.json"));
            }
        }
    }
}

fn collect_audio_files(roots: &[PathBuf], excluded: &HashSet<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        if out.len() >= MAX_FILES {
            break;
        }
        walk_dir(root, excluded, 0, &mut out, &mut seen);
    }
    out
}

fn walk_dir(
    path: &Path,
    excluded: &HashSet<String>,
    depth: usize,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    if out.len() >= MAX_FILES || depth > MAX_DEPTH {
        return;
    }
    if path.is_file() {
        if AudioKind::from_path(path).is_some() {
            let abs = path.to_path_buf();
            let s = abs.to_string_lossy().to_string();
            if !excluded.contains(&s) && seen.insert(abs.clone()) {
                out.push(abs);
            }
        }
        return;
    }
    if !path.is_dir() {
        return;
    }
    if let Ok(entries) = fs::read_dir(path) {
        for ent in entries.flatten() {
            if out.len() >= MAX_FILES {
                break;
            }
            walk_dir(&ent.path(), excluded, depth + 1, out, seen);
        }
    }
}

fn id_of(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let d = hasher.finalize();
    let mut out = String::from("L");
    for b in &d[..8] {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn index_file(
    file: &Path,
    old_entries: &HashMap<String, LibraryEntry>,
    folder_covers: &mut HashMap<PathBuf, Option<String>>,
) -> Option<LibraryEntry> {
    let meta = fs::metadata(file).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let id = id_of(file);
    if let Some(old) = old_entries.get(&id)
        && old.size == size
        && old.mtime == mtime
    {
        return Some(old.clone());
    }

    let kind = AudioKind::from_path(file)?;
    let ext = extension(file);
    let (mut duration_ms, tags) = match kind {
        AudioKind::Flac | AudioKind::Ogg | AudioKind::Mp4 => {
            let info = read_container_info(file, &ext);
            (info.duration_ms, info.tags)
        }
        AudioKind::Mp3 => {
            let d = estimate_mp3_or_pcm_duration(file, kind);
            (d, read_id3_tags(file))
        }
        AudioKind::Pcm => {
            let d = estimate_mp3_or_pcm_duration(file, kind);
            (d, AudioTags::default())
        }
    };
    if duration_ms <= 0 && kind == AudioKind::Mp3 {
        duration_ms = ((size as u128 * 8000) / 128_000) as i64;
    }
    if duration_ms <= 0 {
        return None;
    }
    let (guess_artist, guess_title) = from_filename(file);
    let cover = store_cover(tags.cover.as_deref(), file.parent(), folder_covers);
    Some(LibraryEntry {
        id,
        path: file.to_string_lossy().to_string(),
        title: if tags.title.trim().is_empty() {
            guess_title
        } else {
            tags.title
        },
        artist: if tags.artist.trim().is_empty() {
            guess_artist
        } else {
            tags.artist
        },
        album: tags.album,
        cover,
        size,
        mtime,
        duration_ms,
    })
}

fn estimate_mp3_or_pcm_duration(file: &Path, kind: AudioKind) -> i64 {
    let Ok(bytes) = fs::read(file) else {
        return 0;
    };
    match kind {
        AudioKind::Pcm => crate::audio::pcm::PcmSource::duration_ms(&bytes) as i64,
        AudioKind::Mp3 => crate::audio::mp3::probe_duration_ms(&bytes),
        _ => 0,
    }
}

fn store_cover(
    embedded: Option<&[u8]>,
    folder: Option<&Path>,
    folder_covers: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    if let Some(bytes) = embedded
        && !bytes.is_empty()
    {
        return write_cover_bytes(bytes);
    }
    let dir = folder?;
    if let Some(cached) = folder_covers.get(dir) {
        return cached.clone();
    }
    let mut found = None;
    'outer: for name in ["cover", "folder", "front", "album"] {
        for ext in ["jpg", "jpeg", "png"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if let Ok(m) = fs::metadata(&candidate)
                && m.is_file()
                && m.len() <= MAX_IMAGE_BYTES as u64
                && let Ok(img) = fs::read(&candidate)
            {
                found = write_cover_bytes(&img);
                break 'outer;
            }
        }
    }
    folder_covers.insert(dir.to_path_buf(), found.clone());
    found
}

fn write_cover_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(40);
    for b in &digest[..20] {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    let file = LocalLibrary::covers_dir().join(format!("{hex}.img"));
    if !file.exists() {
        let dir = file.parent()?;
        fs::create_dir_all(dir).ok()?;
        fs::write(&file, bytes).ok()?;
    }
    url::Url::from_file_path(&file).ok().map(|u| u.to_string())
}

// ============================================================================
// AudioCache (LRU RAM + disk cache under data_dir()/musicplayer-cache)
// ============================================================================

struct AudioCacheInner {
    mem: HashMap<String, Vec<u8>>,
    lru: VecDeque<String>,
    mem_bytes: usize,
    keep: HashSet<String>,
    pinned: Option<String>,
    on_disk: HashSet<String>,
}

#[derive(Clone)]
pub struct AudioCache {
    mem_cap: usize,
    disk_enabled: Arc<RwLock<bool>>,
    inner: Arc<Mutex<AudioCacheInner>>,
}

impl Default for AudioCache {
    fn default() -> Self {
        Self::new(DEFAULT_MEM_CAP)
    }
}

impl AudioCache {
    pub fn new(mem_cap: usize) -> Self {
        let cache = Self {
            mem_cap,
            disk_enabled: Arc::new(RwLock::new(true)),
            inner: Arc::new(Mutex::new(AudioCacheInner {
                mem: HashMap::new(),
                lru: VecDeque::new(),
                mem_bytes: 0,
                keep: HashSet::new(),
                pinned: None,
                on_disk: HashSet::new(),
            })),
        };
        cache.scan_disk();
        cache
    }

    pub fn shared() -> Self {
        static SHARED: std::sync::OnceLock<AudioCache> = std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| AudioCache::new(SHARED_MEM_CAP))
            .clone()
    }

    pub fn set_disk_enabled(&self, enabled: bool) {
        *self.disk_enabled.write().unwrap() = enabled;
    }

    fn cache_dir() -> PathBuf {
        config::data_dir().join("musicplayer-cache")
    }

    fn safe_id(id: &str) -> String {
        id.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn disk_path(id: &str) -> PathBuf {
        Self::cache_dir().join(format!("{}.mp3", Self::safe_id(id)))
    }

    fn scan_disk(&self) {
        let dir = Self::cache_dir();
        if !dir.is_dir() {
            return;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut st = self.inner.lock().unwrap();
            for ent in entries.flatten() {
                let p = ent.path();
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".part") {
                        let _ = fs::remove_file(&p);
                    } else if let Some(stem) = name.strip_suffix(".mp3") {
                        st.on_disk.insert(stem.to_string());
                    }
                }
            }
        }
        self.prune_disk();
    }

    pub fn set_keep(&self, ids: HashSet<String>, current: Option<String>) {
        let mut st = self.inner.lock().unwrap();
        st.keep = ids;
        st.pinned = current;
    }

    pub fn has(&self, id: &str) -> bool {
        let st = self.inner.lock().unwrap();
        st.mem.contains_key(id) || st.on_disk.contains(&Self::safe_id(id))
    }

    pub fn get(&self, id: &str) -> Option<Vec<u8>> {
        {
            let mut st = self.inner.lock().unwrap();
            if let Some(v) = st.mem.get(id).cloned() {
                st.lru.retain(|k| k != id);
                st.lru.push_back(id.to_string());
                return Some(v);
            }
            if !st.on_disk.contains(&Self::safe_id(id)) {
                return None;
            }
        }
        let path = Self::disk_path(id);
        let Ok(data) = fs::read(&path) else {
            self.inner
                .lock()
                .unwrap()
                .on_disk
                .remove(&Self::safe_id(id));
            return None;
        };
        if data.is_empty() {
            self.inner
                .lock()
                .unwrap()
                .on_disk
                .remove(&Self::safe_id(id));
            return None;
        }
        self.mem_put(id, data.clone());
        Some(data)
    }

    pub fn put(&self, id: &str, data: Vec<u8>) {
        self.mem_put(id, data.clone());
        if !*self.disk_enabled.read().unwrap() {
            return;
        }
        let dir = Self::cache_dir();
        if fs::create_dir_all(&dir).is_ok() {
            let tmp = dir.join(format!("{}.part", Self::safe_id(id)));
            let target = Self::disk_path(id);
            if fs::write(&tmp, &data).is_ok() && fs::rename(&tmp, &target).is_ok() {
                self.inner.lock().unwrap().on_disk.insert(Self::safe_id(id));
                self.prune_disk();
            }
        }
    }

    fn mem_put(&self, id: &str, data: Vec<u8>) {
        let mut st = self.inner.lock().unwrap();
        if let Some(old) = st.mem.remove(id) {
            st.mem_bytes = st.mem_bytes.saturating_sub(old.len());
            st.lru.retain(|k| k != id);
        }
        st.mem_bytes += data.len();
        st.mem.insert(id.to_string(), data);
        st.lru.push_back(id.to_string());

        for pass in 0..2 {
            if st.mem_bytes <= self.mem_cap || st.mem.len() <= 1 {
                break;
            }
            let order: Vec<String> = st.lru.iter().cloned().collect();
            for k in order {
                if st.mem_bytes <= self.mem_cap || st.mem.len() <= 1 {
                    break;
                }
                if k == id || st.pinned.as_deref() == Some(k.as_str()) {
                    continue;
                }
                if pass == 0 && st.keep.contains(&k) {
                    continue;
                }
                if let Some(v) = st.mem.remove(&k) {
                    st.mem_bytes = st.mem_bytes.saturating_sub(v.len());
                    st.lru.retain(|x| x != &k);
                }
            }
        }
    }

    fn prune_disk(&self) {
        let dir = Self::cache_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        let mut total = 0u64;
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("mp3")
                && let Ok(m) = ent.metadata()
            {
                let sz = m.len();
                let t = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                total += sz;
                files.push((p, sz, t));
            }
        }
        if total <= DISK_CAP {
            return;
        }
        files.sort_by_key(|(_, _, t)| *t);
        let (keep_safe, pinned_safe) = {
            let st = self.inner.lock().unwrap();
            let ks: HashSet<String> = st.keep.iter().map(|k| Self::safe_id(k)).collect();
            let ps = st.pinned.as_deref().map(Self::safe_id);
            (ks, ps)
        };
        for (p, sz, _) in files {
            if total <= DISK_CAP {
                break;
            }
            let base = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if pinned_safe.as_deref() == Some(base.as_str()) || keep_safe.contains(&base) {
                continue;
            }
            if fs::remove_file(&p).is_ok() {
                self.inner.lock().unwrap().on_disk.remove(&base);
                total = total.saturating_sub(sz);
            }
        }
    }

    pub fn clear(&self) {
        {
            let mut st = self.inner.lock().unwrap();
            let pinned = st.pinned.clone();
            st.mem.retain(|k, _| pinned.as_deref() == Some(k.as_str()));
            st.lru.retain(|k| pinned.as_deref() == Some(k.as_str()));
            st.mem_bytes = st.mem.values().map(|v| v.len()).sum();
            st.on_disk.clear();
        }
        if let Ok(entries) = fs::read_dir(Self::cache_dir()) {
            for ent in entries.flatten() {
                let _ = fs::remove_file(ent.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_guessing_and_cp1251() {
        let p = Path::new("03. Алёна Швец - Вино и Сигареты.mp3");
        let (a, t) = from_filename(p);
        assert_eq!(a, "Алёна Швец");
        assert_eq!(t, "Вино и Сигареты");

        // CP1251 "Привет"
        let bytes = [0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2];
        assert!(looks_like_cp1251(&bytes));
        assert_eq!(decode_cp1251(&bytes), "Привет");
    }
}
