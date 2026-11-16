use std::io;

use super::aac_tables::*;
use super::mp4::Mp4Audio;

const MAX_ELEMENTS: usize = 16;
const MAX_WINDOW_COUNT: usize = 8;
const MAX_SECTIONS: usize = 120;
const MAX_MS_MASK: usize = 128;

const ZERO_HCB: usize = 0;
const FIRST_PAIR_HCB: usize = 5;
const NOISE_HCB: usize = 13;
const INTENSITY_HCB2: usize = 14;
const INTENSITY_HCB: usize = 15;

const ELEMENT_SCE: u32 = 0;
const ELEMENT_CPE: u32 = 1;
const ELEMENT_CCE: u32 = 2;
const ELEMENT_LFE: u32 = 3;
const ELEMENT_DSE: u32 = 4;
const ELEMENT_PCE: u32 = 5;
const ELEMENT_FIL: u32 = 6;
const ELEMENT_END: u32 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Unknown,
    AacMain,
    AacLc,
    AacSsr,
    AacLtp,
    AacSbr,
    ErAacLc,
    Other(i32),
}

impl Profile {
    fn from_int(v: i32) -> Self {
        match v {
            1 => Self::AacMain,
            2 => Self::AacLc,
            3 => Self::AacSsr,
            4 => Self::AacLtp,
            5 => Self::AacSbr,
            17 => Self::ErAacLc,
            -1 => Self::Unknown,
            other => Self::Other(other),
        }
    }

    fn is_supported(self) -> bool {
        matches!(
            self,
            Self::AacMain | Self::AacLc | Self::AacSbr | Self::ErAacLc
        )
    }

    fn is_error_resilient(self) -> bool {
        matches!(self, Self::ErAacLc)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleFrequency {
    pub index: i32,
    pub hz: u32,
}

impl SampleFrequency {
    pub const NONE: Self = Self { index: -1, hz: 0 };

    pub fn from_index(i: i32) -> Self {
        const RATES: [u32; 12] = [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000,
        ];
        if (0..12).contains(&i) {
            Self {
                index: i,
                hz: RATES[i as usize],
            }
        } else {
            Self::NONE
        }
    }

    pub fn from_hz(hz: u32) -> Self {
        for idx in 0..12 {
            let sf = Self::from_index(idx);
            if sf.hz == hz {
                return sf;
            }
        }
        Self::NONE
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelConfig {
    Unsupported,
    None,
    Mono,
    Stereo,
    StereoCenter,
    StereoCenterRear,
    Five,
    FivePlusOne,
    SevenPlusOne,
}

impl ChannelConfig {
    fn from_int(i: u32) -> Self {
        match i {
            0 => Self::None,
            1 => Self::Mono,
            2 => Self::Stereo,
            3 => Self::StereoCenter,
            4 => Self::StereoCenterRear,
            5 => Self::Five,
            6 => Self::FivePlusOne,
            7 | 8 => Self::SevenPlusOne,
            _ => Self::Unsupported,
        }
    }

    pub fn count(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::StereoCenter => 3,
            Self::StereoCenterRear => 4,
            Self::Five => 5,
            Self::FivePlusOne => 6,
            Self::SevenPlusOne => 8,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecoderConfig {
    pub profile: Profile,
    pub sample_freq: SampleFrequency,
    pub channel_config: ChannelConfig,
    pub frame_length: usize,
    pub small_frames: bool,
    pub sbr_present: bool,
    pub sbr_enabled: bool,
    pub downsampled_sbr: bool,
    pub section_data_resilience: bool,
    pub scalefactor_resilience: bool,
    pub spectral_data_resilience: bool,
}

impl DecoderConfig {
    pub fn parse_mp4_dsi(dsi: &[u8]) -> io::Result<Self> {
        let mut bs = BitStream::new(dsi);
        let mut cfg = Self {
            profile: Profile::AacMain,
            sample_freq: SampleFrequency::NONE,
            channel_config: ChannelConfig::Unsupported,
            frame_length: 1024,
            small_frames: false,
            sbr_present: false,
            sbr_enabled: true,
            downsampled_sbr: false,
            section_data_resilience: false,
            scalefactor_resilience: false,
            spectral_data_resilience: false,
        };

        cfg.profile = read_profile(&mut bs)?;
        let mut sf_idx = bs.read_bits(4)?;
        cfg.sample_freq = if sf_idx == 0xF {
            SampleFrequency::from_hz(bs.read_bits(24)?)
        } else {
            SampleFrequency::from_index(sf_idx as i32)
        };
        cfg.channel_config = ChannelConfig::from_int(bs.read_bits(4)?);

        if cfg.profile == Profile::AacSbr {
            cfg.sbr_present = true;
            sf_idx = bs.read_bits(4)?;
            let next_sf = if sf_idx == 0xF {
                SampleFrequency::from_hz(bs.read_bits(24)?)
            } else {
                SampleFrequency::from_index(sf_idx as i32)
            };
            cfg.downsampled_sbr = cfg.sample_freq.index == next_sf.index;
            cfg.sample_freq = next_sf;
            cfg.profile = read_profile(&mut bs)?;
        }

        match cfg.profile {
            Profile::AacMain
            | Profile::AacLc
            | Profile::AacSsr
            | Profile::AacLtp
            | Profile::ErAacLc => {
                if bs.bits_left() >= 3 {
                    let frame_len_flag = bs.read_bool()?;
                    cfg.small_frames = frame_len_flag;
                    cfg.frame_length = if frame_len_flag { 960 } else { 1024 };
                    let depends_on_core = bs.read_bool()?;
                    if depends_on_core && bs.bits_left() >= 14 {
                        let _delay = bs.read_bits(14)?;
                    }
                    if bs.bits_left() >= 1 {
                        let ext_flag = bs.read_bool()?;
                        if ext_flag {
                            if cfg.profile.is_error_resilient() && bs.bits_left() >= 3 {
                                cfg.section_data_resilience = bs.read_bool()?;
                                cfg.scalefactor_resilience = bs.read_bool()?;
                                cfg.spectral_data_resilience = bs.read_bool()?;
                            }
                            if bs.bits_left() >= 1 {
                                let _ = bs.skip_bit();
                            }
                        }
                    }
                    if cfg.channel_config == ChannelConfig::None && bs.bits_left() >= 16 {
                        let _ = bs.skip_bits(3);
                        let pce = Pce::decode(&mut bs)?;
                        cfg.profile = pce.profile;
                        cfg.sample_freq = pce.sample_freq;
                        cfg.channel_config = ChannelConfig::from_int(pce.channel_count as u32);
                    }
                    if bs.bits_left() > 10 {
                        let ext_type = bs.read_bits(11)?;
                        if ext_type == 0x2B7 && bs.bits_left() >= 6 {
                            let ext_prof = Profile::from_int(bs.read_bits(5)? as i32);
                            if ext_prof == Profile::AacSbr {
                                cfg.sbr_present = bs.read_bool()?;
                                if cfg.sbr_present && bs.bits_left() >= 4 {
                                    let tmp = bs.read_bits(4)? as i32;
                                    if tmp == cfg.sample_freq.index {
                                        cfg.downsampled_sbr = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unsupported AAC profile: {:?}", cfg.profile),
                ));
            }
        }

        if !cfg.profile.is_supported() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported AAC profile: {:?}", cfg.profile),
            ));
        }
        if cfg.sample_freq == SampleFrequency::NONE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid AAC sample frequency",
            ));
        }
        Ok(cfg)
    }
}

fn read_profile(bs: &mut BitStream) -> io::Result<Profile> {
    let mut i = bs.read_bits(5)? as i32;
    if i == 31 {
        i = 32 + bs.read_bits(6)? as i32;
    }
    Ok(Profile::from_int(i))
}

struct BitStream {
    buf: Vec<u8>,
    pos: usize,
    cache: u32,
    bits_cached: u8,
    position: usize,
}

impl BitStream {
    fn new(data: &[u8]) -> Self {
        let mut s = Self {
            buf: Vec::new(),
            pos: 0,
            cache: 0,
            bits_cached: 0,
            position: 0,
        };
        s.set_data(data);
        s
    }

    fn set_data(&mut self, data: &[u8]) {
        let words = data.len().div_ceil(4);
        self.buf.clear();
        self.buf.resize(words.max(1) * 4, 0);
        self.buf[..data.len()].copy_from_slice(data);
        self.pos = 0;
        self.cache = 0;
        self.bits_cached = 0;
        self.position = 0;
    }

    fn position(&self) -> usize {
        self.position
    }

    fn bits_left(&self) -> usize {
        8 * (self.buf.len().saturating_sub(self.pos)) + usize::from(self.bits_cached)
    }

    fn byte_align(&mut self) -> io::Result<()> {
        let to_flush = self.bits_cached & 7;
        if to_flush > 0 {
            self.skip_bits(usize::from(to_flush))?;
        }
        Ok(())
    }

    fn read_word(&mut self) -> io::Result<u32> {
        if self.pos + 4 > self.buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AAC bitstream EOF",
            ));
        }
        let w = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(w)
    }

    fn peek_word(&self) -> io::Result<u32> {
        if self.pos + 4 > self.buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AAC bitstream EOF",
            ));
        }
        Ok(u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]))
    }

    fn mask(n: u8) -> u32 {
        if n == 0 {
            0
        } else if n >= 32 {
            u32::MAX
        } else {
            (1u32 << n) - 1
        }
    }

    fn read_bits(&mut self, n: u8) -> io::Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.bits_cached >= n {
            self.bits_cached -= n;
            let res = (self.cache >> self.bits_cached) & Self::mask(n);
            self.position += usize::from(n);
            Ok(res)
        } else {
            self.position += usize::from(n);
            let c = self.cache & Self::mask(self.bits_cached);
            let left = n - self.bits_cached;
            self.cache = self.read_word()?;
            self.bits_cached = 32 - left;
            let hi = if left >= 32 { 0 } else { c << left };
            Ok(((self.cache >> self.bits_cached) & Self::mask(left)) | hi)
        }
    }

    fn read_bit(&mut self) -> io::Result<u32> {
        if self.bits_cached > 0 {
            self.bits_cached -= 1;
            self.position += 1;
            Ok((self.cache >> self.bits_cached) & 1)
        } else {
            self.cache = self.read_word()?;
            self.bits_cached = 31;
            self.position += 1;
            Ok((self.cache >> 31) & 1)
        }
    }

    fn read_bool(&mut self) -> io::Result<bool> {
        Ok(self.read_bit()? != 0)
    }

    fn peek_bits(&self, n: u8) -> io::Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.bits_cached >= n {
            Ok((self.cache >> (self.bits_cached - n)) & Self::mask(n))
        } else {
            let c = self.cache & Self::mask(self.bits_cached);
            let left = n - self.bits_cached;
            let next = self.peek_word()?;
            let hi = if left >= 32 { 0 } else { c << left };
            Ok(((next >> (32 - left)) & Self::mask(left)) | hi)
        }
    }

    fn skip_bits(&mut self, mut n: usize) -> io::Result<()> {
        self.position += n;
        if n <= usize::from(self.bits_cached) {
            self.bits_cached -= n as u8;
        } else {
            n -= usize::from(self.bits_cached);
            while n >= 32 {
                n -= 32;
                let _ = self.read_word()?;
            }
            if n > 0 {
                self.cache = self.read_word()?;
                self.bits_cached = 32 - n as u8;
            } else {
                self.cache = 0;
                self.bits_cached = 0;
            }
        }
        Ok(())
    }

    fn skip_bit(&mut self) -> io::Result<()> {
        self.position += 1;
        if self.bits_cached > 0 {
            self.bits_cached -= 1;
        } else {
            self.cache = self.read_word()?;
            self.bits_cached = 31;
        }
        Ok(())
    }
}

