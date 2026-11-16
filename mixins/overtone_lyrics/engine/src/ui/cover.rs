use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config::data_dir;
use crate::net;

pub const HUD_SIZE: u32 = 64;
pub const BIG_SIZE: u32 = 256;
pub const PICKER_CELLS: u32 = 30;
const MAX_MEM: usize = 24;
const CORNER_RATIO: f32 = 0.12;
const ATLAS_W: u32 = 352;
const ATLAS_H: u32 = 256;

#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct CacheSlot {
    key: String,
    image: RgbaImage,
}

#[derive(Default)]
struct CacheInner {
    hud: VecDeque<CacheSlot>,
    big: VecDeque<CacheSlot>,
    inflight_hud: HashSet<String>,
    inflight_big: HashSet<String>,
}

/// Manages cover downloads, image decoding (PNG/JPEG/WebP/GIF), rounded-corner resizing,
/// disk persistence under `<data_dir>/cover_cache`, and LRU RAM cache (1:1 `CoverArt.java`).
pub struct CoverArtManager {
    inner: Arc<Mutex<CacheInner>>,
}

impl Default for CoverArtManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverArtManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheInner::default())),
        }
    }

    pub fn offer_hud(&self, id: &str, url: Option<&str>) {
        self.offer(id, url, false);
    }

    pub fn offer_big(&self, id: &str, url: Option<&str>) {
        self.offer(id, url, true);
    }

    pub fn get_hud(&self, id: &str) -> Option<RgbaImage> {
        let mut g = self.inner.lock().ok()?;
        let pos = g.hud.iter().position(|s| s.key == id)?;
        let slot = g.hud.remove(pos)?;
        let img = slot.image.clone();
        g.hud.push_back(slot);
        Some(img)
    }

    pub fn has_big(&self, id: &str) -> bool {
        self.inner
            .lock()
            .map(|g| g.big.iter().any(|s| s.key == id))
            .unwrap_or(false)
    }

    pub fn get_big(&self, id: &str) -> Option<RgbaImage> {
        let mut g = self.inner.lock().ok()?;
        let pos = g.big.iter().position(|s| s.key == id)?;
        let slot = g.big.remove(pos)?;
        let img = slot.image.clone();
        g.big.push_back(slot);
        Some(img)
    }

    fn offer(&self, id: &str, url: Option<&str>, big: bool) {
        let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
            return;
        };
        if id.is_empty() {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if big {
            if g.big.iter().any(|s| s.key == id) || !g.inflight_big.insert(id.to_owned()) {
                return;
            }
        } else if g.hud.iter().any(|s| s.key == id) || !g.inflight_hud.insert(id.to_owned()) {
            return;
        }
        drop(g);

        let key = id.to_owned();
        let url_owned = url.to_owned();
        let inner = Arc::clone(&self.inner);
        let size = if big { BIG_SIZE } else { HUD_SIZE };
        let _ = thread::Builder::new()
            .name(format!(
                "Overtone-Cover-{}",
                if big { "Big" } else { "Hud" }
            ))
            .spawn(move || {
                let loaded = fetch_or_load_rounded(&key, &url_owned, size);
                if let Ok(mut g) = inner.lock() {
                    if big {
                        g.inflight_big.remove(&key);
                        if let Some(img) = loaded {
                            push_lru(&mut g.big, CacheSlot { key, image: img });
                        }
                    } else {
                        g.inflight_hud.remove(&key);
                        if let Some(img) = loaded {
                            push_lru(&mut g.hud, CacheSlot { key, image: img });
                        }
                    }
                }
            });
    }
}

fn push_lru(deque: &mut VecDeque<CacheSlot>, slot: CacheSlot) {
    deque.retain(|s| s.key != slot.key);
    while deque.len() >= MAX_MEM {
        deque.pop_front();
    }
    deque.push_back(slot);
}

fn cache_file(url: &str, size: u32) -> PathBuf {
    let digest = net::md5_hex(url.as_bytes());
    let prefix = if size >= BIG_SIZE { "big" } else { "hud" };
    data_dir()
        .join("cover_cache")
        .join(format!("{prefix}_{digest}.rgba"))
}

