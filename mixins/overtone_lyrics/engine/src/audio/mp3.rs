//! Pure-Rust MPEG-1/2/2.5 Layer III (MP3) decoder ported 1:1 from JLayer (including Xing/VBRI/LAME gapless trim).

use std::sync::LazyLock;

use super::mp3_tables::{
    CA, CS, HUFF_TABS, IO, NR_OF_SFB_BLOCK, PRE_TAB, S_LEN, SF_BAND_INDEX, SFD, TAN12,
    TWO_TO_NEGATIVE_HALF_POW, WIN,
};

const MPEG2_LSF: usize = 0;
const MPEG1: usize = 1;
const MPEG25_LSF: usize = 2;

const JOINT_STEREO: usize = 1;
const SINGLE_CHANNEL: usize = 3;

const SAMPLES_PER_MP3_FRAME: i64 = 1152;
const DECODER_DELAY: i32 = 529;
const MAX_FRAME_BYTES: usize = 1732;
const BUFSIZE: usize = 4096 * 8;
const BUFMASK: usize = BUFSIZE - 1;
const SBLIMIT: usize = 32;
const SSLIMIT: usize = 18;
const D43: f64 = 4.0 / 3.0;

const FREQUENCIES: [[u32; 4]; 3] = [
    [22050, 24000, 16000, 1],
    [44100, 48000, 32000, 1],
    [11025, 12000, 8000, 1],
];

const BITRATES: [[[i32; 16]; 3]; 3] = [
    // MPEG2 LSF
    [
        [
            0, 32000, 48000, 56000, 64000, 80000, 96000, 112000, 128000, 144000, 160000, 176000,
            192000, 224000, 256000, 0,
        ],
        [
            0, 8000, 16000, 24000, 32000, 40000, 48000, 56000, 64000, 80000, 96000, 112000, 128000,
            144000, 160000, 0,
        ],
        [
            0, 8000, 16000, 24000, 32000, 40000, 48000, 56000, 64000, 80000, 96000, 112000, 128000,
            144000, 160000, 0,
        ],
    ],
    // MPEG1
    [
        [
            0, 32000, 64000, 96000, 128000, 160000, 192000, 224000, 256000, 288000, 320000, 352000,
            384000, 416000, 448000, 0,
        ],
        [
            0, 32000, 48000, 56000, 64000, 80000, 96000, 112000, 128000, 160000, 192000, 224000,
            256000, 320000, 384000, 0,
        ],
        [
            0, 32000, 40000, 48000, 56000, 64000, 80000, 96000, 112000, 128000, 160000, 192000,
            224000, 256000, 320000, 0,
        ],
    ],
    // MPEG2.5 LSF
    [
        [
            0, 32000, 48000, 56000, 64000, 80000, 96000, 112000, 128000, 144000, 160000, 176000,
            192000, 224000, 256000, 0,
        ],
        [
            0, 8000, 16000, 24000, 32000, 40000, 48000, 56000, 64000, 80000, 96000, 112000, 128000,
            144000, 160000, 0,
        ],
        [
            0, 8000, 16000, 24000, 32000, 40000, 48000, 56000, 64000, 80000, 96000, 112000, 128000,
            144000, 160000, 0,
        ],
    ],
];

const MS_PER_FRAMES: [[f32; 3]; 3] = [
    [8.707_483, 8.0, 12.0],
    [26.122_45, 24.0, 36.0],
    [26.122_45, 24.0, 36.0],
];

const H_VBR_TIME_PER_FRAME: [f64; 4] = [-1.0, 384.0, 1152.0, 1152.0];

static T_43: LazyLock<Vec<f32>> =
    LazyLock::new(|| (0..8192).map(|i| (i as f64).powf(D43) as f32).collect());

static REORDER_TABLES: LazyLock<Vec<[usize; 576]>> = LazyLock::new(|| {
    (0..9)
        .map(|sf| {
            let band = &SF_BAND_INDEX[sf].s;
            let mut ix = [0usize; 576];
            let mut j = 0usize;
            for sfb in 0..13 {
                let start = band[sfb];
                let end = band[sfb + 1];
                for w in 0..3 {
                    for i in start..end {
                        let dst = 3 * i + w;
                        if dst < 576 && j < 576 {
                            ix[dst] = j;
                        }
                        j += 1;
                    }
                }
            }
            ix
        })
        .collect()
});

#[derive(Clone, Debug)]
pub struct Mp3Header {
    pub version: usize,
    pub layer: usize,
    pub bitrate_index: usize,
    pub sample_frequency_idx: usize,
    pub padding_bit: usize,
    pub protection_bit: usize,
    pub mode: usize,
    pub mode_extension: usize,
    pub framesize: usize,
    pub n_slots: usize,
    pub raw_header: u32,
    pub vbr: bool,
    pub vbr_frames: i32,
    pub vbr_bytes: i32,
    pub enc_delay: i32,
    pub enc_padding: i32,
}

impl Mp3Header {
    fn from_u32(header: u32) -> Option<Self> {
        let version = ((header >> 19) & 3) as usize;
        if version == 1 {
            return None;
        }
        let version_mapped = match version {
            0 => MPEG25_LSF,
            2 => MPEG2_LSF,
            3 => MPEG1,
            _ => return None,
        };
        let layer_bits = ((header >> 17) & 3) as usize;
        if layer_bits == 0 {
            return None;
        }
        let layer = 4 - layer_bits;
        let protection_bit = ((header >> 16) & 1) as usize;
        let bitrate_index = ((header >> 12) & 0xF) as usize;
        let sample_frequency_idx = ((header >> 10) & 3) as usize;
        if sample_frequency_idx == 3 || bitrate_index == 15 {
            return None;
        }
        let padding_bit = ((header >> 9) & 1) as usize;
        let mode = ((header >> 6) & 3) as usize;
        let mode_extension = ((header >> 4) & 3) as usize;

        if version_mapped == MPEG1 && bitrate_index == 0 {
            return None;
        }

        let freq = FREQUENCIES[version_mapped][sample_frequency_idx] as i32;
        if freq <= 1 {
            return None;
        }
        let br = BITRATES[version_mapped][layer - 1][bitrate_index];
        if br <= 0 {
            return None;
        }

        let (framesize_total, n_slots) = if layer == 1 {
            let mut fs = (12 * br) / freq;
            if padding_bit != 0 {
                fs += 1;
            }
            ((fs as usize) << 2, 0)
        } else {
            let mut fs = (144 * br) / freq;
            if version_mapped == MPEG2_LSF || version_mapped == MPEG25_LSF {
                fs >>= 1;
            }
            if padding_bit != 0 {
                fs += 1;
            }
            let slots = if layer == 3 {
                let side = if version_mapped == MPEG1 {
                    if mode == SINGLE_CHANNEL { 17 } else { 32 }
                } else if mode == SINGLE_CHANNEL {
                    9
                } else {
                    17
                };
                let crc = if protection_bit != 0 { 0 } else { 2 };
                (fs as usize).saturating_sub(side + crc + 4)
            } else {
                0
            };
            (fs as usize, slots)
        };

        if framesize_total <= 4 || framesize_total - 4 > MAX_FRAME_BYTES {
            return None;
        }

        Some(Self {
            version: version_mapped,
            layer,
            bitrate_index,
            sample_frequency_idx,
            padding_bit,
            protection_bit,
            mode,
            mode_extension,
            framesize: framesize_total - 4,
            n_slots,
            raw_header: header,
            vbr: false,
            vbr_frames: -1,
            vbr_bytes: -1,
            enc_delay: -1,
            enc_padding: -1,
        })
    }

    pub fn frequency(&self) -> u32 {
        FREQUENCIES[self.version][self.sample_frequency_idx]
    }

    pub fn channels(&self) -> usize {
        if self.mode == SINGLE_CHANNEL { 1 } else { 2 }
    }

    pub fn sample_sfreq_index(&self) -> usize {
        if self.version == MPEG1 {
            self.sample_frequency_idx + 3
        } else if self.version == MPEG2_LSF {
            self.sample_frequency_idx
        } else {
            self.sample_frequency_idx + 6
        }
    }

    pub fn ms_per_frame(&self) -> f64 {
        if self.vbr {
            let mut tpf = H_VBR_TIME_PER_FRAME[self.layer] / self.frequency() as f64;
            if self.version == MPEG2_LSF || self.version == MPEG25_LSF {
                tpf /= 2.0;
            }
            tpf * 1000.0
        } else {
            MS_PER_FRAMES[self.layer - 1][self.sample_frequency_idx] as f64
        }
    }

    pub fn total_ms(&self, stream_bytes: usize) -> f64 {
        if self.vbr && self.vbr_frames > 0 {
            self.vbr_frames as f64 * self.ms_per_frame()
        } else {
            let frame_with_hdr = self.framesize + 4 - self.padding_bit;
            stream_bytes.checked_div(frame_with_hdr).unwrap_or(0) as f64 * self.ms_per_frame()
        }
    }

    pub fn samples_to_skip_at_start(&self) -> i32 {
        if self.enc_delay <= 0 {
            0
        } else {
            self.enc_delay + DECODER_DELAY
        }
    }

    pub fn samples_to_skip_at_end(&self) -> i32 {
        if self.enc_padding <= 0 {
            0
        } else {
            (self.enc_padding - DECODER_DELAY).max(0)
        }
    }

    fn parse_vbr(&mut self, frame: &[u8]) {
        let xing_offset = if self.version == MPEG1 {
            if self.mode == SINGLE_CHANNEL { 17 } else { 32 }
        } else if self.mode == SINGLE_CHANNEL {
            9
        } else {
            17
        };
        if self.parse_xing(frame, xing_offset) {
            return;
        }
        self.parse_vbri(frame);
    }