struct Pce {
    profile: Profile,
    sample_freq: SampleFrequency,
    channel_count: usize,
}

impl Pce {
    fn decode(bs: &mut BitStream) -> io::Result<Self> {
        let _tag = bs.read_bits(4)?;
        let profile = Profile::from_int(bs.read_bits(2)? as i32 + 1);
        let sample_freq = SampleFrequency::from_index(bs.read_bits(4)? as i32);
        let front = bs.read_bits(4)? as usize;
        let side = bs.read_bits(4)? as usize;
        let back = bs.read_bits(4)? as usize;
        let lfe = bs.read_bits(2)? as usize;
        let assoc = bs.read_bits(3)? as usize;
        let cc = bs.read_bits(4)? as usize;

        if bs.read_bool()? {
            let _ = bs.read_bits(4)?;
        }
        if bs.read_bool()? {
            let _ = bs.read_bits(4)?;
        }
        if bs.read_bool()? {
            let _ = bs.read_bits(3)?;
        }

        let mut ch_count = 0usize;
        for _ in 0..front {
            let is_cpe = bs.read_bool()?;
            let _tag = bs.read_bits(4)?;
            ch_count += if is_cpe { 2 } else { 1 };
        }
        for _ in 0..side {
            let is_cpe = bs.read_bool()?;
            let _tag = bs.read_bits(4)?;
            ch_count += if is_cpe { 2 } else { 1 };
        }
        for _ in 0..back {
            let is_cpe = bs.read_bool()?;
            let _tag = bs.read_bits(4)?;
            ch_count += if is_cpe { 2 } else { 1 };
        }
        for _ in 0..lfe {
            let _ = bs.read_bits(4)?;
            ch_count += 1;
        }
        for _ in 0..assoc {
            let _ = bs.read_bits(4)?;
        }
        for _ in 0..cc {
            let _ = bs.read_bool()?;
            let _ = bs.read_bits(4)?;
        }
        bs.byte_align()?;
        let comment_len = bs.read_bits(8)? as usize;
        for _ in 0..comment_len {
            let _ = bs.read_bits(8)?;
        }
        Ok(Self {
            profile,
            sample_freq,
            channel_count: ch_count,
        })
    }
}

fn try_decode_adif(bs: &mut BitStream, cfg: &mut DecoderConfig) -> io::Result<()> {
    if bs.bits_left() < 32 {
        return Ok(());
    }
    if bs.peek_bits(32)? != 0x4144_4946 {
        return Ok(());
    }
    let _ = bs.read_bits(32)?;
    if bs.read_bool()? {
        bs.skip_bits(72)?;
    }
    let _orig = bs.read_bool()?;
    let _home = bs.read_bool()?;
    let bitstream_type = bs.read_bool()?;
    let _bitrate = bs.read_bits(23)?;
    let pce_count = bs.read_bits(4)? + 1;
    let mut first_pce = None;
    for _ in 0..pce_count {
        if !bitstream_type {
            let _fullness = bs.read_bits(20)?;
        }
        let pce = Pce::decode(bs)?;
        if first_pce.is_none() {
            first_pce = Some(pce);
        }
    }
    if let Some(pce) = first_pce {
        cfg.profile = pce.profile;
        cfg.sample_freq = pce.sample_freq;
        cfg.channel_config = ChannelConfig::from_int(pce.channel_count as u32);
    }
    Ok(())
}