fn fetch_or_load_rounded(key: &str, url: &str, target: u32) -> Option<RgbaImage> {
    let cache = cache_file(url, target);
    if let Some(img) = read_cached_rgba(&cache, target) {
        return Some(img);
    }
    let raw = read_raw_image_bytes(key, url)?;
    let decoded = decode_image_to_rgba(&raw)?;
    let resized = resize_and_round(
        &decoded,
        target,
        target,
        (target as f32 * CORNER_RATIO).round() as u32,
    );
    write_cached_rgba(&cache, &resized);
    Some(resized)
}

fn read_raw_image_bytes(_key: &str, url: &str) -> Option<Vec<u8>> {
    let trimmed = url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        net::get_bytes(trimmed, &[]).ok()
    } else if let Some(path) = trimmed.strip_prefix("file://") {
        fs::read(path).ok()
    } else if Path::new(trimmed).is_file() {
        fs::read(trimmed).ok()
    } else {
        None
    }
}

fn read_cached_rgba(path: &Path, target: u32) -> Option<RgbaImage> {
    let bytes = fs::read(path).ok()?;
    let expected = (target * target * 4) as usize;
    if bytes.len() != expected {
        return None;
    }
    Some(RgbaImage {
        width: target,
        height: target,
        pixels: bytes,
    })
}

fn write_cached_rgba(path: &Path, img: &RgbaImage) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, &img.pixels);
}

/// Decodes PNG, JPEG, WebP, or GIF bytes into RGBA8 pixels using pure-Rust decoders.
pub fn decode_image_to_rgba(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() < 8 {
        return None;
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        return decode_png(bytes);
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return decode_jpeg(bytes);
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return decode_webp(bytes);
    }
    if bytes.starts_with(b"GIF8") {
        return decode_gif(bytes);
    }
    decode_jpeg(bytes)
        .or_else(|| decode_png(bytes))
        .or_else(|| decode_webp(bytes))
        .or_else(|| decode_gif(bytes))
}