    fn parse_xing(&mut self, frame: &[u8], offset: usize) -> bool {
        if offset + 8 > frame.len() {
            return false;
        }
        let tag = &frame[offset..offset + 4];
        if tag != b"Xing" && tag != b"Info" {
            return false;
        }
        self.vbr = true;
        let flags = u32::from_be_bytes([
            frame[offset + 4],
            frame[offset + 5],
            frame[offset + 6],
            frame[offset + 7],
        ]);
        let mut cursor = offset + 8;
        if (flags & 0x01) != 0 && cursor + 4 <= frame.len() {
            self.vbr_frames = i32::from_be_bytes([
                frame[cursor],
                frame[cursor + 1],
                frame[cursor + 2],
                frame[cursor + 3],
            ]);
            cursor += 4;
        }
        if (flags & 0x02) != 0 && cursor + 4 <= frame.len() {
            self.vbr_bytes = i32::from_be_bytes([
                frame[cursor],
                frame[cursor + 1],
                frame[cursor + 2],
                frame[cursor + 3],
            ]);
            cursor += 4;
        }
        if (flags & 0x04) != 0 && cursor + 100 <= frame.len() {
            cursor += 100;
        }
        if (flags & 0x08) != 0 && cursor + 4 <= frame.len() {
            cursor += 4;
        }
        if cursor + 24 <= frame.len() {
            let ver = &frame[cursor..cursor + 4];
            if ver == b"LAME" || ver == b"Lavc" || ver == b"Lavf" {
                let at = cursor + 21;
                if at + 3 <= frame.len() {
                    self.enc_delay = ((frame[at] as i32) << 4) | ((frame[at + 1] as i32) >> 4);
                    self.enc_padding =
                        (((frame[at + 1] as i32) & 0x0F) << 8) | (frame[at + 2] as i32);
                }
            }
        }
        true
    }

    fn parse_vbri(&mut self, frame: &[u8]) {
        let offset = 32; // Bitstream frameBytes starts AFTER 4-byte sync header (Java offset 36-4=32)
        if offset + 18 <= frame.len() && &frame[offset..offset + 4] == b"VBRI" {
            self.vbr = true;
            let b_off = offset + 10;
            self.vbr_bytes = i32::from_be_bytes([
                frame[b_off],
                frame[b_off + 1],
                frame[b_off + 2],
                frame[b_off + 3],
            ]);
            self.vbr_frames = i32::from_be_bytes([
                frame[b_off + 4],
                frame[b_off + 5],
                frame[b_off + 6],
                frame[b_off + 7],
            ]);
        }
    }

    fn copy_vbr_from(&mut self, src: &Mp3Header) {
        self.vbr = src.vbr;
        self.vbr_frames = src.vbr_frames;
        self.vbr_bytes = src.vbr_bytes;
        self.enc_delay = src.enc_delay;
        self.enc_padding = src.enc_padding;
    }
}

/// Drops encoder delay & padding samples signaled by LAME/Xing tags.
#[derive(Clone, Debug)]
struct GaplessTrim {
    first: i64,
    last: i64,
    position: i64,
    offset: usize,
}

impl GaplessTrim {
    fn from_header(h: &Mp3Header, channels: usize) -> Self {
        let ch = channels.max(1) as i64;
        let first = i64::from(h.samples_to_skip_at_start()) * ch;
        let last = if h.vbr_frames <= 0 {
            -1
        } else {
            let total = i64::from(h.vbr_frames) * SAMPLES_PER_MP3_FRAME * ch;
            let raw_last = total - i64::from(h.samples_to_skip_at_end()) * ch;
            raw_last.max(first)
        };
        Self {
            first,
            last,
            position: 0,
            offset: 0,
        }
    }

    fn accept(&mut self, buffer_len: usize) -> usize {
        let len = buffer_len as i64;
        let start = self.position;
        let end = start + len;
        self.position = end;

        if end <= self.first {
            self.offset = 0;
            return 0;
        }
        let from = (self.first - start).max(0);
        let to = if self.last < 0 {
            len
        } else {
            len.min(self.last - start)
        };
        if to <= from {
            self.offset = 0;
            return 0;
        }
        self.offset = from as usize;
        (to - from) as usize
    }
}

struct BitReserve {
    offset: usize,
    totbit: i32,
    buf_byte_idx: usize,
    buf: Vec<u8>,
}

impl BitReserve {
    fn new() -> Self {
        Self {
            offset: 0,
            totbit: 0,
            buf_byte_idx: 0,
            buf: vec![0u8; BUFSIZE],
        }
    }

    fn hsstell(&self) -> i32 {
        self.totbit
    }

    fn hputbuf(&mut self, val: u8) {
        let ofs = self.offset;
        for b in 0..8 {
            self.buf[(ofs + b) & BUFMASK] = (val >> (7 - b)) & 1;
        }
        self.offset = (ofs + 8) & BUFMASK;
    }

    fn hgetbits(&mut self, mut n: usize) -> i32 {
        if n == 0 {
            return 0;
        }
        self.totbit += n as i32;
        let mut val = 0u32;
        let mut pos = self.buf_byte_idx;
        if pos + n < BUFSIZE {
            while n > 0 {
                val <<= 1;
                val |= u32::from(self.buf[pos] != 0);
                pos += 1;
                n -= 1;
            }
        } else {
            while n > 0 {
                val <<= 1;
                val |= u32::from(self.buf[pos] != 0);
                pos = (pos + 1) & BUFMASK;
                n -= 1;
            }
        }
        self.buf_byte_idx = pos;
        val as i32
    }

    fn hget1bit(&mut self) -> i32 {
        self.totbit += 1;
        let val = self.buf[self.buf_byte_idx];
        self.buf_byte_idx = (self.buf_byte_idx + 1) & BUFMASK;
        i32::from(val != 0)
    }

    fn rewind_n_bits(&mut self, n: i32) {
        self.totbit -= n;
        let n_u = (n as isize).rem_euclid(BUFSIZE as isize) as usize;
        self.buf_byte_idx = (self.buf_byte_idx + BUFSIZE - n_u) & BUFMASK;
    }

    fn rewind_n_bytes(&mut self, n: i32) {
        let bits = n * 8;
        self.totbit -= bits;
        let bits_u = (bits as isize).rem_euclid(BUFSIZE as isize) as usize;
        self.buf_byte_idx = (self.buf_byte_idx + BUFSIZE - bits_u) & BUFMASK;
    }
}

struct FrameReader<'a> {
    data: &'a [u8],
    pos: usize,
    sync_word: Option<(u32, bool)>,
    first_frame: bool,
}

impl<'a> FrameReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        let start = skip_id3v2(data);
        Self {
            data,
            pos: start,
            sync_word: None,
            first_frame: true,
        }
    }

    fn seek_byte(&mut self, byte_pos: usize) {
        self.pos = byte_pos.min(self.data.len());
        self.first_frame = false;
    }

    fn is_valid_sync(&self, hdr: u32, strict: bool) -> bool {
        let ok = if let (true, Some((word, single_ch))) = (strict, self.sync_word) {
            (hdr & 0xFFF8_0C00) == word && (((hdr & 0x0000_00C0) == 0x0000_00C0) == single_ch)
        } else {
            (hdr & 0xFFE0_0000) == 0xFFE0_0000
        };
        if !ok {
            return false;
        }
        ((hdr >> 10) & 3) != 3 && ((hdr >> 17) & 3) != 0 && ((hdr >> 19) & 3) != 1
    }

    fn read_next_raw(&mut self) -> Option<(Mp3Header, &'a [u8])> {
        let strict = self.sync_word.is_some();
        while self.pos + 4 <= self.data.len() {
            let hdr_u32 = u32::from_be_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ]);
            if !self.is_valid_sync(hdr_u32, strict) {
                self.pos += 1;
                continue;
            }
            let Some(header) = Mp3Header::from_u32(hdr_u32) else {
                self.pos += 1;
                continue;
            };
            let end = self.pos + 4 + header.framesize;
            if end > self.data.len() {
                return None;
            }
            // Validate following sync mark on initial sync so garbage isn't mistaken for an MP3 frame
            if self.sync_word.is_none() && end + 4 <= self.data.len() {
                let next_hdr = u32::from_be_bytes([
                    self.data[end],
                    self.data[end + 1],
                    self.data[end + 2],
                    self.data[end + 3],
                ]);
                if !self.is_valid_sync(next_hdr, false) || Mp3Header::from_u32(next_hdr).is_none() {
                    self.pos += 1;
                    continue;
                }
            }
            let frame_slice = &self.data[self.pos + 4..end];
            self.pos = end;

            let word = hdr_u32 & 0xFFF8_0C00;
            self.sync_word = Some((word, (hdr_u32 & 0x0000_00C0) == 0x0000_00C0));
            header.crc_skip_into_pos();
            return Some((header, frame_slice));
        }
        None
    }

    fn read_frame(&mut self) -> Option<(Mp3Header, &'a [u8])> {
        let (mut hdr, slice) = self.read_next_raw()?;
        if self.first_frame {
            self.first_frame = false;
            hdr.parse_vbr(slice);
            if hdr.vbr {
                if let Some((mut audio_hdr, audio_slice)) = self.read_next_raw() {
                    audio_hdr.copy_vbr_from(&hdr);
                    return Some((audio_hdr, audio_slice));
                }
                return None;
            }
        }
        Some((hdr, slice))
    }
}

impl Mp3Header {
    fn crc_skip_into_pos(&self) {
        // CRC bytes (2) sit immediately after the 4-byte header inside frame_slice
    }
}

struct FrameBits<'a> {
    slice: &'a [u8],
    bit_pos: usize,
}

impl<'a> FrameBits<'a> {
    fn new(slice: &'a [u8], skip_crc_bytes: usize) -> Self {
        Self {
            slice,
            bit_pos: skip_crc_bytes * 8,
        }
    }

    fn read_bits(&mut self, n: usize) -> i32 {
        let mut val = 0i32;
        for _ in 0..n {
            let byte_idx = self.bit_pos >> 3;
            let bit_in_byte = 7 - (self.bit_pos & 7);
            let bit = if byte_idx < self.slice.len() {
                (self.slice[byte_idx] >> bit_in_byte) & 1
            } else {
                0
            };
            val = (val << 1) | i32::from(bit);
            self.bit_pos += 1;
        }
        val
    }
}

fn skip_id3v2(data: &[u8]) -> usize {
    if data.len() >= 10 && &data[..3] == b"ID3" {
        let sz = ((data[6] as usize) << 21)
            | ((data[7] as usize) << 14)
            | ((data[8] as usize) << 7)
            | (data[9] as usize);
        let total = sz + 10;
        if total < data.len() {
            return total;
        }
    }
    0
}

pub fn probe_duration_ms(raw: &[u8]) -> i64 {
    let start = skip_id3v2(raw);
    let audio_bytes = raw.len().saturating_sub(start);
    let mut reader = FrameReader::new(raw);
    let Some((hdr, slice)) = reader.read_next_raw() else {
        return 0;
    };
    let mut header = hdr;
    header.parse_vbr(slice);
    header.total_ms(audio_bytes).round() as i64
}

struct SynthesisFilter {
    v1: [f32; 512],
    v2: [f32; 512],
    use_v1: bool,
    actual_write_pos: usize,
    samples: [f32; 32],
    tmp_out: [f32; 32],
}