fn find_offset_quad(bs: &mut BitStream, table: &[[i32; 6]]) -> io::Result<usize> {
    let mut off = 0usize;
    let mut len = table[off][0] as u8;
    let mut cw = bs.read_bits(len)? as i32;
    while cw != table[off][1] {
        off += 1;
        if off >= table.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC huffman quad EOF",
            ));
        }
        let j = (table[off][0] - i32::from(len)).max(0) as u8;
        len = table[off][0] as u8;
        if j > 0 {
            cw = (cw << j) | (bs.read_bits(j)? as i32);
        }
    }
    Ok(off)
}

fn find_offset_pair(bs: &mut BitStream, table: &[[i32; 4]]) -> io::Result<usize> {
    let mut off = 0usize;
    let mut len = table[off][0] as u8;
    let mut cw = bs.read_bits(len)? as i32;
    while cw != table[off][1] {
        off += 1;
        if off >= table.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC huffman pair EOF",
            ));
        }
        let j = (table[off][0] - i32::from(len)).max(0) as u8;
        len = table[off][0] as u8;
        if j > 0 {
            cw = (cw << j) | (bs.read_bits(j)? as i32);
        }
    }
    Ok(off)
}

fn find_offset_sf(bs: &mut BitStream) -> io::Result<usize> {
    let mut off = 0usize;
    let mut len = HCB_SF[off][0] as u8;
    let mut cw = bs.read_bits(len)? as i32;
    while cw != HCB_SF[off][1] {
        off += 1;
        if off >= HCB_SF.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC huffman SF EOF",
            ));
        }
        let j = (HCB_SF[off][0] - i32::from(len)).max(0) as u8;
        len = HCB_SF[off][0] as u8;
        if j > 0 {
            cw = (cw << j) | (bs.read_bits(j)? as i32);
        }
    }
    Ok(off)
}

fn decode_scale_factor(bs: &mut BitStream) -> io::Result<i32> {
    let off = find_offset_sf(bs)?;
    Ok(HCB_SF[off][2])
}

fn sign_values(bs: &mut BitStream, data: &mut [i32; 4], len: usize) -> io::Result<()> {
    for val in data.iter_mut().take(len) {
        if *val != 0 && bs.read_bool()? {
            *val = -*val;
        }
    }
    Ok(())
}

fn get_escape(bs: &mut BitStream, s: i32) -> io::Result<i32> {
    let neg = s < 0;
    let mut i = 4u8;
    while bs.read_bool()? {
        i += 1;
        if i > 28 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC escape overflow",
            ));
        }
    }
    let j = (bs.read_bits(i)? | (1u32 << i)) as i32;
    Ok(if neg { -j } else { j })
}

fn decode_spectral_cb(bs: &mut BitStream, cb: usize, data: &mut [i32; 4]) -> io::Result<()> {
    const UNSIGNED: [bool; 11] = [
        false, false, true, true, false, false, true, true, true, true, true,
    ];
    match cb {
        1..=4 => {
            let tbl = match cb {
                1 => HCB1,
                2 => HCB2,
                3 => HCB3,
                _ => HCB4,
            };
            let off = find_offset_quad(bs, tbl)?;
            data[0] = tbl[off][2];
            data[1] = tbl[off][3];
            data[2] = tbl[off][4];
            data[3] = tbl[off][5];
            if UNSIGNED[cb - 1] {
                sign_values(bs, data, 4)?;
            }
        }
        5..=11 => {
            let tbl = match cb {
                5 => HCB5,
                6 => HCB6,
                7 => HCB7,
                8 => HCB8,
                9 => HCB9,
                10 => HCB10,
                _ => HCB11,
            };
            let off = find_offset_pair(bs, tbl)?;
            data[0] = tbl[off][2];
            data[1] = tbl[off][3];
            data[2] = 0;
            data[3] = 0;
            if UNSIGNED[cb - 1] {
                sign_values(bs, data, 2)?;
            }
            if cb == 11 {
                if data[0].abs() == 16 {
                    data[0] = get_escape(bs, data[0])?;
                }
                if data[1].abs() == 16 {
                    data[1] = get_escape(bs, data[1])?;
                }
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown AAC codebook {cb}"),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowSequence {
    OnlyLong = 0,
    LongStart = 1,
    EightShort = 2,
    LongStop = 3,
}

impl WindowSequence {
    fn from_bits(v: u32) -> io::Result<Self> {
        match v {
            0 => Ok(Self::OnlyLong),
            1 => Ok(Self::LongStart),
            2 => Ok(Self::EightShort),
            3 => Ok(Self::LongStop),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid window sequence",
            )),
        }
    }
}

#[derive(Clone)]
struct IcsInfo {
    window_sequence: WindowSequence,
    window_shape: [usize; 2],
    max_sfb: usize,
    window_group_count: usize,
    window_group_length: [usize; MAX_WINDOW_COUNT],
    window_count: usize,
    swb_count: usize,
    swb_offsets: &'static [i32],
}

impl IcsInfo {
    fn new() -> Self {
        Self {
            window_sequence: WindowSequence::OnlyLong,
            window_shape: [0, 0],
            max_sfb: 0,
            window_group_count: 1,
            window_group_length: [1; MAX_WINDOW_COUNT],
            window_count: 1,
            swb_count: 0,
            swb_offsets: SWB_OFFSET_LONG_WINDOW[4],
        }
    }

    fn decode(&mut self, bs: &mut BitStream, cfg: &DecoderConfig) -> io::Result<()> {
        let sf = cfg.sample_freq;
        if sf == SampleFrequency::NONE || !(0..12).contains(&sf.index) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid sample frequency",
            ));
        }
        let sf_idx = sf.index as usize;
        bs.skip_bit()?; // reserved
        self.window_sequence = WindowSequence::from_bits(bs.read_bits(2)?)?;
        self.window_shape[0] = self.window_shape[1];
        self.window_shape[1] = bs.read_bit()? as usize;

        self.window_group_count = 1;
        self.window_group_length[0] = 1;
        if self.window_sequence == WindowSequence::EightShort {
            self.max_sfb = bs.read_bits(4)? as usize;
            for _ in 0..7 {
                if bs.read_bool()? {
                    self.window_group_length[self.window_group_count - 1] += 1;
                } else {
                    self.window_group_count += 1;
                    self.window_group_length[self.window_group_count - 1] = 1;
                }
            }
            self.window_count = 8;
            self.swb_offsets = SWB_OFFSET_SHORT_WINDOW[sf_idx];
            self.swb_count = SWB_SHORT_WINDOW_COUNT[sf_idx] as usize;
        } else {
            self.max_sfb = bs.read_bits(6)? as usize;
            self.window_count = 1;
            self.swb_offsets = SWB_OFFSET_LONG_WINDOW[sf_idx];
            self.swb_count = SWB_LONG_WINDOW_COUNT[sf_idx] as usize;
            let pred_present = bs.read_bool()?;
            if pred_present {
                // Skip ICPrediction / LTP if encountered
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported AAC prediction data present",
                ));
            }
        }
        if self.max_sfb > self.swb_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "max_sfb > swb_count",
            ));
        }
        Ok(())
    }

    fn copy_from(&mut self, other: &Self) {
        self.window_sequence = other.window_sequence;
        self.window_shape[0] = self.window_shape[1];
        self.window_shape[1] = other.window_shape[1];
        self.max_sfb = other.max_sfb;
        self.window_count = other.window_count;
        self.window_group_count = other.window_group_count;
        self.window_group_length = other.window_group_length;
        self.swb_count = other.swb_count;
        self.swb_offsets = other.swb_offsets;
    }
}