fn decode_png(bytes: &[u8]) -> Option<RgbaImage> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let raw = &buf[..info.buffer_size()];
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(raw),
        png::ColorType::Rgb => {
            for c in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for c in raw.chunks_exact(2) {
                rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in raw {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    Some(RgbaImage {
        width: info.width,
        height: info.height,
        pixels: rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Option<RgbaImage> {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    let pixels = decoder.decode().ok()?;
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    if pixels.len() == info.width as usize * info.height as usize * 3 {
        for c in pixels.chunks_exact(3) {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    } else if pixels.len() == info.width as usize * info.height as usize {
        for g in pixels {
            rgba.extend_from_slice(&[g, g, g, 255]);
        }
    } else if pixels.len() == info.width as usize * info.height as usize * 4 {
        rgba = pixels;
    } else {
        return None;
    }
    Some(RgbaImage {
        width: info.width as u32,
        height: info.height as u32,
        pixels: rgba,
    })
}

fn decode_webp(bytes: &[u8]) -> Option<RgbaImage> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
    let (w, h) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    let mut buf = vec![0u8; decoder.output_buffer_size()?];
    decoder.read_image(&mut buf).ok()?;
    let rgba = if has_alpha {
        buf
    } else {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for c in buf.chunks_exact(3) {
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        out
    };
    Some(RgbaImage {
        width: w,
        height: h,
        pixels: rgba,
    })
}

fn decode_gif(bytes: &[u8]) -> Option<RgbaImage> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts.read_info(Cursor::new(bytes)).ok()?;
    let frame = decoder.read_next_frame().ok()??;
    Some(RgbaImage {
        width: frame.width as u32,
        height: frame.height as u32,
        pixels: frame.buffer.to_vec(),
    })
}

/// Center-crops to square, bilinearly resizes to `dst_w x dst_h`, and applies rounded corners.
pub fn resize_and_round(src: &RgbaImage, dst_w: u32, dst_h: u32, radius: u32) -> RgbaImage {
    let side = src.width.min(src.height).max(1);
    let sx0 = (src.width.saturating_sub(side)) / 2;
    let sy0 = (src.height.saturating_sub(side)) / 2;
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    let r = radius as f32;

    for y in 0..dst_h {
        let v = (y as f32 + 0.5) * side as f32 / dst_h as f32 - 0.5;
        let y0 = (v.floor() as i32).clamp(0, side as i32 - 1) as u32;
        let y1 = (y0 + 1).min(side - 1);
        let fy = (v - y0 as f32).clamp(0.0, 1.0);
        for x in 0..dst_w {
            let u = (x as f32 + 0.5) * side as f32 / dst_w as f32 - 0.5;
            let x0 = (u.floor() as i32).clamp(0, side as i32 - 1) as u32;
            let x1 = (x0 + 1).min(side - 1);
            let fx = (u - x0 as f32).clamp(0.0, 1.0);

            let p00 = sample_px(src, sx0 + x0, sy0 + y0);
            let p10 = sample_px(src, sx0 + x1, sy0 + y0);
            let p01 = sample_px(src, sx0 + x0, sy0 + y1);
            let p11 = sample_px(src, sx0 + x1, sy0 + y1);

            let mut rgba = [0u8; 4];
            for ch in 0..4 {
                let top = p00[ch] as f32 * (1.0 - fx) + p10[ch] as f32 * fx;
                let bot = p01[ch] as f32 * (1.0 - fx) + p11[ch] as f32 * fx;
                rgba[ch] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
            }
            if r > 0.0 {
                let cx = if (x as f32) < r {
                    r - (x as f32 + 0.5)
                } else if (x as f32 + 0.5) > dst_w as f32 - r {
                    (x as f32 + 0.5) - (dst_w as f32 - r)
                } else {
                    0.0
                };
                let cy = if (y as f32) < r {
                    r - (y as f32 + 0.5)
                } else if (y as f32 + 0.5) > dst_h as f32 - r {
                    (y as f32 + 0.5) - (dst_h as f32 - r)
                } else {
                    0.0
                };
                if cx > 0.0 && cy > 0.0 {
                    let dist = (cx * cx + cy * cy).sqrt();
                    let cov = (r + 0.5 - dist).clamp(0.0, 1.0);
                    rgba[3] = ((rgba[3] as f32) * cov).round() as u8;
                }
            }
            let idx = ((y * dst_w + x) * 4) as usize;
            out[idx..idx + 4].copy_from_slice(&rgba);
        }
    }
    RgbaImage {
        width: dst_w,
        height: dst_h,
        pixels: out,
    }
}

fn sample_px(src: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    if src.width == 0 || src.height == 0 {
        return [0, 0, 0, 0];
    }
    let idx = ((y.min(src.height - 1) * src.width + x.min(src.width - 1)) * 4) as usize;
    [
        src.pixels[idx],
        src.pixels[idx + 1],
        src.pixels[idx + 2],
        src.pixels[idx + 3],
    ]
}

/// Packs HUD cover (64x64), Big cover (256x256), and ColorPicker textures (30x30 each)
/// into one 352x256 RGBA buffer suitable for Horizon's `native_mod_image`.
pub struct ModImageAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub dirty: bool,
    last_hud_id: String,
    last_big_id: String,
    last_hue_q: i32,
    hue_strip_ready: bool,
}

impl Default for ModImageAtlas {
    fn default() -> Self {
        Self::new()
    }
}

impl ModImageAtlas {
    pub const HUD_RECT: [f32; 4] = [0.0, 0.0, 64.0, 64.0];
    pub const BIG_RECT: [f32; 4] = [64.0, 0.0, 256.0, 256.0];
    pub const PICKER_SQ_RECT: [f32; 4] = [320.0, 0.0, 30.0, 30.0];
    pub const PICKER_HUE_RECT: [f32; 4] = [320.0, 32.0, 30.0, 30.0];

    pub fn new() -> Self {
        Self {
            width: ATLAS_W,
            height: ATLAS_H,
            rgba: vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize],
            dirty: false,
            last_hud_id: String::new(),
            last_big_id: String::new(),
            last_hue_q: -1,
            hue_strip_ready: false,
        }
    }

    pub fn sync_covers(&mut self, covers: &CoverArtManager, track_id: &str) {
        if track_id.is_empty() {
            return;
        }
        if self.last_hud_id != track_id
            && let Some(img) = covers.get_hud(track_id)
        {
            self.blit(0, 0, &img);
            self.last_hud_id = track_id.to_owned();
            self.dirty = true;
        }
        if self.last_big_id != track_id
            && let Some(img) = covers.get_big(track_id)
        {
            self.blit(64, 0, &img);
            self.last_big_id = track_id.to_owned();
            self.dirty = true;
        }
    }

    pub fn sync_picker(&mut self, hue: f32) {
        let q = (hue * 1000.0).round() as i32;
        if self.last_hue_q != q {
            let mut sq = vec![0u8; (PICKER_CELLS * PICKER_CELLS * 4) as usize];
            for r in 0..PICKER_CELLS {
                let val = 1.0 - (r as f32 + 0.5) / PICKER_CELLS as f32;
                for c in 0..PICKER_CELLS {
                    let sat = (c as f32 + 0.5) / PICKER_CELLS as f32;
                    let rgb = hsv_to_rgb(hue, sat, val);
                    let idx = ((r * PICKER_CELLS + c) * 4) as usize;
                    sq[idx] = ((rgb >> 16) & 0xFF) as u8;
                    sq[idx + 1] = ((rgb >> 8) & 0xFF) as u8;
                    sq[idx + 2] = (rgb & 0xFF) as u8;
                    sq[idx + 3] = 255;
                }
            }
            self.blit(
                320,
                0,
                &RgbaImage {
                    width: PICKER_CELLS,
                    height: PICKER_CELLS,
                    pixels: sq,
                },
            );
            self.last_hue_q = q;
            self.dirty = true;
        }
        if !self.hue_strip_ready {
            let mut hs = vec![0u8; (PICKER_CELLS * PICKER_CELLS * 4) as usize];
            for r in 0..PICKER_CELLS {
                let h = (r as f32 + 0.5) / PICKER_CELLS as f32;
                let rgb = hsv_to_rgb(h, 1.0, 1.0);
                for c in 0..PICKER_CELLS {
                    let idx = ((r * PICKER_CELLS + c) * 4) as usize;
                    hs[idx] = ((rgb >> 16) & 0xFF) as u8;
                    hs[idx + 1] = ((rgb >> 8) & 0xFF) as u8;
                    hs[idx + 2] = (rgb & 0xFF) as u8;
                    hs[idx + 3] = 255;
                }
            }
            self.blit(
                320,
                32,
                &RgbaImage {
                    width: PICKER_CELLS,
                    height: PICKER_CELLS,
                    pixels: hs,
                },
            );
            self.hue_strip_ready = true;
            self.dirty = true;
        }
    }

    fn blit(&mut self, dst_x: u32, dst_y: u32, img: &RgbaImage) {
        let w = img.width.min(self.width.saturating_sub(dst_x));
        let h = img.height.min(self.height.saturating_sub(dst_y));
        for row in 0..h {
            let src_off = (row * img.width * 4) as usize;
            let dst_off = (((dst_y + row) * self.width + dst_x) * 4) as usize;
            let n = (w * 4) as usize;
            self.rgba[dst_off..dst_off + n].copy_from_slice(&img.pixels[src_off..src_off + n]);
        }
    }
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> u32 {
    let h = h.clamp(0.0, 0.9999);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let hh = h * 6.0;
    let x = c * (1.0 - (hh % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match hh as i32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let r = ((r + m) * 255.0).round() as u32;
    let g = ((g + m) * 255.0).round() as u32;
    let b = ((b + m) * 255.0).round() as u32;
    (r << 16) | (g << 8) | b
}

pub fn rgb_to_hsv(rgb: u32) -> [f32; 3] {
    let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
    let b = (rgb & 0xFF) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = if d == 0.0 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        ((g - b) / d % 6.0) / 6.0
    } else if (max - g).abs() < 1e-6 {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    if h < 0.0 {
        h += 1.0;
    }
    let s = if max == 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_atlas_generates_square_and_hue_and_hsv_roundtrips() {
        let mut atlas = ModImageAtlas::new();
        assert_eq!(atlas.rgba.len(), (ATLAS_W * ATLAS_H * 4) as usize);
        atlas.sync_picker(0.5);
        assert!(atlas.dirty);
        let rgb = hsv_to_rgb(0.33, 1.0, 1.0);
        let [h, s, v] = rgb_to_hsv(rgb);
        assert!((h - 0.33).abs() < 0.02);
        assert!((s - 1.0).abs() < 0.02);
        assert!((v - 1.0).abs() < 0.02);
    }
}