impl SynthesisFilter {
    fn new() -> Self {
        Self {
            v1: [0.0; 512],
            v2: [0.0; 512],
            use_v1: true,
            actual_write_pos: 15,
            samples: [0.0; 32],
            tmp_out: [0.0; 32],
        }
    }

    fn compute_new_v(&mut self) {
        let s = &self.samples;
        let cos1_64 = (1.0f64 / (2.0 * (std::f64::consts::PI / 64.0).cos())) as f32;
        let cos3_64 = (1.0f64 / (2.0 * (3.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos5_64 = (1.0f64 / (2.0 * (5.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos7_64 = (1.0f64 / (2.0 * (7.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos9_64 = (1.0f64 / (2.0 * (9.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos11_64 = (1.0f64 / (2.0 * (11.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos13_64 = (1.0f64 / (2.0 * (13.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos15_64 = (1.0f64 / (2.0 * (15.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos17_64 = (1.0f64 / (2.0 * (17.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos19_64 = (1.0f64 / (2.0 * (19.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos21_64 = (1.0f64 / (2.0 * (21.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos23_64 = (1.0f64 / (2.0 * (23.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos25_64 = (1.0f64 / (2.0 * (25.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos27_64 = (1.0f64 / (2.0 * (27.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos29_64 = (1.0f64 / (2.0 * (29.0 * std::f64::consts::PI / 64.0).cos())) as f32;
        let cos31_64 = (1.0f64 / (2.0 * (31.0 * std::f64::consts::PI / 64.0).cos())) as f32;

        let cos1_32 = (1.0f64 / (2.0 * (std::f64::consts::PI / 32.0).cos())) as f32;
        let cos3_32 = (1.0f64 / (2.0 * (3.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos5_32 = (1.0f64 / (2.0 * (5.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos7_32 = (1.0f64 / (2.0 * (7.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos9_32 = (1.0f64 / (2.0 * (9.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos11_32 = (1.0f64 / (2.0 * (11.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos13_32 = (1.0f64 / (2.0 * (13.0 * std::f64::consts::PI / 32.0).cos())) as f32;
        let cos15_32 = (1.0f64 / (2.0 * (15.0 * std::f64::consts::PI / 32.0).cos())) as f32;

        let cos1_16 = (1.0f64 / (2.0 * (std::f64::consts::PI / 16.0).cos())) as f32;
        let cos3_16 = (1.0f64 / (2.0 * (3.0 * std::f64::consts::PI / 16.0).cos())) as f32;
        let cos5_16 = (1.0f64 / (2.0 * (5.0 * std::f64::consts::PI / 16.0).cos())) as f32;
        let cos7_16 = (1.0f64 / (2.0 * (7.0 * std::f64::consts::PI / 16.0).cos())) as f32;

        let cos1_8 = (1.0f64 / (2.0 * (std::f64::consts::PI / 8.0).cos())) as f32;
        let cos3_8 = (1.0f64 / (2.0 * (3.0 * std::f64::consts::PI / 8.0).cos())) as f32;
        let cos1_4 = (1.0f64 / (2.0 * (std::f64::consts::PI / 4.0).cos())) as f32;

        let mut p = [0.0f32; 16];
        let mut pp = [0.0f32; 16];
        for i in 0..16 {
            p[i] = s[i] + s[31 - i];
        }
        for i in 0..8 {
            pp[i] = p[i] + p[15 - i];
        }
        let c32 = [
            cos1_32, cos3_32, cos5_32, cos7_32, cos9_32, cos11_32, cos13_32, cos15_32,
        ];
        for i in 0..8 {
            pp[8 + i] = (p[i] - p[15 - i]) * c32[i];
        }
        let c16 = [cos1_16, cos3_16, cos5_16, cos7_16];
        for i in 0..4 {
            p[i] = pp[i] + pp[7 - i];
            p[4 + i] = (pp[i] - pp[7 - i]) * c16[i];
            p[8 + i] = pp[8 + i] + pp[15 - i];
            p[12 + i] = (pp[8 + i] - pp[15 - i]) * c16[i];
        }
        for b in 0..4 {
            let base = b * 4;
            pp[base] = p[base] + p[base + 3];
            pp[base + 1] = p[base + 1] + p[base + 2];
            pp[base + 2] = (p[base] - p[base + 3]) * cos1_8;
            pp[base + 3] = (p[base + 1] - p[base + 2]) * cos3_8;
        }
        for b in 0..8 {
            let base = b * 2;
            p[base] = pp[base] + pp[base + 1];
            p[base + 1] = (pp[base] - pp[base + 1]) * cos1_4;
        }

        let mut nv = [0.0f32; 32];
        nv[12] = p[7];
        nv[4] = p[7] + p[5];
        nv[19] = -nv[4] - p[6];
        nv[27] = -p[6] - p[7] - p[4];
        nv[14] = p[15];
        nv[10] = nv[14] + p[11];
        nv[6] = nv[10] + p[13];
        nv[2] = p[15] + p[13] + p[9];
        nv[17] = -nv[2] - p[14];
        let t1 = -p[14] - p[15] - p[10] - p[11];
        nv[21] = t1 - p[13];
        nv[29] = -p[14] - p[15] - p[12] - p[8];
        nv[25] = t1 - p[12];
        nv[31] = -p[0];
        nv[0] = p[1];
        nv[8] = p[3];
        nv[23] = -nv[8] - p[2];

        let c64 = [
            cos1_64, cos3_64, cos5_64, cos7_64, cos9_64, cos11_64, cos13_64, cos15_64, cos17_64,
            cos19_64, cos21_64, cos23_64, cos25_64, cos27_64, cos29_64, cos31_64,
        ];
        for i in 0..16 {
            p[i] = (s[i] - s[31 - i]) * c64[i];
        }
        for i in 0..8 {
            pp[i] = p[i] + p[15 - i];
            pp[8 + i] = (p[i] - p[15 - i]) * c32[i];
        }
        for i in 0..4 {
            p[i] = pp[i] + pp[7 - i];
            p[4 + i] = (pp[i] - pp[7 - i]) * c16[i];
            p[8 + i] = pp[8 + i] + pp[15 - i];
            p[12 + i] = (pp[8 + i] - pp[15 - i]) * c16[i];
        }
        for b in 0..4 {
            let base = b * 4;
            pp[base] = p[base] + p[base + 3];
            pp[base + 1] = p[base + 1] + p[base + 2];
            pp[base + 2] = (p[base] - p[base + 3]) * cos1_8;
            pp[base + 3] = (p[base + 1] - p[base + 2]) * cos3_8;
        }
        for b in 0..8 {
            let base = b * 2;
            p[base] = pp[base] + pp[base + 1];
            p[base + 1] = (pp[base] - pp[base + 1]) * cos1_4;
        }

        nv[15] = p[15];
        nv[13] = p[15] + p[7];
        nv[11] = nv[13] + p[11];
        nv[5] = nv[11] + p[5] + p[13];
        nv[9] = p[15] + p[11] + p[3];
        nv[7] = nv[9] + p[13];
        let tmp1 = p[13] + p[15] + p[9];
        nv[1] = tmp1 + p[1];
        nv[16] = -nv[1] - p[14];
        nv[3] = tmp1 + p[5] + p[7];
        nv[18] = -nv[3] - p[6] - p[14];
        let tmp1b = -p[10] - p[11] - p[14] - p[15];
        nv[22] = tmp1b - p[13] - p[2] - p[3];
        nv[20] = tmp1b - p[13] - p[5] - p[6] - p[7];
        nv[24] = tmp1b - p[12] - p[2] - p[3];
        let tmp2 = p[4] + p[6] + p[7];
        nv[26] = tmp1b - p[12] - tmp2;
        let tmp1c = -p[8] - p[12] - p[14] - p[15];
        nv[30] = tmp1c - p[0];
        nv[28] = tmp1c - tmp2;

        let pos = self.actual_write_pos;
        let (dest_cur, dest_other) = if self.use_v1 {
            (&mut self.v1, &mut self.v2)
        } else {
            (&mut self.v2, &mut self.v1)
        };
        for i in 0..16 {
            dest_cur[i * 16 + pos] = nv[i];
        }
        dest_cur[256 + pos] = 0.0;
        for i in 1..16 {
            dest_cur[(16 + i) * 16 + pos] = -nv[16 - i];
        }

        dest_other[pos] = -nv[0];
        for i in 0..16 {
            dest_other[(i + 1) * 16 + pos] = nv[16 + i];
        }
        for i in 0..15 {
            dest_other[(17 + i) * 16 + pos] = nv[30 - i];
        }
    }

    fn compute_pcm_samples(
        &mut self,
        channel: usize,
        channels: usize,
        out: &mut [i16],
        write_pos: &mut usize,
    ) {
        let vp = if self.use_v1 { &self.v1 } else { &self.v2 };
        let wpos = self.actual_write_pos;
        const SCALE: f32 = 32768.0;
        for i in 0..32 {
            let dvp = i * 16;
            let dp = &SFD[dvp..dvp + 16];
            let mut acc = 0.0f32;
            for (k, &d) in dp.iter().enumerate() {
                let idx = (wpos + 16 - k) & 15;
                acc += vp[dvp + idx] * d;
            }
            let s = (acc * SCALE).clamp(-32767.0, 32767.0) as i16;
            out[*write_pos] = s;
            *write_pos += channels;
        }
        let _ = channel;
        self.tmp_out.fill(0.0);
    }

    fn synthesize_32(
        &mut self,
        subband_samples: &[f32; 32],
        channel: usize,
        channels: usize,
        out: &mut [i16],
        write_pos: &mut usize,
    ) {
        self.samples.copy_from_slice(subband_samples);
        self.compute_new_v();
        self.compute_pcm_samples(channel, channels, out, write_pos);
        self.actual_write_pos = (self.actual_write_pos + 1) & 15;
        self.use_v1 = !self.use_v1;
        self.samples.fill(0.0);
    }
}

#[derive(Default, Clone)]
struct GrInfo {
    part2_3_length: i32,
    big_values: i32,
    global_gain: i32,
    scalefac_compress: i32,
    window_switching_flag: i32,
    block_type: i32,
    mixed_block_flag: i32,
    table_select: [usize; 3],
    subblock_gain: [i32; 3],
    region0_count: usize,
    region1_count: usize,
    preflag: i32,
    scalefac_scale: i32,
    count1_table_select: usize,
}

#[derive(Default, Clone)]
struct SideInfo {
    main_data_begin: i32,
    scfsi: [[i32; 4]; 2],
    gr: [[GrInfo; 2]; 2],
}

#[derive(Default, Clone)]
struct ScaleFac {
    l: [i32; 23],
    s: [[i32; 13]; 3],
}

struct LayerIIIDecoder {
    filter1: SynthesisFilter,
    filter2: SynthesisFilter,
    br: BitReserve,
    frame_start: i32,
    part2_start: i32,
    si: SideInfo,
    scalefac: [ScaleFac; 2],
    scale_fac_buffer: [i32; 54],
    is1d: [i32; SBLIMIT * SSLIMIT + 4],
    ro: [[[f32; SSLIMIT]; SBLIMIT]; 2],
    lr: [[[f32; SSLIMIT]; SBLIMIT]; 2],
    out1d: [f32; SBLIMIT * SSLIMIT],
    prev_block: [[f32; SBLIMIT * SSLIMIT]; 2],
    k: [[f32; SBLIMIT * SSLIMIT]; 2],
    nonzero: [usize; 2],
    is_pos: [i32; 576],
    is_ratio: [f32; 576],
}

impl LayerIIIDecoder {
    fn new() -> Self {
        Self {
            filter1: SynthesisFilter::new(),
            filter2: SynthesisFilter::new(),
            br: BitReserve::new(),
            frame_start: 0,
            part2_start: 0,
            si: SideInfo::default(),
            scalefac: [ScaleFac::default(), ScaleFac::default()],
            scale_fac_buffer: [0; 54],
            is1d: [0; SBLIMIT * SSLIMIT + 4],
            ro: [[[0.0; SSLIMIT]; SBLIMIT]; 2],
            lr: [[[0.0; SSLIMIT]; SBLIMIT]; 2],
            out1d: [0.0; SBLIMIT * SSLIMIT],
            prev_block: [[0.0; SBLIMIT * SSLIMIT]; 2],
            k: [[0.0; SBLIMIT * SSLIMIT]; 2],
            nonzero: [0; 2],
            is_pos: [0; 576],
            is_ratio: [0.0; 576],
        }
    }

    fn seek_notify(&mut self) {
        self.frame_start = 0;
        self.prev_block = [[0.0; SBLIMIT * SSLIMIT]; 2];
        self.br = BitReserve::new();
    }

    fn decode_frame(&mut self, hdr: &Mp3Header, frame_slice: &[u8]) -> Vec<i16> {
        let channels = hdr.channels();
        let sfreq = hdr.sample_sfreq_index();
        let max_gr = if hdr.version == MPEG1 { 2 } else { 1 };
        let crc_skip = if hdr.protection_bit == 0 { 2 } else { 0 };
        let mut bits = FrameBits::new(frame_slice, crc_skip);

        if !self.read_side_info(hdr, channels, &mut bits) {
            return Vec::new();
        }
        for _ in 0..hdr.n_slots {
            self.br.hputbuf(bits.read_bits(8) as u8);
        }

        let mut main_data_end = self.br.hsstell() >> 3;
        let flush_main = self.br.hsstell() & 7;
        if flush_main != 0 {
            self.br.hgetbits((8 - flush_main) as usize);
            main_data_end += 1;
        }
        let mut bytes_to_discard = self.frame_start - main_data_end - self.si.main_data_begin;
        self.frame_start += hdr.n_slots as i32;
        if bytes_to_discard < 0 {
            return Vec::new();
        }
        if main_data_end > 4096 {
            self.frame_start -= 4096;
            self.br.rewind_n_bytes(4096);
        }
        while bytes_to_discard > 0 {
            self.br.hgetbits(8);
            bytes_to_discard -= 1;
        }

        let total_samples_per_ch = max_gr * 576;
        let mut pcm = vec![0i16; total_samples_per_ch * channels];
        let mut write_pos = [0usize, 1usize];

        for gr in 0..max_gr {
            for ch in 0..channels {
                self.part2_start = self.br.hsstell();
                if hdr.version == MPEG1 {
                    self.get_scale_factors(ch, gr);
                } else {
                    self.get_lsf_scale_factors(hdr, ch, gr);
                }
                self.huffman_decode(ch, gr, sfreq);
                self.dequantize_sample(ch, gr, sfreq);
            }
            self.stereo(hdr, channels, gr, sfreq);

            for ch in 0..channels {
                self.reorder(ch, gr, sfreq);
                self.antialias(ch, gr);
                self.hybrid(ch, gr);

                for sb18 in (18..576).step_by(36) {
                    for ss in (1..SSLIMIT).step_by(2) {
                        self.out1d[sb18 + ss] = -self.out1d[sb18 + ss];
                    }
                }
                for ss in 0..SSLIMIT {
                    let mut sub = [0.0f32; 32];
                    for (sb, slot) in sub.iter_mut().enumerate() {
                        *slot = self.out1d[sb * 18 + ss];
                    }
                    if ch == 0 {
                        self.filter1
                            .synthesize_32(&sub, ch, channels, &mut pcm, &mut write_pos[0]);
                    } else {
                        self.filter2
                            .synthesize_32(&sub, ch, channels, &mut pcm, &mut write_pos[1]);
                    }
                }
            }
        }
        pcm
    }

    fn read_side_info(
        &mut self,
        hdr: &Mp3Header,
        channels: usize,
        bits: &mut FrameBits<'_>,
    ) -> bool {
        if hdr.version == MPEG1 {
            self.si.main_data_begin = bits.read_bits(9);
            let _priv = bits.read_bits(if channels == 1 { 5 } else { 3 });
            for ch in 0..channels {
                for b in 0..4 {
                    self.si.scfsi[ch][b] = bits.read_bits(1);
                }
            }
            for gr in 0..2 {
                for ch in 0..channels {
                    let g = &mut self.si.gr[ch][gr];
                    g.part2_3_length = bits.read_bits(12);
                    g.big_values = bits.read_bits(9).min(288);
                    g.global_gain = bits.read_bits(8);
                    g.scalefac_compress = bits.read_bits(4);
                    g.window_switching_flag = bits.read_bits(1);
                    if g.window_switching_flag != 0 {
                        g.block_type = bits.read_bits(2);
                        g.mixed_block_flag = bits.read_bits(1);
                        g.table_select[0] = bits.read_bits(5) as usize;
                        g.table_select[1] = bits.read_bits(5) as usize;
                        g.subblock_gain[0] = bits.read_bits(3);
                        g.subblock_gain[1] = bits.read_bits(3);
                        g.subblock_gain[2] = bits.read_bits(3);
                        if g.block_type == 0 {
                            return false;
                        }
                        g.region0_count = if g.block_type == 2 && g.mixed_block_flag == 0 {
                            8
                        } else {
                            7
                        };
                        g.region1_count = 20 - g.region0_count;
                    } else {
                        g.table_select[0] = bits.read_bits(5) as usize;
                        g.table_select[1] = bits.read_bits(5) as usize;
                        g.table_select[2] = bits.read_bits(5) as usize;
                        g.region0_count = bits.read_bits(4) as usize;
                        g.region1_count = bits.read_bits(3) as usize;
                        g.block_type = 0;
                        g.mixed_block_flag = 0;
                    }
                    g.preflag = bits.read_bits(1);
                    g.scalefac_scale = bits.read_bits(1);
                    g.count1_table_select = bits.read_bits(1) as usize;
                }
            }
        } else {
            self.si.main_data_begin = bits.read_bits(8);
            let _priv = bits.read_bits(if channels == 1 { 1 } else { 2 });
            for ch in 0..channels {
                let g = &mut self.si.gr[ch][0];
                g.part2_3_length = bits.read_bits(12);
                g.big_values = bits.read_bits(9).min(288);
                g.global_gain = bits.read_bits(8);
                g.scalefac_compress = bits.read_bits(9);
                g.window_switching_flag = bits.read_bits(1);
                if g.window_switching_flag != 0 {
                    g.block_type = bits.read_bits(2);
                    g.mixed_block_flag = bits.read_bits(1);
                    g.table_select[0] = bits.read_bits(5) as usize;
                    g.table_select[1] = bits.read_bits(5) as usize;
                    g.subblock_gain[0] = bits.read_bits(3);
                    g.subblock_gain[1] = bits.read_bits(3);
                    g.subblock_gain[2] = bits.read_bits(3);
                    if g.block_type == 0 {
                        return false;
                    }
                    g.region0_count = if g.block_type == 2 && g.mixed_block_flag == 0 {
                        8
                    } else {
                        7
                    };
                    g.region1_count = 20 - g.region0_count;
                } else {
                    g.table_select[0] = bits.read_bits(5) as usize;
                    g.table_select[1] = bits.read_bits(5) as usize;
                    g.table_select[2] = bits.read_bits(5) as usize;
                    g.region0_count = bits.read_bits(4) as usize;
                    g.region1_count = bits.read_bits(3) as usize;
                    g.block_type = 0;
                    g.mixed_block_flag = 0;
                }
                g.preflag = 0;
                g.scalefac_scale = bits.read_bits(1);
                g.count1_table_select = bits.read_bits(1) as usize;
            }
        }
        true
    }

    fn get_scale_factors(&mut self, ch: usize, gr: usize) {
        let g = &self.si.gr[ch][gr];
        let comp = g.scalefac_compress as usize;
        let len0 = S_LEN[0][comp] as usize;
        let len1 = S_LEN[1][comp] as usize;
        if g.window_switching_flag != 0 && g.block_type == 2 {
            if g.mixed_block_flag != 0 {
                for sfb in 0..8 {
                    self.scalefac[ch].l[sfb] = self.br.hgetbits(len0);
                }
                for sfb in 3..6 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.br.hgetbits(len0);
                    }
                }
                for sfb in 6..12 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.br.hgetbits(len1);
                    }
                }
                for w in 0..3 {
                    self.scalefac[ch].s[w][12] = 0;
                }
            } else {
                for sfb in 0..6 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.br.hgetbits(len0);
                    }
                }
                for sfb in 6..12 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.br.hgetbits(len1);
                    }
                }
                for w in 0..3 {
                    self.scalefac[ch].s[w][12] = 0;
                }
            }
        } else {
            if self.si.scfsi[ch][0] == 0 || gr == 0 {
                for sfb in 0..6 {
                    self.scalefac[ch].l[sfb] = self.br.hgetbits(len0);
                }
            }
            if self.si.scfsi[ch][1] == 0 || gr == 0 {
                for sfb in 6..11 {
                    self.scalefac[ch].l[sfb] = self.br.hgetbits(len0);
                }
            }
            if self.si.scfsi[ch][2] == 0 || gr == 0 {
                for sfb in 11..16 {
                    self.scalefac[ch].l[sfb] = self.br.hgetbits(len1);
                }
            }
            if self.si.scfsi[ch][3] == 0 || gr == 0 {
                for sfb in 16..21 {
                    self.scalefac[ch].l[sfb] = self.br.hgetbits(len1);
                }
            }
            self.scalefac[ch].l[21] = 0;
            self.scalefac[ch].l[22] = 0;
        }
    }

    fn get_lsf_scale_factors(&mut self, hdr: &Mp3Header, ch: usize, gr: usize) {
        let mode_ext = hdr.mode_extension;
        let scalefac_comp = self.si.gr[ch][gr].scalefac_compress;
        let block_type_num = if self.si.gr[ch][gr].block_type == 2 {
            if self.si.gr[ch][gr].mixed_block_flag != 0 {
                2
            } else {
                1
            }
        } else {
            0
        };
        let mut new_slen = [0i32; 4];
        let mut block_number = 0usize;

        if !((mode_ext == 1 || mode_ext == 3) && ch == 1) {
            if scalefac_comp < 400 {
                new_slen[0] = (scalefac_comp >> 4) / 5;
                new_slen[1] = (scalefac_comp >> 4) % 5;
                new_slen[2] = (scalefac_comp & 0xF) >> 2;
                new_slen[3] = scalefac_comp & 3;
                self.si.gr[ch][gr].preflag = 0;
                block_number = 0;
            } else if scalefac_comp < 500 {
                new_slen[0] = ((scalefac_comp - 400) >> 2) / 5;
                new_slen[1] = ((scalefac_comp - 400) >> 2) % 5;
                new_slen[2] = (scalefac_comp - 400) & 3;
                self.si.gr[ch][gr].preflag = 0;
                block_number = 1;
            } else if scalefac_comp < 512 {
                new_slen[0] = (scalefac_comp - 500) / 3;
                new_slen[1] = (scalefac_comp - 500) % 3;
                self.si.gr[ch][gr].preflag = 1;
                block_number = 2;
            }
        } else {
            let ic = scalefac_comp >> 1;
            if ic < 180 {
                new_slen[0] = ic / 36;
                new_slen[1] = (ic % 36) / 6;
                new_slen[2] = (ic % 36) % 6;
                self.si.gr[ch][gr].preflag = 0;
                block_number = 3;
            } else if ic < 244 {
                new_slen[0] = ((ic - 180) & 0x3F) >> 4;
                new_slen[1] = ((ic - 180) & 0xF) >> 2;
                new_slen[2] = (ic - 180) & 3;
                self.si.gr[ch][gr].preflag = 0;
                block_number = 4;
            } else if ic < 255 {
                new_slen[0] = (ic - 244) / 3;
                new_slen[1] = (ic - 244) % 3;
                self.si.gr[ch][gr].preflag = 0;
                block_number = 5;
            }
        }

        self.scale_fac_buffer[..45].fill(0);
        let mut m = 0usize;
        for i in 0..4 {
            let count = NR_OF_SFB_BLOCK[block_number][block_type_num][i];
            for _ in 0..count {
                if m < self.scale_fac_buffer.len() {
                    self.scale_fac_buffer[m] = if new_slen[i] == 0 {
                        0
                    } else {
                        self.br.hgetbits(new_slen[i] as usize)
                    };
                }
                m += 1;
            }
        }

        let g = &self.si.gr[ch][gr];
        let mut m = 0usize;
        if g.window_switching_flag != 0 && g.block_type == 2 {
            if g.mixed_block_flag != 0 {
                for sfb in 0..8 {
                    self.scalefac[ch].l[sfb] = self.scale_fac_buffer[m];
                    m += 1;
                }
                for sfb in 3..12 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.scale_fac_buffer[m];
                        m += 1;
                    }
                }
            } else {
                for sfb in 0..12 {
                    for w in 0..3 {
                        self.scalefac[ch].s[w][sfb] = self.scale_fac_buffer[m];
                        m += 1;
                    }
                }
            }
            for w in 0..3 {
                self.scalefac[ch].s[w][12] = 0;
            }
        } else {
            for sfb in 0..21 {
                self.scalefac[ch].l[sfb] = self.scale_fac_buffer[m];
                m += 1;
            }
            self.scalefac[ch].l[21] = 0;
            self.scalefac[ch].l[22] = 0;
        }
    }

    fn huffman_symbol(table_idx: usize, br: &mut BitReserve) -> [i32; 4] {
        const MXOFF: i16 = 250;
        let h = &HUFF_TABS[table_idx.min(33)];
        if h.treelen == 0 {
            return [0; 4];
        }
        let mut point = 0usize;
        let mut level = 1u32 << 31;
        let mut x = 0i32;
        let mut y = 0i32;
        loop {
            if point >= h.val.len() {
                break;
            }
            if h.val[point][0] == 0 {
                x = i32::from(h.val[point][1] >> 4);
                y = i32::from(h.val[point][1] & 0xF);
                break;
            }
            let step = if br.hget1bit() != 0 {
                let mut p = point;
                while h.val[p][1] >= MXOFF {
                    p = (p as isize + h.val[p][1] as isize).max(0) as usize;
                    if p >= h.val.len() {
                        return [0; 4];
                    }
                }
                p as isize + h.val[p][1] as isize
            } else {
                let mut p = point;
                while h.val[p][0] >= MXOFF {
                    p = (p as isize + h.val[p][0] as isize).max(0) as usize;
                    if p >= h.val.len() {
                        return [0; 4];
                    }
                }
                p as isize + h.val[p][0] as isize
            };
            if step < 0 {
                break;
            }
            point = step as usize;
            level >>= 1;
            if level == 0 {
                break;
            }
        }
        if table_idx >= 32 {
            let mut v = (y >> 3) & 1;
            let mut w = (y >> 2) & 1;
            let mut x4 = (y >> 1) & 1;
            let mut y4 = y & 1;
            if v != 0 && br.hget1bit() != 0 {
                v = -v;
            }
            if w != 0 && br.hget1bit() != 0 {
                w = -w;
            }
            if x4 != 0 && br.hget1bit() != 0 {
                x4 = -x4;
            }
            if y4 != 0 && br.hget1bit() != 0 {
                y4 = -y4;
            }
            [v, w, x4, y4]
        } else {
            if h.linbits != 0 && (h.xlen - 1) == x {
                x += br.hgetbits(h.linbits as usize);
            }
            if x != 0 && br.hget1bit() != 0 {
                x = -x;
            }
            if h.linbits != 0 && (h.ylen - 1) == y {
                y += br.hgetbits(h.linbits as usize);
            }
            if y != 0 && br.hget1bit() != 0 {
                y = -y;
            }
            [x, y, 0, 0]
        }
    }

    fn huffman_decode(&mut self, ch: usize, gr: usize, sfreq: usize) {
        let g = self.si.gr[ch][gr].clone();
        let part2_3_end = self.part2_start + g.part2_3_length;
        let (region1_start, region2_start) = if g.window_switching_flag != 0 && g.block_type == 2 {
            (if sfreq == 8 { 72 } else { 36 }, 576usize)
        } else {
            let buf = g.region0_count + 1;
            let buf1 = (buf + g.region1_count + 1).min(SF_BAND_INDEX[sfreq].l.len() - 1);
            (
                SF_BAND_INDEX[sfreq].l[buf.min(22)],
                SF_BAND_INDEX[sfreq].l[buf1],
            )
        };

        let mut index = 0usize;
        let big_end = (g.big_values.max(0) as usize * 2).min(576);
        let mut i = 0usize;
        while i < big_end && index + 1 < 576 {
            let tbl = if i < region1_start {
                g.table_select[0]
            } else if i < region2_start {
                g.table_select[1]
            } else {
                g.table_select[2]
            };
            let xy = Self::huffman_symbol(tbl, &mut self.br);
            self.is1d[index] = xy[0];
            self.is1d[index + 1] = xy[1];
            index += 2;
            i += 2;
        }

        let count1_tbl = g.count1_table_select + 32;
        let mut num_bits = self.br.hsstell();
        while num_bits < part2_3_end && index < 576 {
            let vwxy = Self::huffman_symbol(count1_tbl, &mut self.br);
            for (k, &val) in vwxy.iter().enumerate() {
                if index + k < self.is1d.len() {
                    self.is1d[index + k] = val;
                }
            }
            index += 4;
            num_bits = self.br.hsstell();
        }
        if num_bits > part2_3_end {
            self.br.rewind_n_bits(num_bits - part2_3_end);
            index = index.saturating_sub(4);
        }
        num_bits = self.br.hsstell();
        if num_bits < part2_3_end {
            self.br.hgetbits((part2_3_end - num_bits) as usize);
        }
        let nz = index.min(576);
        self.nonzero[ch] = nz;
        self.is1d[nz..576].fill(0);
    }

    fn dequantize_sample(&mut self, ch: usize, gr: usize, sfreq: usize) {
        let g = &self.si.gr[ch][gr];
        let sbi = &SF_BAND_INDEX[sfreq];
        let mut cb = 0usize;
        let mut cb_begin = 0usize;
        let mut cb_width = 0usize;
        let mut next_cb_boundary = if g.window_switching_flag != 0 && g.block_type == 2 {
            if g.mixed_block_flag != 0 {
                sbi.l[1]
            } else {
                cb_width = sbi.s[1];
                (cb_width << 2) - cb_width
            }
        } else {
            sbi.l[1]
        };

        let g_gain = 2.0f64.powf(0.25 * (g.global_gain as f64 - 210.0)) as f32;
        let nz = self.nonzero[ch];
        let t43 = &*T_43;
        for j in 0..nz {
            let quot = j / SSLIMIT;
            let rem = j % SSLIMIT;
            let v = self.is1d[j];
            self.ro[ch][quot][rem] = if v == 0 {
                0.0
            } else {
                let abv = v.unsigned_abs() as usize;
                let base = if abv < t43.len() {
                    t43[abv]
                } else {
                    (abv as f64).powf(D43) as f32
                };
                if v > 0 { g_gain * base } else { -g_gain * base }
            };
        }

        for (index, j) in (0..nz).enumerate() {
            let quot = j / SSLIMIT;
            let rem = j % SSLIMIT;
            if index == next_cb_boundary {
                if g.window_switching_flag != 0 && g.block_type == 2 {
                    if g.mixed_block_flag != 0 {
                        if index == sbi.l[8] {
                            next_cb_boundary = (sbi.s[4] << 2) - sbi.s[4];
                            cb = 3;
                            cb_width = sbi.s[4] - sbi.s[3];
                            cb_begin = (sbi.s[3] << 2) - sbi.s[3];
                        } else if index < sbi.l[8] {
                            cb += 1;
                            next_cb_boundary = sbi.l[(cb + 1).min(22)];
                        } else {
                            cb += 1;
                            next_cb_boundary =
                                (sbi.s[(cb + 1).min(13)] << 2) - sbi.s[(cb + 1).min(13)];
                            cb_begin = sbi.s[cb.min(12)];
                            cb_width = sbi.s[(cb + 1).min(13)] - cb_begin;
                            cb_begin = (cb_begin << 2) - cb_begin;
                        }
                    } else {
                        cb += 1;
                        next_cb_boundary = (sbi.s[(cb + 1).min(13)] << 2) - sbi.s[(cb + 1).min(13)];
                        cb_begin = sbi.s[cb.min(12)];
                        cb_width = sbi.s[(cb + 1).min(13)] - cb_begin;
                        cb_begin = (cb_begin << 2) - cb_begin;
                    }
                } else {
                    cb += 1;
                    next_cb_boundary = sbi.l[(cb + 1).min(22)];
                }
            }

            let is_short = g.window_switching_flag != 0
                && g.block_type == 2
                && (g.mixed_block_flag == 0 || j >= 36);
            if is_short && cb_width > 0 && cb < 13 {
                let t_index = ((index - cb_begin) / cb_width).min(2);
                let idx = ((self.scalefac[ch].s[t_index][cb] << g.scalefac_scale)
                    + (g.subblock_gain[t_index] << 2))
                    .clamp(0, 63) as usize;
                self.ro[ch][quot][rem] *= TWO_TO_NEGATIVE_HALF_POW[idx];
            } else if cb < 22 {
                let mut idx = self.scalefac[ch].l[cb];
                if g.preflag != 0 {
                    idx += PRE_TAB[cb] as i32;
                }
                let idx = (idx << g.scalefac_scale).clamp(0, 63) as usize;
                self.ro[ch][quot][rem] *= TWO_TO_NEGATIVE_HALF_POW[idx];
            }
        }

        for j in nz..576 {
            self.ro[ch][j / SSLIMIT][j % SSLIMIT] = 0.0;
        }
    }

    fn i_stereo_k_values(&mut self, is_pos: i32, io_type: usize, i: usize) {
        if is_pos <= 0 {
            self.k[0][i] = 1.0;
            self.k[1][i] = 1.0;
        } else if (is_pos & 1) != 0 {
            let idx = ((is_pos + 1) >> 1).min(31) as usize;
            self.k[0][i] = IO[io_type][idx];
            self.k[1][i] = 1.0;
        } else {
            let idx = (is_pos >> 1).min(31) as usize;
            self.k[0][i] = 1.0;
            self.k[1][i] = IO[io_type][idx];
        }
    }

    fn stereo(&mut self, hdr: &Mp3Header, channels: usize, gr: usize, sfreq: usize) {
        if channels == 1 {
            self.lr[0] = self.ro[0];
            return;
        }
        let g = self.si.gr[0][gr].clone();
        let mode_ext = hdr.mode_extension;
        let lsf = hdr.version == MPEG2_LSF || hdr.version == MPEG25_LSF;
        let io_type = (g.scalefac_compress & 1) as usize;
        let ms_stereo = hdr.mode == JOINT_STEREO && (mode_ext & 0x2) != 0;
        let i_stereo = hdr.mode == JOINT_STEREO && (mode_ext & 0x1) != 0;
        self.is_pos.fill(7);

        if i_stereo {
            let sbi = &SF_BAND_INDEX[sfreq];
            if g.window_switching_flag != 0 && g.block_type == 2 {
                if g.mixed_block_flag != 0 {
                    let mut max_sfb = 0usize;
                    for j in 0..3 {
                        let mut sfbcnt = -1i32;
                        for sfb in (3..=12).rev() {
                            let mut l = sbi.s[sfb + 1] - sbi.s[sfb];
                            let mut idx = (sbi.s[sfb] * 3) + (j + 1) * l - 1;
                            while l > 0 {
                                if self.ro[1][idx / 18][idx % 18] != 0.0 {
                                    sfbcnt = sfb as i32;
                                    break;
                                }
                                l -= 1;
                                idx = idx.saturating_sub(1);
                            }
                            if sfbcnt >= 0 {
                                break;
                            }
                        }
                        for sfb in ((sfbcnt + 1) as usize)..12 {
                            let lines = sbi.s[sfb + 1] - sbi.s[sfb];
                            let base = sbi.s[sfb] * 3 + j * lines;
                            for i in base..base + lines {
                                let p = self.scalefac[1].s[j][sfb];
                                self.is_pos[i] = p;
                                if p != 7 {
                                    if lsf {
                                        self.i_stereo_k_values(p, io_type, i);
                                    } else {
                                        self.is_ratio[i] = TAN12[p.clamp(0, 15) as usize];
                                    }
                                }
                            }
                        }
                        if sfbcnt > max_sfb as i32 {
                            max_sfb = sfbcnt as usize;
                        }
                    }
                    if max_sfb <= 3 {
                        let mut i = 2usize;
                        let mut ss = 17isize;
                        let mut sb = 0usize;
                        while i > 0 || ss >= 0 {
                            if self.ro[1][i][ss as usize] != 0.0 {
                                sb = i * 18 + ss as usize;
                                break;
                            }
                            ss -= 1;
                            if ss < 0 {
                                if i == 0 {
                                    break;
                                }
                                i -= 1;
                                ss = 17;
                            }
                        }
                        let mut sfb = 0usize;
                        while sfb < 21 && sbi.l[sfb] <= sb {
                            sfb += 1;
                        }
                        let mut idx = sbi.l[sfb];
                        while sfb < 8 {
                            let lines = sbi.l[sfb + 1] - sbi.l[sfb];
                            for _ in 0..lines {
                                let p = self.scalefac[1].l[sfb];
                                self.is_pos[idx] = p;
                                if p != 7 {
                                    if lsf {
                                        self.i_stereo_k_values(p, io_type, idx);
                                    } else {
                                        self.is_ratio[idx] = TAN12[p.clamp(0, 15) as usize];
                                    }
                                }
                                idx += 1;
                            }
                            sfb += 1;
                        }
                    }
                } else {
                    for j in 0..3 {
                        let mut sfbcnt = -1i32;
                        for sfb in (0..=12).rev() {
                            let mut l = sbi.s[sfb + 1] - sbi.s[sfb];
                            let mut idx = sbi.s[sfb] * 3 + (j + 1) * l - 1;
                            while l > 0 {
                                if self.ro[1][idx / 18][idx % 18] != 0.0 {
                                    sfbcnt = sfb as i32;
                                    break;
                                }
                                l -= 1;
                                idx = idx.saturating_sub(1);
                            }
                            if sfbcnt >= 0 {
                                break;
                            }
                        }
                        for sfb in ((sfbcnt + 1) as usize)..12 {
                            let lines = sbi.s[sfb + 1] - sbi.s[sfb];
                            let base = sbi.s[sfb] * 3 + j * lines;
                            for i in base..base + lines {
                                let p = self.scalefac[1].s[j][sfb];
                                self.is_pos[i] = p;
                                if p != 7 {
                                    if lsf {
                                        self.i_stereo_k_values(p, io_type, i);
                                    } else {
                                        self.is_ratio[i] = TAN12[p.clamp(0, 15) as usize];
                                    }
                                }
                            }
                        }
                        let t10 = sbi.s[10];
                        let t11 = sbi.s[11];
                        let sfb_idx = t10 * 3 + j * (t11 - t10);
                        let base = t11 * 3 + j * (sbi.s[12] - t11);
                        for i in base..base + (sbi.s[12] - t11) {
                            if i < 576 && sfb_idx < 576 {
                                self.is_pos[i] = self.is_pos[sfb_idx];
                                if lsf {
                                    self.k[0][i] = self.k[0][sfb_idx];
                                    self.k[1][i] = self.k[1][sfb_idx];
                                } else {
                                    self.is_ratio[i] = self.is_ratio[sfb_idx];
                                }
                            }
                        }
                    }
                }
            } else {
                let mut sb = 0usize;
                'outer: for i in (0..32).rev() {
                    for ss in (0..18).rev() {
                        if self.ro[1][i][ss] != 0.0 {
                            sb = i * 18 + ss;
                            break 'outer;
                        }
                    }
                }
                let mut sfb = 0usize;
                while sfb < 21 && sbi.l[sfb] <= sb {
                    sfb += 1;
                }
                let mut i = sbi.l[sfb];
                while sfb < 21 {
                    let lines = sbi.l[sfb + 1] - sbi.l[sfb];
                    for _ in 0..lines {
                        if i < 576 {
                            let p = self.scalefac[1].l[sfb];
                            self.is_pos[i] = p;
                            if p != 7 {
                                if lsf {
                                    self.i_stereo_k_values(p, io_type, i);
                                } else {
                                    self.is_ratio[i] = TAN12[p.clamp(0, 15) as usize];
                                }
                            }
                        }
                        i += 1;
                    }
                    sfb += 1;
                }
                let sfb20 = sbi.l[20].min(575);
                for _ in 0..576usize.saturating_sub(sbi.l[21]) {
                    if i >= 576 {
                        break;
                    }
                    self.is_pos[i] = self.is_pos[sfb20];
                    if lsf {
                        self.k[0][i] = self.k[0][sfb20];
                        self.k[1][i] = self.k[1][sfb20];
                    } else {
                        self.is_ratio[i] = self.is_ratio[sfb20];
                    }
                    i += 1;
                }
            }
        }

        let mut i = 0usize;
        for sb in 0..SBLIMIT {
            for ss in 0..SSLIMIT {
                if self.is_pos[i] == 7 {
                    if ms_stereo {
                        let a = self.ro[0][sb][ss];
                        let b = self.ro[1][sb][ss];
                        self.lr[0][sb][ss] = (a + b) * 0.707_106_77;
                        self.lr[1][sb][ss] = (a - b) * 0.707_106_77;
                    } else {
                        self.lr[0][sb][ss] = self.ro[0][sb][ss];
                        self.lr[1][sb][ss] = self.ro[1][sb][ss];
                    }
                } else if i_stereo {
                    if lsf {
                        self.lr[0][sb][ss] = self.ro[0][sb][ss] * self.k[0][i];
                        self.lr[1][sb][ss] = self.ro[0][sb][ss] * self.k[1][i];
                    } else {
                        let r = self.ro[0][sb][ss] / (1.0 + self.is_ratio[i]);
                        self.lr[1][sb][ss] = r;
                        self.lr[0][sb][ss] = r * self.is_ratio[i];
                    }
                }
                i += 1;
            }
        }
    }

    fn reorder(&mut self, ch: usize, gr: usize, sfreq: usize) {
        let g = &self.si.gr[ch][gr];
        let xr = &self.lr[ch];
        if g.window_switching_flag != 0 && g.block_type == 2 {
            self.out1d.fill(0.0);
            if g.mixed_block_flag != 0 {
                for idx in 0..36 {
                    self.out1d[idx] = xr[idx / SSLIMIT][idx % SSLIMIT];
                }
                let band = &SF_BAND_INDEX[sfreq].s;
                for sfb in 3..13 {
                    let sfb_start = band[sfb];
                    let sfb_lines = band[sfb + 1] - sfb_start;
                    let sfb_start3 = sfb_start * 3;
                    for freq in 0..sfb_lines {
                        let mut src = sfb_start3 + freq;
                        let dst_base = sfb_start3 + freq * 3;
                        for dst in dst_base..dst_base + 3 {
                            if dst < 576 && src < 576 {
                                self.out1d[dst] = xr[src / SSLIMIT][src % SSLIMIT];
                            }
                            src += sfb_lines;
                        }
                    }
                }
            } else {
                let tbl = &REORDER_TABLES[sfreq];
                for (idx, &j) in tbl.iter().enumerate() {
                    self.out1d[idx] = xr[j / SSLIMIT][j % SSLIMIT];
                }
            }
        } else {
            for idx in 0..576 {
                self.out1d[idx] = xr[idx / SSLIMIT][idx % SSLIMIT];
            }
        }
    }

    fn antialias(&mut self, ch: usize, gr: usize) {
        let g = &self.si.gr[ch][gr];
        if g.window_switching_flag != 0 && g.block_type == 2 && g.mixed_block_flag == 0 {
            return;
        }
        let lim = if g.window_switching_flag != 0 && g.mixed_block_flag != 0 && g.block_type == 2 {
            18
        } else {
            558
        };
        for sb18 in (0..lim).step_by(18) {
            for ss in 0..8 {
                let i1 = sb18 + 17 - ss;
                let i2 = sb18 + 18 + ss;
                let bu = self.out1d[i1];
                let bd = self.out1d[i2];
                self.out1d[i1] = bu * CS[ss] - bd * CA[ss];
                self.out1d[i2] = bd * CS[ss] + bu * CA[ss];
            }
        }
    }

    fn hybrid(&mut self, ch: usize, gr: usize) {
        let g = &self.si.gr[ch][gr];
        let mut ts_copy = [0.0f32; 18];
        let mut rawout = [0.0f32; 36];
        for sb18 in (0..576).step_by(18) {
            let bt = if g.window_switching_flag != 0 && g.mixed_block_flag != 0 && sb18 < 36 {
                0
            } else {
                g.block_type as usize
            };
            ts_copy.copy_from_slice(&self.out1d[sb18..sb18 + 18]);
            inv_mdct(&mut ts_copy, &mut rawout, bt);
            self.out1d[sb18..sb18 + 18].copy_from_slice(&ts_copy);
            for i in 0..18 {
                self.out1d[sb18 + i] = rawout[i] + self.prev_block[ch][sb18 + i];
                self.prev_block[ch][sb18 + i] = rawout[18 + i];
            }
        }
    }
}

#[allow(clippy::excessive_precision)]
fn inv_mdct(in_buf: &mut [f32; 18], out: &mut [f32; 36], block_type: usize) {
    let win_bt = &WIN[block_type.min(3)];
    if block_type == 2 {
        out.fill(0.0);
        let mut six_i = 0usize;
        for i in 0..3 {
            in_buf[15 + i] += in_buf[12 + i];
            in_buf[12 + i] += in_buf[9 + i];
            in_buf[9 + i] += in_buf[6 + i];
            in_buf[6 + i] += in_buf[3 + i];
            in_buf[3 + i] += in_buf[i];

            in_buf[15 + i] += in_buf[9 + i];
            in_buf[9 + i] += in_buf[3 + i];

            let mut pp2 = in_buf[12 + i] * 0.500_000_000;
            let mut pp1 = in_buf[6 + i] * 0.866_025_403;
            let mut sum = in_buf[i] + pp2;
            let mut tmpf_1 = in_buf[i] - in_buf[12 + i];
            let mut tmpf_0 = sum + pp1;
            let mut tmpf_2 = sum - pp1;

            pp2 = in_buf[15 + i] * 0.500_000_000;
            pp1 = in_buf[9 + i] * 0.866_025_403;
            sum = in_buf[3 + i] + pp2;
            let mut tmpf_4 = in_buf[3 + i] - in_buf[15 + i];
            let mut tmpf_5 = sum + pp1;
            let mut tmpf_3 = sum - pp1;

            tmpf_3 *= 1.931_851_653;
            tmpf_4 *= std::f32::consts::FRAC_1_SQRT_2;
            tmpf_5 *= 0.517_638_090;

            let mut save = tmpf_0;
            tmpf_0 += tmpf_5;
            tmpf_5 = save - tmpf_5;
            save = tmpf_1;
            tmpf_1 += tmpf_4;
            tmpf_4 = save - tmpf_4;
            save = tmpf_2;
            tmpf_2 += tmpf_3;
            tmpf_3 = save - tmpf_3;

            tmpf_0 *= 0.504_314_480;
            tmpf_1 *= 0.541_196_100;
            tmpf_2 *= 0.630_236_207;
            tmpf_3 *= 0.821_339_815;
            tmpf_4 *= 1.306_562_965;
            tmpf_5 *= 3.830_648_788;

            let tmpf_8 = -tmpf_0 * 0.793_353_340;
            let tmpf_9 = -tmpf_0 * 0.608_761_429;
            let tmpf_7 = -tmpf_1 * 0.923_879_532;
            let tmpf_10 = -tmpf_1 * 0.382_683_432;
            let tmpf_6 = -tmpf_2 * 0.991_444_861;
            let tmpf_11 = -tmpf_2 * 0.130_526_192;

            let tmpf_0_saved = tmpf_3;
            tmpf_1 = tmpf_4 * 0.382_683_432;
            tmpf_2 = tmpf_5 * 0.608_761_429;

            tmpf_3 = -tmpf_5 * 0.793_353_340;
            tmpf_4 = -tmpf_4 * 0.923_879_532;
            tmpf_5 = -tmpf_0_saved * 0.991_444_861;
            tmpf_0 = tmpf_0_saved * 0.130_526_192;

            out[six_i + 6] += tmpf_0;
            out[six_i + 7] += tmpf_1;
            out[six_i + 8] += tmpf_2;
            out[six_i + 9] += tmpf_3;
            out[six_i + 10] += tmpf_4;
            out[six_i + 11] += tmpf_5;
            out[six_i + 12] += tmpf_6;
            out[six_i + 13] += tmpf_7;
            out[six_i + 14] += tmpf_8;
            out[six_i + 15] += tmpf_9;
            out[six_i + 16] += tmpf_10;
            out[six_i + 17] += tmpf_11;

            six_i += 6;
        }
        return;
    }

    for k in (1..=17).rev() {
        in_buf[k] += in_buf[k - 1];
    }
    let mut k = 17usize;
    while k >= 3 {
        in_buf[k] += in_buf[k - 2];
        k -= 2;
    }
    in_buf[1] += 0.0;

    let i00 = in_buf[0] + in_buf[0];
    let iip12 = i00 + in_buf[12];
    let tmp0 =
        iip12 + in_buf[4] * 1.879_385_2 + in_buf[8] * 1.532_088_9 + in_buf[16] * 0.347_296_36;
    let tmp1 = i00 + in_buf[4] - in_buf[8] - in_buf[12] - in_buf[12] - in_buf[16];
    let tmp2 =
        iip12 - in_buf[4] * 0.347_296_36 - in_buf[8] * 1.879_385_2 + in_buf[16] * 1.532_088_9;
    let tmp3 =
        iip12 - in_buf[4] * 1.532_088_9 + in_buf[8] * 0.347_296_36 - in_buf[16] * 1.879_385_2;
    let tmp4 = in_buf[0] - in_buf[4] + in_buf[8] - in_buf[12] + in_buf[16];

    let i66_ = in_buf[6] * 1.732_050_8;
    let tmp0_ =
        in_buf[2] * 1.969_615_5 + i66_ + in_buf[10] * 1.285_575_2 + in_buf[14] * 0.684_040_3;
    let tmp1_ = (in_buf[2] - in_buf[10] - in_buf[14]) * 1.732_050_8;
    let tmp2_ =
        in_buf[2] * 1.285_575_2 - i66_ - in_buf[10] * 0.684_040_3 + in_buf[14] * 1.969_615_5;
    let tmp3_ =
        in_buf[2] * 0.684_040_3 - i66_ + in_buf[10] * 1.969_615_5 - in_buf[14] * 1.285_575_2;

    let i0 = in_buf[1] + in_buf[1];
    let i0p12 = i0 + in_buf[13];
    let tmp0o =
        i0p12 + in_buf[5] * 1.879_385_2 + in_buf[9] * 1.532_088_9 + in_buf[17] * 0.347_296_36;
    let tmp1o = i0 + in_buf[5] - in_buf[9] - in_buf[13] - in_buf[13] - in_buf[17];
    let tmp2o =
        i0p12 - in_buf[5] * 0.347_296_36 - in_buf[9] * 1.879_385_2 + in_buf[17] * 1.532_088_9;
    let tmp3o =
        i0p12 - in_buf[5] * 1.532_088_9 + in_buf[9] * 0.347_296_36 - in_buf[17] * 1.879_385_2;
    let tmp4o = (in_buf[1] - in_buf[5] + in_buf[9] - in_buf[13] + in_buf[17]) * 0.707_106_77;

    let i6_ = in_buf[7] * 1.732_050_8;
    let tmp0_o =
        in_buf[3] * 1.969_615_5 + i6_ + in_buf[11] * 1.285_575_2 + in_buf[15] * 0.684_040_3;
    let tmp1_o = (in_buf[3] - in_buf[11] - in_buf[15]) * 1.732_050_8;
    let tmp2_o =
        in_buf[3] * 1.285_575_2 - i6_ - in_buf[11] * 0.684_040_3 + in_buf[15] * 1.969_615_5;
    let tmp3_o =
        in_buf[3] * 0.684_040_3 - i6_ + in_buf[11] * 1.969_615_5 - in_buf[15] * 1.285_575_2;

    let mut t = [0.0f32; 18];
    let (e, o) = (tmp0 + tmp0_, (tmp0o + tmp0_o) * 0.501_909_9);
    t[0] = e + o;
    t[17] = e - o;
    let (e, o) = (tmp1 + tmp1_, (tmp1o + tmp1_o) * 0.517_638_1);
    t[1] = e + o;
    t[16] = e - o;
    let (e, o) = (tmp2 + tmp2_, (tmp2o + tmp2_o) * 0.551_689_0);
    t[2] = e + o;
    t[15] = e - o;
    let (e, o) = (tmp3 + tmp3_, (tmp3o + tmp3_o) * 0.610_387_3);
    t[3] = e + o;
    t[14] = e - o;
    t[4] = tmp4 + tmp4o;
    t[13] = tmp4 - tmp4o;
    let (e, o) = (tmp3 - tmp3_, (tmp3o - tmp3_o) * 0.871_723_4);
    t[5] = e + o;
    t[12] = e - o;
    let (e, o) = (tmp2 - tmp2_, (tmp2o - tmp2_o) * 1.183_100_8);
    t[6] = e + o;
    t[11] = e - o;
    let (e, o) = (tmp1 - tmp1_, (tmp1o - tmp1_o) * 1.931_851_7);
    t[7] = e + o;
    t[10] = e - o;
    let (e, o) = (tmp0 - tmp0_, (tmp0o - tmp0_o) * 5.736_856_5);
    t[8] = e + o;
    t[9] = e - o;

    for i in 0..9 {
        out[i] = -t[9 + i] * win_bt[i];
        out[9 + i] = t[17 - i] * win_bt[9 + i];
        out[18 + i] = t[8 - i] * win_bt[18 + i];
        out[27 + i] = t[i] * win_bt[27 + i];
    }
}

pub struct Mp3DecodedFrame {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: usize,
    pub ms_per_frame: f64,
}

pub struct Mp3Stream<'a> {
    reader: FrameReader<'a>,
    l3: LayerIIIDecoder,
    trim: Option<GaplessTrim>,
}

impl<'a> Mp3Stream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            reader: FrameReader::new(data),
            l3: LayerIIIDecoder::new(),
            trim: None,
        }
    }

    pub fn skip_to_ms(&mut self, target_ms: u64) -> f64 {
        let mut skipped = 0.0f64;
        while skipped < target_ms as f64 {
            let Some((hdr, _)) = self.reader.read_frame() else {
                break;
            };
            if self.trim.is_none() {
                self.trim = Some(GaplessTrim::from_header(&hdr, hdr.channels()));
            }
            if let Some(ref mut trim) = self.trim {
                let len = SAMPLES_PER_MP3_FRAME as usize * hdr.channels();
                let _ = trim.accept(len);
            }
            skipped += hdr.ms_per_frame();
        }
        self.l3.seek_notify();
        let _ = self.reader.pos;
        skipped
    }

    pub fn seek_near_end_discard(&mut self) {
        self.reader.seek_byte(self.reader.data.len());
    }

    pub fn next_frame(&mut self) -> Option<Mp3DecodedFrame> {
        loop {
            let (hdr, slice) = self.reader.read_frame()?;
            let ch = hdr.channels();
            let sr = hdr.frequency();
            let ms = hdr.ms_per_frame();
            if self.trim.is_none() {
                self.trim = Some(GaplessTrim::from_header(&hdr, ch));
            }
            let raw_pcm = self.l3.decode_frame(&hdr, slice);
            if raw_pcm.is_empty() {
                continue;
            }
            if let Some(ref mut trim) = self.trim {
                let keep = trim.accept(raw_pcm.len());
                if keep == 0 {
                    continue;
                }
                let sliced = raw_pcm[trim.offset..trim.offset + keep].to_vec();
                return Some(Mp3DecodedFrame {
                    samples: sliced,
                    sample_rate: sr,
                    channels: ch,
                    ms_per_frame: ms,
                });
            }
            return Some(Mp3DecodedFrame {
                samples: raw_pcm,
                sample_rate: sr,
                channels: ch,
                ms_per_frame: ms,
            });
        }
    }
}

pub fn decode_all_pcm16(raw: &[u8]) -> Option<(Vec<i16>, usize, u32)> {
    let mut stream = Mp3Stream::new(raw);
    let mut out = Vec::new();
    let mut channels = 0usize;
    let mut sample_rate = 0u32;
    while let Some(frame) = stream.next_frame() {
        if sample_rate == 0 {
            sample_rate = frame.sample_rate;
            channels = frame.channels;
        }
        out.extend_from_slice(&frame.samples);
    }
    if sample_rate == 0 {
        return None;
    }
    Some((out, channels, sample_rate))
}

/// Owned streaming MP3 source with no lifetime bounds (for AudioEngine thread + seek).
pub struct OwnedMp3Source {
    data: Vec<u8>,
    pos: usize,
    sync_word: Option<(u32, bool)>,
    first_frame: bool,
    l3: LayerIIIDecoder,
    trim: Option<GaplessTrim>,
    pending: Vec<i16>,
    cursor: usize,
    pub sample_rate: u32,
    pub channels: usize,
}

impl OwnedMp3Source {
    pub fn open(raw: Vec<u8>, start_ms: u64) -> std::io::Result<Self> {
        let pos = skip_id3v2(&raw);
        let mut s = Self {
            data: raw,
            pos,
            sync_word: None,
            first_frame: true,
            l3: LayerIIIDecoder::new(),
            trim: None,
            pending: Vec::new(),
            cursor: 0,
            sample_rate: 44100,
            channels: 2,
        };
        if start_ms > 0 {
            let mut skipped = 0.0f64;
            while skipped < start_ms as f64 {
                let Some(hdr) = s.skip_one_frame() else {
                    break;
                };
                if s.trim.is_none() {
                    s.trim = Some(GaplessTrim::from_header(&hdr, hdr.channels()));
                }
                if let Some(ref mut trim) = s.trim {
                    let len = SAMPLES_PER_MP3_FRAME as usize * hdr.channels();
                    let _ = trim.accept(len);
                }
                skipped += hdr.ms_per_frame();
            }
            s.l3.seek_notify();
        }
        let _ = s.decode_next_frame();
        if s.pending.is_empty() && s.pos >= s.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid or empty MP3 stream",
            ));
        }
        Ok(s)
    }

    fn skip_one_frame(&mut self) -> Option<Mp3Header> {
        let mut r = FrameReader {
            data: &self.data,
            pos: self.pos,
            sync_word: self.sync_word,
            first_frame: self.first_frame,
        };
        let (hdr, _) = r.read_frame()?;
        self.pos = r.pos;
        self.sync_word = r.sync_word;
        self.first_frame = r.first_frame;
        self.sample_rate = hdr.frequency();
        self.channels = hdr.channels();
        Some(hdr)
    }

    fn decode_next_frame(&mut self) -> bool {
        loop {
            let mut r = FrameReader {
                data: &self.data,
                pos: self.pos,
                sync_word: self.sync_word,
                first_frame: self.first_frame,
            };
            let Some((hdr, slice)) = r.read_frame() else {
                self.pos = r.pos;
                self.pending.clear();
                self.cursor = 0;
                return false;
            };
            self.pos = r.pos;
            self.sync_word = r.sync_word;
            self.first_frame = r.first_frame;

            let ch = hdr.channels();
            let sr = hdr.frequency();
            self.sample_rate = sr;
            self.channels = ch;
            if self.trim.is_none() {
                self.trim = Some(GaplessTrim::from_header(&hdr, ch));
            }
            let raw_pcm = self.l3.decode_frame(&hdr, slice);
            if raw_pcm.is_empty() {
                continue;
            }
            if let Some(ref mut trim) = self.trim {
                let keep = trim.accept(raw_pcm.len());
                if keep == 0 {
                    continue;
                }
                self.pending = raw_pcm[trim.offset..trim.offset + keep].to_vec();
            } else {
                self.pending = raw_pcm;
            }
            self.cursor = 0;
            return true;
        }
    }

    pub fn read_i16(&mut self, out: &mut [i16]) -> usize {
        let mut written = 0usize;
        while written < out.len() {
            if self.cursor >= self.pending.len() && !self.decode_next_frame() {
                break;
            }
            let avail = self.pending.len() - self.cursor;
            let take = avail.min(out.len() - written);
            out[written..written + take]
                .copy_from_slice(&self.pending[self.cursor..self.cursor + take]);
            self.cursor += take;
            written += take;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_synthetic_cbr_mp3_frame() -> Vec<u8> {
        // MPEG-1 Layer III 128 kbps 44100 Hz stereo (0xFF 0xFB 0x90 0x00) => frame len = 417 bytes
        let mut buf = vec![0u8; 417 * 2];
        for i in 0..2 {
            let off = i * 417;
            buf[off] = 0xFF;
            buf[off + 1] = 0xFB;
            buf[off + 2] = 0x90;
            buf[off + 3] = 0x00;
        }
        buf
    }

    #[test]
    fn probes_cbr_and_xing_lame_mp3_and_decodes_silence() {
        let mut raw = build_synthetic_cbr_mp3_frame();
        assert!(probe_duration_ms(&raw) > 40);

        // Add Xing + LAME gapless header into first frame side-info offset (4 + 32 = 36)
        raw[36..40].copy_from_slice(b"Xing");
        raw[40..44].copy_from_slice(&1u32.to_be_bytes()); // frames flag
        raw[44..48].copy_from_slice(&100u32.to_be_bytes()); // 100 frames
        assert!((probe_duration_ms(&raw) - 2612).abs() <= 5);

        let binding = build_synthetic_cbr_mp3_frame();
        let mut stream = Mp3Stream::new(&binding);
        let first = stream.next_frame().expect("frame decoded");
        assert_eq!(first.sample_rate, 44100);
        assert_eq!(first.channels, 2);
        assert_eq!(first.samples.len(), 1152 * 2);
        stream.skip_to_ms(0);
        stream.seek_near_end_discard();
    }
}