fn decode_tns(bs: &mut BitStream, info: &IcsInfo) -> io::Result<()> {
    let bits = if info.window_sequence == WindowSequence::EightShort {
        [1u8, 4u8, 3u8]
    } else {
        [2u8, 6u8, 5u8]
    };
    let mut n_filt = [0usize; 8];
    let mut order = [[0usize; 4]; 8];
    let tns_tables: [&[f32]; 4] = [&TNS_COEF_0_3, &TNS_COEF_1_3, &TNS_COEF_0_4, &TNS_COEF_1_4];
    for w in 0..info.window_count {
        n_filt[w] = bs.read_bits(bits[0])? as usize;
        if n_filt[w] != 0 {
            let coef_res = bs.read_bit()? as usize;
            for item in order[w].iter_mut().take(n_filt[w].min(4)) {
                let _length = bs.read_bits(bits[1])?;
                let ord = bs.read_bits(bits[2])? as usize;
                *item = ord;
                if ord > 20 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "TNS order > 20"));
                }
                if ord > 0 {
                    let _dir = bs.read_bool()?;
                    let coef_compress = bs.read_bit()? as usize;
                    let coef_len = (coef_res + 3 - coef_compress) as u8;
                    let tbl = tns_tables[2 * coef_compress + coef_res];
                    for _ in 0..ord {
                        let idx = bs.read_bits(coef_len)? as usize;
                        let _ = tbl.get(idx).copied().unwrap_or(0.0);
                    }
                }
            }
        }
    }
    Ok(())
}

struct IcStream {
    info: IcsInfo,
    sfb_cb: [usize; MAX_SECTIONS],
    sect_end: [usize; MAX_SECTIONS],
    scale_factors: [f32; MAX_SECTIONS],
    data: Vec<f32>,
    global_gain: i32,
}

impl IcStream {
    fn new(frame_length: usize) -> Self {
        Self {
            info: IcsInfo::new(),
            sfb_cb: [0; MAX_SECTIONS],
            sect_end: [0; MAX_SECTIONS],
            scale_factors: [0.0; MAX_SECTIONS],
            data: vec![0.0; frame_length],
            global_gain: 0,
        }
    }

    fn decode(
        &mut self,
        bs: &mut BitStream,
        common_window: bool,
        cfg: &DecoderConfig,
        rng: &mut u32,
    ) -> io::Result<()> {
        self.global_gain = bs.read_bits(8)? as i32;
        if !common_window {
            self.info.decode(bs, cfg)?;
        }
        self.decode_section_data(bs)?;
        self.decode_scale_factors(bs)?;

        let pulse_present = bs.read_bool()?;
        if pulse_present {
            if self.info.window_sequence == WindowSequence::EightShort {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Pulse data not allowed in short frames",
                ));
            }
            let pulse_count = bs.read_bits(2)? + 1;
            let _start_swb = bs.read_bits(6)?;
            let _ = bs.read_bits(5)?;
            let _ = bs.read_bits(4)?;
            for _ in 1..pulse_count {
                let _ = bs.read_bits(5)?;
                let _ = bs.read_bits(4)?;
            }
        }

        let tns_present = bs.read_bool()?;
        if tns_present && !cfg.profile.is_error_resilient() {
            decode_tns(bs, &self.info)?;
        }

        let gain_control_present = bs.read_bool()?;
        if gain_control_present {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC gain control unsupported",
            ));
        }

        self.decode_spectral_data(bs, rng)?;
        Ok(())
    }

    fn decode_section_data(&mut self, bs: &mut BitStream) -> io::Result<()> {
        self.sfb_cb.fill(0);
        self.sect_end.fill(0);
        let bits = if self.info.window_sequence == WindowSequence::EightShort {
            3u8
        } else {
            5u8
        };
        let esc_val = (1u32 << bits) - 1;
        let wg_count = self.info.window_group_count;
        let max_sfb = self.info.max_sfb;
        let mut idx = 0usize;

        for _ in 0..wg_count {
            let mut k = 0usize;
            while k < max_sfb {
                let mut end = k;
                let cb = bs.read_bits(4)? as usize;
                if cb == 12 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid HCB 12"));
                }
                loop {
                    let incr = bs.read_bits(bits)?;
                    end += incr as usize;
                    if incr != esc_val {
                        break;
                    }
                }
                if end > max_sfb {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Too many SFBs"));
                }
                while k < end {
                    if idx >= MAX_SECTIONS {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Section overflow",
                        ));
                    }
                    self.sfb_cb[idx] = cb;
                    self.sect_end[idx] = end;
                    idx += 1;
                    k += 1;
                }
            }
        }
        Ok(())
    }

    fn decode_scale_factors(&mut self, bs: &mut BitStream) -> io::Result<()> {
        const SF_DELTA: i32 = 60;
        const SF_OFFSET: i32 = 200;
        let wg_count = self.info.window_group_count;
        let max_sfb = self.info.max_sfb;
        let mut offset = [self.global_gain, self.global_gain - 90, 0];
        let mut noise_flag = true;
        let mut idx = 0usize;

        for _ in 0..wg_count {
            let mut sfb = 0usize;
            while sfb < max_sfb {
                let end = self.sect_end[idx];
                match self.sfb_cb[idx] {
                    ZERO_HCB => {
                        while sfb < end {
                            self.scale_factors[idx] = 0.0;
                            sfb += 1;
                            idx += 1;
                        }
                    }
                    INTENSITY_HCB | INTENSITY_HCB2 => {
                        while sfb < end {
                            offset[2] += decode_scale_factor(bs)? - SF_DELTA;
                            let tmp = offset[2].clamp(-155, 100);
                            let sf_i = (-tmp + SF_OFFSET)
                                .clamp(0, SCALEFACTOR_TABLE.len() as i32 - 1)
                                as usize;
                            self.scale_factors[idx] = SCALEFACTOR_TABLE[sf_i];
                            sfb += 1;
                            idx += 1;
                        }
                    }
                    NOISE_HCB => {
                        while sfb < end {
                            if noise_flag {
                                offset[1] += bs.read_bits(9)? as i32 - 256;
                                noise_flag = false;
                            } else {
                                offset[1] += decode_scale_factor(bs)? - SF_DELTA;
                            }
                            let tmp = offset[1].clamp(-100, 155);
                            let sf_i = (tmp + SF_OFFSET)
                                .clamp(0, SCALEFACTOR_TABLE.len() as i32 - 1)
                                as usize;
                            self.scale_factors[idx] = -SCALEFACTOR_TABLE[sf_i];
                            sfb += 1;
                            idx += 1;
                        }
                    }
                    _ => {
                        while sfb < end {
                            offset[0] += decode_scale_factor(bs)? - SF_DELTA;
                            if offset[0] > 255 {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "AAC scalefactor out of range",
                                ));
                            }
                            let sf_i = (offset[0] - 100 + SF_OFFSET)
                                .clamp(0, SCALEFACTOR_TABLE.len() as i32 - 1)
                                as usize;
                            self.scale_factors[idx] = SCALEFACTOR_TABLE[sf_i];
                            sfb += 1;
                            idx += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_spectral_data(&mut self, bs: &mut BitStream, rng: &mut u32) -> io::Result<()> {
        self.data.fill(0.0);
        let max_sfb = self.info.max_sfb;
        let wg_count = self.info.window_group_count;
        let offsets = self.info.swb_offsets;
        let mut buf = [0i32; 4];
        let mut group_off = 0usize;
        let mut idx = 0usize;

        for g in 0..wg_count {
            let group_len = self.info.window_group_length[g];
            for sfb in 0..max_sfb {
                let hcb = self.sfb_cb[idx];
                let mut off = group_off + offsets[sfb] as usize;
                let width = (offsets[sfb + 1] - offsets[sfb]).max(0) as usize;
                if hcb == ZERO_HCB || hcb == INTENSITY_HCB || hcb == INTENSITY_HCB2 {
                    for _ in 0..group_len {
                        let end = (off + width).min(self.data.len());
                        if off < end {
                            self.data[off..end].fill(0.0);
                        }
                        off += 128;
                    }
                } else if hcb == NOISE_HCB {
                    for _ in 0..group_len {
                        let mut energy = 0.0f32;
                        for k in 0..width {
                            *rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                            let val = (*rng as i32) as f32;
                            if off + k < self.data.len() {
                                self.data[off + k] = val;
                            }
                            energy += val * val;
                        }
                        let scale = if energy > 0.0 {
                            self.scale_factors[idx] / energy.sqrt()
                        } else {
                            0.0
                        };
                        for k in 0..width {
                            if off + k < self.data.len() {
                                self.data[off + k] *= scale;
                            }
                        }
                        off += 128;
                    }
                } else {
                    let num = if hcb >= FIRST_PAIR_HCB { 2 } else { 4 };
                    for _ in 0..group_len {
                        let mut k = 0usize;
                        while k < width {
                            decode_spectral_cb(bs, hcb, &mut buf)?;
                            for (j, &v) in buf.iter().enumerate().take(num) {
                                let iq = if v >= 0 {
                                    let uv = v as usize;
                                    IQ_TABLE
                                        .get(uv)
                                        .copied()
                                        .unwrap_or_else(|| (uv as f64).powf(4.0 / 3.0) as f32)
                                } else {
                                    let uv = v.unsigned_abs() as usize;
                                    -IQ_TABLE
                                        .get(uv)
                                        .copied()
                                        .unwrap_or_else(|| (uv as f64).powf(4.0 / 3.0) as f32)
                                };
                                if off + k + j < self.data.len() {
                                    self.data[off + k + j] = iq * self.scale_factors[idx];
                                }
                            }
                            k += num;
                        }
                        off += 128;
                    }
                }
                idx += 1;
            }
            group_off += group_len << 7;
        }
        Ok(())
    }
}

struct Cpe {
    ms_mask_type: u32,
    ms_used: [bool; MAX_MS_MASK],
    common_window: bool,
    ics_l: IcStream,
    ics_r: IcStream,
}

impl Cpe {
    fn new(frame_length: usize) -> Self {
        Self {
            ms_mask_type: 0,
            ms_used: [false; MAX_MS_MASK],
            common_window: false,
            ics_l: IcStream::new(frame_length),
            ics_r: IcStream::new(frame_length),
        }
    }

    fn decode(&mut self, bs: &mut BitStream, cfg: &DecoderConfig, rng: &mut u32) -> io::Result<()> {
        let _tag = bs.read_bits(4)?;
        self.common_window = bs.read_bool()?;
        if self.common_window {
            self.ics_l.info.decode(bs, cfg)?;
            self.ics_r.info.copy_from(&self.ics_l.info);
            self.ms_mask_type = bs.read_bits(2)?;
            match self.ms_mask_type {
                0 => self.ms_used.fill(false),
                1 => {
                    let count = self.ics_l.info.window_group_count * self.ics_l.info.max_sfb;
                    for slot in self.ms_used.iter_mut().take(count.min(MAX_MS_MASK)) {
                        *slot = bs.read_bool()?;
                    }
                }
                2 => self.ms_used.fill(true),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Reserved MS mask",
                    ));
                }
            }
        } else {
            self.ms_mask_type = 0;
            self.ms_used.fill(false);
        }

        self.ics_l.decode(bs, self.common_window, cfg, rng)?;
        self.ics_r.decode(bs, self.common_window, cfg, rng)?;
        Ok(())
    }

    fn apply_ms(&mut self) {
        if !self.common_window || self.ms_mask_type == 0 {
            return;
        }
        let wg_count = self.ics_l.info.window_group_count;
        let max_sfb = self.ics_l.info.max_sfb;
        let offsets = self.ics_l.info.swb_offsets;
        let mut group_off = 0usize;
        let mut idx = 0usize;

        for g in 0..wg_count {
            let group_len = self.ics_l.info.window_group_length[g];
            for i in 0..max_sfb {
                if self.ms_used[idx]
                    && self.ics_l.sfb_cb[idx] < NOISE_HCB
                    && self.ics_r.sfb_cb[idx] < NOISE_HCB
                {
                    let start = offsets[i] as usize;
                    let width = (offsets[i + 1] - offsets[i]).max(0) as usize;
                    for w in 0..group_len {
                        let off = group_off + w * 128 + start;
                        for j in 0..width {
                            let l = self.ics_l.data[off + j];
                            let r = self.ics_r.data[off + j];
                            self.ics_l.data[off + j] = l + r;
                            self.ics_r.data[off + j] = l - r;
                        }
                    }
                }
                idx += 1;
            }
            group_off += group_len * 128;
        }
    }

    fn apply_is(&mut self) {
        let wg_count = self.ics_r.info.window_group_count;
        let max_sfb = self.ics_r.info.max_sfb;
        let offsets = self.ics_r.info.swb_offsets;
        let mut group_off = 0usize;
        let mut idx = 0usize;

        for g in 0..wg_count {
            let group_len = self.ics_r.info.window_group_length[g];
            let mut i = 0usize;
            while i < max_sfb {
                let end = self.ics_r.sect_end[idx];
                if self.ics_r.sfb_cb[idx] == INTENSITY_HCB
                    || self.ics_r.sfb_cb[idx] == INTENSITY_HCB2
                {
                    while i < end {
                        let mut c = if self.ics_r.sfb_cb[idx] == INTENSITY_HCB {
                            1.0f32
                        } else {
                            -1.0f32
                        };
                        if self.ms_mask_type != 0 && self.ms_used[idx] {
                            c = -c;
                        }
                        let scale = c * self.ics_r.scale_factors[idx];
                        let start = offsets[i] as usize;
                        let width = (offsets[i + 1] - offsets[i]).max(0) as usize;
                        for w in 0..group_len {
                            let off = group_off + w * 128 + start;
                            for j in 0..width {
                                self.ics_r.data[off + j] = self.ics_l.data[off + j] * scale;
                            }
                        }
                        i += 1;
                        idx += 1;
                    }
                } else {
                    idx += end.saturating_sub(i);
                    i = end.max(i + 1);
                }
            }
            group_off += group_len * 128;
        }
    }
}

struct Fft {
    len: usize,
    rev: Vec<[f32; 2]>,
}

impl Fft {
    fn new(len: usize) -> io::Result<Self> {
        if !matches!(len, 64 | 512 | 60 | 480) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported AAC FFT length {len}"),
            ));
        }
        Ok(Self {
            len,
            rev: vec![[0.0; 2]; len],
        })
    }

    fn root(&self, km: usize) -> (f32, f32) {
        match self.len {
            64 => (FFT_TABLE_64[km][0], FFT_TABLE_64[km][1]),
            512 => (FFT_TABLE_512[km][0], FFT_TABLE_512[km][1]),
            60 => (FFT_TABLE_60[km][0], FFT_TABLE_60[km][1]),
            480 => (FFT_TABLE_480[km][0], FFT_TABLE_480[km][1]),
            _ => (1.0, 0.0),
        }
    }

    fn process_ifft(&mut self, data: &mut [[f32; 2]]) {
        let len = self.len;
        let mut ii = 0usize;
        for i in 0..len {
            self.rev[i] = data[ii];
            let mut k = len >> 1;
            while ii >= k && k > 0 {
                ii -= k;
                k >>= 1;
            }
            ii += k;
        }
        data[..len].copy_from_slice(&self.rev);

        let mut i = 0usize;
        while i + 3 < len {
            let a0 = data[i][0] + data[i + 1][0];
            let a1 = data[i][1] + data[i + 1][1];
            let b0 = data[i + 2][0] + data[i + 3][0];
            let b1 = data[i + 2][1] + data[i + 3][1];
            let c0 = data[i][0] - data[i + 1][0];
            let c1 = data[i][1] - data[i + 1][1];
            let d0 = data[i + 2][0] - data[i + 3][0];
            let d1 = data[i + 2][1] - data[i + 3][1];
            data[i][0] = a0 + b0;
            data[i][1] = a1 + b1;
            data[i + 2][0] = a0 - b0;
            data[i + 2][1] = a1 - b1;
            // forward = false: i+1 gets e1, i+3 gets e2
            data[i + 1][0] = c0 - d1;
            data[i + 1][1] = c1 + d0;
            data[i + 3][0] = c0 + d1;
            data[i + 3][1] = c1 - d0;
            i += 4;
        }

        let mut step = 4usize;
        while step < len {
            let shift = step << 1;
            let m = len / shift;
            let mut j = 0usize;
            while j < len {
                for k in 0..step {
                    let km = k * m;
                    let (root_re, root_im) = self.root(km);
                    let a_re = data[step + j + k][0];
                    let a_im = data[step + j + k][1];
                    let z_re = a_re * root_re - a_im * root_im;
                    let z_im = a_re * root_im + a_im * root_re;
                    let b_re = data[j + k][0];
                    let b_im = data[j + k][1];
                    data[step + j + k][0] = b_re - z_re;
                    data[step + j + k][1] = b_im - z_im;
                    data[j + k][0] = b_re + z_re;
                    data[j + k][1] = b_im + z_im;
                }
                j += shift;
            }
            step <<= 1;
        }
    }
}

struct Mdct {
    n: usize,
    sincos: &'static [[f32; 2]],
    fft: Fft,
    buf: Vec<[f32; 2]>,
}

impl Mdct {
    fn new(length: usize) -> io::Result<Self> {
        let sincos: &'static [[f32; 2]] = match length {
            2048 => &MDCT_TABLE_2048,
            256 => &MDCT_TABLE_128,
            1920 => &MDCT_TABLE_1920,
            240 => &MDCT_TABLE_240,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unsupported AAC MDCT length {length}"),
                ));
            }
        };
        let n4 = length >> 2;
        Ok(Self {
            n: length,
            sincos,
            fft: Fft::new(n4)?,
            buf: vec![[0.0; 2]; n4],
        })
    }

    fn process(&mut self, input: &[f32], in_off: usize, out: &mut [f32], out_off: usize) {
        let n2 = self.n >> 1;
        let n4 = self.n >> 2;
        let n8 = self.n >> 3;

        for k in 0..n4 {
            let a = input[in_off + 2 * k];
            let b = input[in_off + n2 - 1 - 2 * k];
            let sc0 = self.sincos[k][0];
            let sc1 = self.sincos[k][1];
            self.buf[k][1] = a * sc0 + b * sc1;
            self.buf[k][0] = b * sc0 - a * sc1;
        }

        self.fft.process_ifft(&mut self.buf);

        for k in 0..n4 {
            let t0 = self.buf[k][0];
            let t1 = self.buf[k][1];
            let sc0 = self.sincos[k][0];
            let sc1 = self.sincos[k][1];
            self.buf[k][1] = t1 * sc0 + t0 * sc1;
            self.buf[k][0] = t0 * sc0 - t1 * sc1;
        }

        let mut k = 0usize;
        while k < n8 {
            out[out_off + 2 * k] = self.buf[n8 + k][1];
            out[out_off + 2 + 2 * k] = self.buf[n8 + 1 + k][1];

            out[out_off + 1 + 2 * k] = -self.buf[n8 - 1 - k][0];
            out[out_off + 3 + 2 * k] = -self.buf[n8 - 2 - k][0];

            out[out_off + n4 + 2 * k] = self.buf[k][0];
            out[out_off + n4 + 2 + 2 * k] = self.buf[1 + k][0];

            out[out_off + n4 + 1 + 2 * k] = -self.buf[n4 - 1 - k][1];
            out[out_off + n4 + 3 + 2 * k] = -self.buf[n4 - 2 - k][1];

            out[out_off + n2 + 2 * k] = self.buf[n8 + k][0];
            out[out_off + n2 + 2 + 2 * k] = self.buf[n8 + 1 + k][0];

            out[out_off + n2 + 1 + 2 * k] = -self.buf[n8 - 1 - k][1];
            out[out_off + n2 + 3 + 2 * k] = -self.buf[n8 - 2 - k][1];

            out[out_off + n2 + n4 + 2 * k] = -self.buf[k][1];
            out[out_off + n2 + n4 + 2 + 2 * k] = -self.buf[1 + k][1];

            out[out_off + n2 + n4 + 1 + 2 * k] = self.buf[n4 - 1 - k][0];
            out[out_off + n2 + n4 + 3 + 2 * k] = self.buf[n4 - 2 - k][0];
            k += 2;
        }
    }
}

struct FilterBank {
    small_frames: bool,
    length: usize,
    short_len: usize,
    mid: usize,
    trans: usize,
    mdct_short: Mdct,
    mdct_long: Mdct,
    buf: Vec<f32>,
    overlaps: Vec<Vec<f32>>,
}

impl FilterBank {
    fn new(small_frames: bool, channels: usize) -> io::Result<Self> {
        let (length, short_len) = if small_frames {
            (960, 120)
        } else {
            (1024, 128)
        };
        Ok(Self {
            small_frames,
            length,
            short_len,
            mid: (length - short_len) / 2,
            trans: short_len / 2,
            mdct_short: Mdct::new(short_len * 2)?,
            mdct_long: Mdct::new(length * 2)?,
            buf: vec![0.0; 2 * length],
            overlaps: vec![vec![0.0; length]; channels.max(1)],
        })
    }

    fn long_window(&self, shape: usize) -> &'static [f32] {
        match (self.small_frames, shape & 1) {
            (false, 0) => &SINE_1024,
            (false, _) => &KBD_1024,
            (true, 0) => &SINE_960,
            (true, _) => &KBD_960,
        }
    }

    fn short_window(&self, shape: usize) -> &'static [f32] {
        match (self.small_frames, shape & 1) {
            (false, 0) => &SINE_128,
            (false, _) => &KBD_128,
            (true, 0) => &SINE_120,
            (true, _) => &KBD_120,
        }
    }

    fn process(
        &mut self,
        seq: WindowSequence,
        shape_cur: usize,
        shape_prev: usize,
        input: &[f32],
        out: &mut [f32],
        channel: usize,
    ) {
        if channel >= self.overlaps.len() {
            self.overlaps
                .resize_with(channel + 1, || vec![0.0; self.length]);
        }
        let len = self.length;
        let sl = self.short_len;
        let mid = self.mid;
        let trans = self.trans;
        let lw_prev = self.long_window(shape_prev);
        let lw_cur = self.long_window(shape_cur);
        let sw_prev = self.short_window(shape_prev);
        let sw_cur = self.short_window(shape_cur);
        let overlap = &mut self.overlaps[channel];

        match seq {
            WindowSequence::OnlyLong => {
                self.mdct_long.process(input, 0, &mut self.buf, 0);
                for i in 0..len {
                    out[i] = overlap[i] + self.buf[i] * lw_prev[i];
                }
                for i in 0..len {
                    overlap[i] = self.buf[len + i] * lw_cur[len - 1 - i];
                }
            }
            WindowSequence::LongStart => {
                self.mdct_long.process(input, 0, &mut self.buf, 0);
                for i in 0..len {
                    out[i] = overlap[i] + self.buf[i] * lw_prev[i];
                }
                overlap[..mid].copy_from_slice(&self.buf[len..(mid + len)]);
                for i in 0..sl {
                    overlap[mid + i] = self.buf[len + mid + i] * sw_cur[sl - i - 1];
                }
                for i in 0..mid {
                    overlap[mid + sl + i] = 0.0;
                }
            }
            WindowSequence::EightShort => {
                for i in 0..8 {
                    self.mdct_short
                        .process(input, i * sl, &mut self.buf, 2 * i * sl);
                }
                out[..mid].copy_from_slice(&overlap[..mid]);
                for i in 0..sl {
                    out[mid + i] = overlap[mid + i] + self.buf[i] * sw_prev[i];
                    out[mid + sl + i] = overlap[mid + sl + i]
                        + self.buf[sl + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 2 + i] * sw_cur[i];
                    out[mid + sl * 2 + i] = overlap[mid + sl * 2 + i]
                        + self.buf[sl * 3 + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 4 + i] * sw_cur[i];
                    out[mid + sl * 3 + i] = overlap[mid + sl * 3 + i]
                        + self.buf[sl * 5 + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 6 + i] * sw_cur[i];
                    if i < trans {
                        out[mid + sl * 4 + i] = overlap[mid + sl * 4 + i]
                            + self.buf[sl * 7 + i] * sw_cur[sl - 1 - i]
                            + self.buf[sl * 8 + i] * sw_cur[i];
                    }
                }
                for i in 0..sl {
                    if i >= trans {
                        overlap[mid + sl * 4 + i - len] = self.buf[sl * 7 + i] * sw_cur[sl - 1 - i]
                            + self.buf[sl * 8 + i] * sw_cur[i];
                    }
                    overlap[mid + sl * 5 + i - len] = self.buf[sl * 9 + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 10 + i] * sw_cur[i];
                    overlap[mid + sl * 6 + i - len] = self.buf[sl * 11 + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 12 + i] * sw_cur[i];
                    overlap[mid + sl * 7 + i - len] = self.buf[sl * 13 + i] * sw_cur[sl - 1 - i]
                        + self.buf[sl * 14 + i] * sw_cur[i];
                    overlap[mid + sl * 8 + i - len] = self.buf[sl * 15 + i] * sw_cur[sl - 1 - i];
                }
                for i in 0..mid {
                    overlap[mid + sl + i] = 0.0;
                }
            }
            WindowSequence::LongStop => {
                self.mdct_long.process(input, 0, &mut self.buf, 0);
                out[..mid].copy_from_slice(&overlap[..mid]);
                for i in 0..sl {
                    out[mid + i] = overlap[mid + i] + self.buf[mid + i] * sw_prev[i];
                }
                for i in 0..mid {
                    out[mid + sl + i] = overlap[mid + sl + i] + self.buf[mid + sl + i];
                }
                for i in 0..len {
                    overlap[i] = self.buf[len + i] * lw_cur[len - 1 - i];
                }
            }
        }
    }
}

enum SynElement {
    Single(Box<IcStream>),
    Pair(Box<Cpe>),
}

pub struct AacDecoder {
    pub config: DecoderConfig,
    filter_bank: FilterBank,
    elements: Vec<SynElement>,
    channel_buf: Vec<Vec<f32>>,
    bs: BitStream,
    rng: u32,
}

impl AacDecoder {
    pub fn from_mp4_dsi(dsi: &[u8]) -> io::Result<Self> {
        let config = DecoderConfig::parse_mp4_dsi(dsi)?;
        let ch = config.channel_config.count().max(1);
        let filter_bank = FilterBank::new(config.small_frames, ch)?;
        Ok(Self {
            config,
            filter_bank,
            elements: Vec::with_capacity(MAX_ELEMENTS),
            channel_buf: vec![Vec::new(); ch],
            bs: BitStream::new(&[]),
            rng: 0x1F2E_3D4C,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_freq.hz.max(44100)
    }

    pub fn channels(&self) -> usize {
        self.config.channel_config.count().clamp(1, 2)
    }

    pub fn decode_frame(&mut self, frame: &[u8], out_interleaved: &mut Vec<i16>) -> io::Result<()> {
        out_interleaved.clear();
        if frame.is_empty() {
            return Ok(());
        }
        self.bs.set_data(frame);
        try_decode_adif(&mut self.bs, &mut self.config)?;
        if !self.config.profile.is_supported() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported AAC profile",
            ));
        }

        let fl = self.config.frame_length;
        self.elements.clear();
        if !self.config.profile.is_error_resilient() {
            loop {
                if self.bs.bits_left() < 3 {
                    break;
                }
                let typ = self.bs.read_bits(3)?;
                if typ == ELEMENT_END {
                    break;
                }
                match typ {
                    ELEMENT_SCE | ELEMENT_LFE => {
                        let _tag = self.bs.read_bits(4)?;
                        let mut ics = Box::new(IcStream::new(fl));
                        ics.decode(&mut self.bs, false, &self.config, &mut self.rng)?;
                        self.elements.push(SynElement::Single(ics));
                    }
                    ELEMENT_CPE => {
                        let mut cpe = Box::new(Cpe::new(fl));
                        cpe.decode(&mut self.bs, &self.config, &mut self.rng)?;
                        self.elements.push(SynElement::Pair(cpe));
                    }
                    ELEMENT_DSE => {
                        let _tag = self.bs.read_bits(4)?;
                        let align = self.bs.read_bool()?;
                        let mut count = self.bs.read_bits(8)?;
                        if count == 255 {
                            count += self.bs.read_bits(8)?;
                        }
                        if align {
                            self.bs.byte_align()?;
                        }
                        self.bs.skip_bits(count as usize * 8)?;
                    }
                    ELEMENT_PCE => {
                        let pce = Pce::decode(&mut self.bs)?;
                        self.config.profile = pce.profile;
                        self.config.sample_freq = pce.sample_freq;
                        self.config.channel_config =
                            ChannelConfig::from_int(pce.channel_count as u32);
                    }
                    ELEMENT_FIL => {
                        let mut count = self.bs.read_bits(4)? as usize;
                        if count == 15 {
                            count += self.bs.read_bits(8)? as usize - 1;
                        }
                        self.bs.skip_bits(count * 8)?;
                    }
                    ELEMENT_CCE => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "AAC CCE unsupported",
                        ));
                    }
                    _ => break,
                }
            }
        } else {
            match self.config.channel_config {
                ChannelConfig::Mono => {
                    let mut ics = Box::new(IcStream::new(fl));
                    ics.decode(&mut self.bs, false, &self.config, &mut self.rng)?;
                    self.elements.push(SynElement::Single(ics));
                }
                ChannelConfig::Stereo => {
                    let mut cpe = Box::new(Cpe::new(fl));
                    cpe.decode(&mut self.bs, &self.config, &mut self.rng)?;
                    self.elements.push(SynElement::Pair(cpe));
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Unsupported ER channel config",
                    ));
                }
            }
        }
        let _ = self.bs.byte_align();
        let _bits_read = self.bs.position();

        let total_chs = self
            .config
            .channel_config
            .count()
            .max(
                self.elements
                    .iter()
                    .map(|e| match e {
                        SynElement::Single(_) => 1,
                        SynElement::Pair(_) => 2,
                    })
                    .sum(),
            )
            .max(1);
        if self.channel_buf.len() != total_chs {
            self.channel_buf = vec![vec![0.0; fl]; total_chs];
        } else {
            for b in &mut self.channel_buf {
                if b.len() != fl {
                    b.resize(fl, 0.0);
                }
            }
        }

        let mut ch_cursor = 0usize;
        for elem in &mut self.elements {
            if ch_cursor >= total_chs {
                break;
            }
            match elem {
                SynElement::Single(ics) => {
                    self.filter_bank.process(
                        ics.info.window_sequence,
                        ics.info.window_shape[1],
                        ics.info.window_shape[0],
                        &ics.data,
                        &mut self.channel_buf[ch_cursor],
                        ch_cursor,
                    );
                    ch_cursor += 1;
                }
                SynElement::Pair(cpe) => {
                    cpe.apply_ms();
                    cpe.apply_is();
                    self.filter_bank.process(
                        cpe.ics_l.info.window_sequence,
                        cpe.ics_l.info.window_shape[1],
                        cpe.ics_l.info.window_shape[0],
                        &cpe.ics_l.data,
                        &mut self.channel_buf[ch_cursor],
                        ch_cursor,
                    );
                    if ch_cursor + 1 < total_chs {
                        self.filter_bank.process(
                            cpe.ics_r.info.window_sequence,
                            cpe.ics_r.info.window_shape[1],
                            cpe.ics_r.info.window_shape[0],
                            &cpe.ics_r.data,
                            &mut self.channel_buf[ch_cursor + 1],
                            ch_cursor + 1,
                        );
                    }
                    ch_cursor += 2;
                }
            }
        }

        let out_chs = total_chs.clamp(1, 2);
        out_interleaved.reserve(fl * out_chs);
        for j in 0..fl {
            for ch in 0..out_chs {
                let s = self.channel_buf[ch][j]
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
                out_interleaved.push(s);
            }
        }
        Ok(())
    }
}

/// MP4/fMP4 AAC stream wrapper matching `Overtone`'s `AacPlayer` + overlap-priming logic.
pub struct AacSource {
    mp4: Mp4Audio,
    decoder: AacDecoder,
    next: usize,
    primed: bool,
    pending: Vec<i16>,
    cursor: usize,
    pub sample_rate: u32,
    pub channels: usize,
}

impl AacSource {
    pub fn open(raw: Vec<u8>, start_ms: u64) -> io::Result<Self> {
        let mp4 = Mp4Audio::parse(raw)?;
        let decoder = AacDecoder::from_mp4_dsi(mp4.decoder_config())?;
        let target = mp4.index_at(start_ms);
        let next = target.saturating_sub(1);
        let primed = next == target;
        let sample_rate = decoder.sample_rate();
        let channels = decoder.channels();
        let mut s = Self {
            mp4,
            decoder,
            next,
            primed,
            pending: Vec::new(),
            cursor: 0,
            sample_rate,
            channels,
        };
        let _ = s.decode_next();
        if s.decoder.sample_rate() > 0 {
            s.sample_rate = s.decoder.sample_rate();
        }
        if s.decoder.channels() > 0 {
            s.channels = s.decoder.channels();
        }
        Ok(s)
    }

    pub fn read_i16(&mut self, out: &mut [i16]) -> usize {
        let mut written = 0usize;
        while written < out.len() {
            if self.cursor >= self.pending.len() && !self.decode_next() {
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

    fn decode_next(&mut self) -> bool {
        while self.next < self.mp4.count() {
            let idx = self.next;
            self.next += 1;
            let frame = self.mp4.frame(idx);
            let mut decoded = Vec::new();
            if self.decoder.decode_frame(frame, &mut decoded).is_err() || decoded.is_empty() {
                continue;
            }
            if !self.primed {
                self.primed = true;
                continue;
            }
            self.pending = decoded;
            self.cursor = 0;
            return true;
        }
        self.pending.clear();
        self.cursor = 0;
        false
    }
}

/// Checks ADTS AAC or MP4 signature and probes duration in ms.
pub fn probe_aac_duration_ms(raw: &[u8]) -> u64 {
    if Mp4Audio::looks_like_mp4(raw)
        && let Ok(mp4) = Mp4Audio::parse(raw.to_vec())
    {
        return mp4.duration_ms();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aac_lc_audio_specific_config() {
        // ObjectType=2 (AAC-LC: 00010), freqIdx=4 (44100: 0100), chanConfig=2 (stereo: 0010),
        // frameLen=0, depends=0, ext=0 -> 00010 0100 0010 000 = 0x12 0x10
        let dsi = [0x12, 0x10];
        let cfg = DecoderConfig::parse_mp4_dsi(&dsi).expect("parse DSI");
        assert_eq!(cfg.profile, Profile::AacLc);
        assert_eq!(cfg.sample_freq.hz, 44100);
        assert_eq!(cfg.channel_config, ChannelConfig::Stereo);
        assert_eq!(cfg.frame_length, 1024);
    }

    #[test]
    fn decodes_silent_cpe_aac_frame() {
        // DSI: AAC-LC 44100 Hz stereo
        let dsi = [0x12, 0x10];
        let mut dec = AacDecoder::from_mp4_dsi(&dsi).expect("init decoder");
        // Bitstream for silent CPE:
        // element_type=1 (CPE, 3b: 001)
        // tag=0 (4b: 0000)
        // common_window=1 (1b: 1)
        // reserved=0 (1b: 0), win_seq=0 (2b: 00), win_shape=0 (1b: 0), max_sfb=0 (6b: 000000), pred=0 (1b: 0)
        // ms_mask=0 (2b: 00)
        // icsL: global_gain=0 (8b), pulse=0 (1b), tns=0 (1b), gain=0 (1b)
        // icsR: global_gain=0 (8b), pulse=0 (1b), tns=0 (1b), gain=0 (1b)
        // element_type=7 (END, 3b: 111)
        let mut bits: Vec<u8> = Vec::new();
        let push = |v: &mut Vec<u8>, val: u32, n: u8| {
            for i in (0..n).rev() {
                v.push(((val >> i) & 1) as u8);
            }
        };
        push(&mut bits, ELEMENT_CPE, 3);
        push(&mut bits, 0, 4); // tag
        push(&mut bits, 1, 1); // common_window
        push(&mut bits, 0, 1); // reserved
        push(&mut bits, 0, 2); // ONLY_LONG
        push(&mut bits, 0, 1); // win_shape
        push(&mut bits, 0, 6); // max_sfb = 0
        push(&mut bits, 0, 1); // pred = 0
        push(&mut bits, 0, 2); // ms_mask = ALL_0
        for _ in 0..2 {
            push(&mut bits, 100, 8); // global_gain
            push(&mut bits, 0, 1); // pulse
            push(&mut bits, 0, 1); // tns
            push(&mut bits, 0, 1); // gain
        }
        push(&mut bits, ELEMENT_END, 3);
        while !bits.len().is_multiple_of(8) {
            bits.push(0);
        }
        let mut bytes = vec![0u8; bits.len() / 8];
        for (i, b) in bits.chunks(8).enumerate() {
            let mut byte = 0u8;
            for &bit in b {
                byte = (byte << 1) | bit;
            }
            bytes[i] = byte;
        }
        let mut pcm = Vec::new();
        dec.decode_frame(&bytes, &mut pcm)
            .expect("decode silent AAC frame");
        assert_eq!(pcm.len(), 2048); // 1024 samples * 2 ch
        assert!(pcm.iter().all(|&s| s == 0));
    }
}
