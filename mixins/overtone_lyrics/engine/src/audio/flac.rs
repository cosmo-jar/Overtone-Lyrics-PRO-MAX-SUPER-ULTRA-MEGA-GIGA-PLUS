use std::io;

pub struct FlacDecoder {
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: usize,
    pub max_block: usize,
    pub total_samples: u64,
    pub out_channels: usize,

    data: Vec<u8>,
    audio_start: usize,
    fixed_block: usize,
    seek_samples: Vec<u64>,
    seek_offsets: Vec<u64>,
    ch: Vec<Vec<i32>>,
    bp: usize,
    block_size: usize,
    block_filled: usize,
    block_read: usize,
    hdr_channels: usize,
    hdr_assign: u8,
    hdr_bps: usize,
    bps_of_block: usize,
}

impl FlacDecoder {
    pub fn new(data: Vec<u8>) -> io::Result<Self> {
        let p0 = skip_id3(&data);
        if p0 + 4 > data.len() || &data[p0..p0 + 4] != b"fLaC" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a FLAC file",
            ));
        }
        let mut p = p0 + 4;
        let mut rate = 0u32;
        let mut chans = 0usize;
        let mut bps = 0usize;
        let mut min_b = 0usize;
        let mut max_b = 0usize;
        let mut total = 0u64;
        let mut seek_samples = Vec::new();
        let mut seek_offsets = Vec::new();
        let mut last = false;

        while !last {
            if p + 4 > data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated FLAC metadata",
                ));
            }
            let h = data[p];
            last = (h & 0x80) != 0;
            let typ = h & 0x7F;
            let len = u24(&data, p + 1);
            let body = p + 4;
            if body + len > data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated FLAC metadata",
                ));
            }
            if typ == 0 && len >= 18 {
                min_b = u16_be(&data, body);
                max_b = u16_be(&data, body + 2);
                let mut v = 0u64;
                for i in 0..8 {
                    v = (v << 8) | u64::from(data[body + 10 + i]);
                }
                rate = (v >> 44) as u32;
                chans = (((v >> 41) & 7) + 1) as usize;
                bps = (((v >> 36) & 31) + 1) as usize;
                total = v & 0xF_FFFF_FFFF;
            } else if typ == 3 {
                let mut i = 0;
                while i + 18 <= len {
                    let s = u64_be(&data, body + i);
                    if s != u64::MAX {
                        seek_samples.push(s);
                        seek_offsets.push(u64_be(&data, body + i + 8));
                    }
                    i += 18;
                }
            }
            p = body + len;
        }
        if rate == 0 || chans == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "FLAC without STREAMINFO",
            ));
        }
        let max_block = max_b.max(16);
        let fixed_block = if min_b == max_b { min_b } else { 0 };
        let out_channels = chans.min(2);
        let buf_len = max_block.max(65536);
        let ch = vec![vec![0i32; buf_len]; chans];
        Ok(Self {
            sample_rate: rate,
            channels: chans,
            bits_per_sample: bps,
            max_block,
            total_samples: total,
            out_channels,
            data,
            audio_start: p,
            fixed_block,
            seek_samples,
            seek_offsets,
            ch,
            bp: p * 8,
            block_size: 0,
            block_filled: 0,
            block_read: 0,
            hdr_channels: chans,
            hdr_assign: 0,
            hdr_bps: bps,
            bps_of_block: 16,
        })
    }

    pub fn duration_ms(&self) -> u64 {
        if self.total_samples == 0 {
            0
        } else {
            self.total_samples.saturating_mul(1000) / u64::from(self.sample_rate)
        }
    }

    pub fn seek_to_sample(&mut self, target: u64) -> io::Result<()> {
        if target == 0 {
            self.bp = self.audio_start * 8;
            self.block_filled = 0;
            self.block_read = 0;
            return Ok(());
        }
        let mut from = self.audio_start;
        let mut from_sample = 0u64;
        for i in 0..self.seek_samples.len() {
            let s = self.seek_samples[i];
            let off = self.seek_offsets[i] as usize;
            if s <= target && s >= from_sample && self.audio_start + off < self.data.len() {
                from = self.audio_start + off;
                from_sample = s;
            }
        }
        if self.seek_samples.is_empty() && self.total_samples > 0 {
            let k = (target as f64 / self.total_samples as f64).min(1.0);
            let mut est = self.audio_start as isize
                + ((self.data.len() - self.audio_start) as f64 * k) as isize
                - 4096;
            for tries in 0..24 {
                if est <= self.audio_start as isize {
                    break;
                }
                if let Some((pos, s)) = self.find_frame(est as usize)
                    && s <= target
                {
                    from = pos;
                    from_sample = s;
                    break;
                }
                est -= 32768isize << tries.min(6);
            }
        }
        self.bp = from * 8;
        self.block_filled = 0;
        self.block_read = 0;
        let mut skip = target.saturating_sub(from_sample);
        while skip > 0 {
            if !self.decode_frame()? {
                return Ok(());
            }
            let n = (skip as usize).min(self.block_filled);
            self.block_read = n;
            skip -= n as u64;
            if self.block_read < self.block_filled {
                break;
            }
        }
        Ok(())
    }

    /// Reads interleaved `i16` PCM frames into `out`, returning number of `i16` samples written.
    pub fn read_i16(&mut self, out: &mut [i16]) -> io::Result<usize> {
        let mut written = 0usize;
        while written + self.out_channels <= out.len() {
            if self.block_read >= self.block_filled {
                if !self.decode_frame()? {
                    break;
                }
                continue;
            }
            let shift = self.bps_of_block as isize - 16;
            for c in 0..self.out_channels {
                let v = self.ch[c][self.block_read];
                let s = if shift > 0 { v >> shift } else { v << (-shift) };
                out[written] = s.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                written += 1;
            }
            self.block_read += 1;
        }
        Ok(written)
    }

    fn find_frame(&mut self, from: usize) -> Option<(usize, u64)> {
        let start = from.max(self.audio_start);
        for i in start..self.data.len().saturating_sub(16) {
            if self.data[i] != 0xFF || (self.data[i + 1] >> 1) != 0x7C {
                continue;
            }
            let save = self.bp;
            self.bp = i * 8;
            let res = self.read_header_sample();
            self.bp = save;
            if let Ok(Some(sample)) = res {
                return Some((i, sample));
            }
        }
        None
    }

    fn decode_frame(&mut self) -> io::Result<bool> {
        loop {
            let start = (self.bp + 7) >> 3;
            if start + 4 >= self.data.len() {
                return Ok(false);
            }
            self.bp = start * 8;
            match self.try_decode_frame() {
                Ok(true) => return Ok(true),
                _ => {
                    let Some((pos, _)) = self.find_frame(start + 1) else {
                        return Ok(false);
                    };
                    self.bp = pos * 8;
                }
            }
        }
    }

    fn try_decode_frame(&mut self) -> io::Result<bool> {
        let Some(_) = self.read_header_sample()? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad frame header",
            ));
        };
        self.decode_subframes()?;
        self.bp = ((self.bp + 7) >> 3) * 8 + 16; // skip CRC-16
        self.block_filled = self.block_size;
        self.block_read = 0;
        Ok(true)
    }

    fn read_header_sample(&mut self) -> io::Result<Option<u64>> {
        let start_byte = self.bp >> 3;
        if self.bits(15)? != 0x7FFC {
            return Ok(None);
        }
        let variable = self.bits(1)? == 1;
        let bs_code = self.bits(4)? as u8;
        let sr_code = self.bits(4)? as u8;
        let ch_code = self.bits(4)? as u8;
        let ss_code = self.bits(3)? as u8;
        if self.bits(1)? != 0 || bs_code == 0 || sr_code == 15 || ch_code > 10 || ss_code == 3 {
            return Ok(None);
        }
        let Some(num) = self.utf8()? else {
            return Ok(None);
        };
        let bs = match bs_code {
            1 => 192,
            2..=5 => 576 << (bs_code - 2),
            6 => self.bits(8)? as usize + 1,
            7 => self.bits(16)? as usize + 1,
            _ => 256 << (bs_code - 8),
        };
        if sr_code == 12 {
            let _ = self.bits(8)?;
        } else if sr_code == 13 || sr_code == 14 {
            let _ = self.bits(16)?;
        }
        let crc_at = self.bp >> 3;
        let crc = self.bits(8)? as u8;
        if crc8(&self.data[start_byte..crc_at]) != crc {
            return Ok(None);
        }
        if bs > self.ch[0].len() {
            return Ok(None);
        }
        let bps = match ss_code {
            0 => self.bits_per_sample,
            1 => 8,
            2 => 12,
            4 => 16,
            5 => 20,
            6 => 24,
            _ => 32,
        };
        let chans = if ch_code < 8 { ch_code as usize + 1 } else { 2 };
        if chans != self.channels {
            return Ok(None);
        }
        self.block_size = bs;
        self.hdr_channels = chans;
        self.hdr_assign = ch_code;
        self.hdr_bps = bps;
        let sample = if variable {
            num
        } else {
            let blk = if self.fixed_block > 0 {
                self.fixed_block
            } else {
                bs
            };
            num.saturating_mul(blk as u64)
        };
        Ok(Some(sample))
    }

    fn decode_subframes(&mut self) -> io::Result<()> {
        for c in 0..self.hdr_channels {
            let mut bps = self.hdr_bps;
            if (self.hdr_assign == 8 && c == 1)
                || (self.hdr_assign == 9 && c == 0)
                || (self.hdr_assign == 10 && c == 1)
            {
                bps += 1;
            }
            self.subframe(c, bps)?;
        }
        let n = self.block_size;
        if self.hdr_channels > 1 {
            let (first, rest) = self.ch.split_at_mut(1);
            let a = &mut first[0];
            let b = &mut rest[0];
            match self.hdr_assign {
                8 => {
                    for i in 0..n {
                        b[i] = a[i].wrapping_sub(b[i]);
                    }
                }
                9 => {
                    for i in 0..n {
                        a[i] = a[i].wrapping_add(b[i]);
                    }
                }
                10 => {
                    for i in 0..n {
                        let side = b[i];
                        let mid = (a[i] << 1) | (side & 1);
                        a[i] = (mid + side) >> 1;
                        b[i] = (mid - side) >> 1;
                    }
                }
                _ => {}
            }
        }
        self.bps_of_block = self.hdr_bps;
        Ok(())
    }

    fn subframe(&mut self, ch_idx: usize, bps: usize) -> io::Result<()> {
        if self.bits(1)? != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad subframe"));
        }
        let typ = self.bits(6)?;
        let mut wasted = 0usize;
        if self.bits(1)? == 1 {
            wasted = 1;
            while self.bits(1)? == 0 {
                wasted += 1;
                if wasted >= bps {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bad wasted bits",
                    ));
                }
            }
        }
        let eb = bps - wasted;
        let n = self.block_size;
        if typ == 0 {
            let v = self.sbits(eb)?;
            for i in 0..n {
                self.ch[ch_idx][i] = v;
            }
        } else if typ == 1 {
            for i in 0..n {
                self.ch[ch_idx][i] = self.sbits(eb)?;
            }
        } else if (8..=12).contains(&typ) {
            let order = (typ - 8) as usize;
            for i in 0..order {
                self.ch[ch_idx][i] = self.sbits(eb)?;
            }
            self.residual(ch_idx, order)?;
            self.fixed(ch_idx, order);
        } else if typ >= 32 {
            let order = (typ - 31) as usize;
            for i in 0..order {
                self.ch[ch_idx][i] = self.sbits(eb)?;
            }
            let precision = self.bits(4)? as usize + 1;
            if precision == 16 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad LPC precision",
                ));
            }
            let shift = self.sbits(5)?.max(0);
            let mut coef = vec![0i32; order];
            for c in &mut coef {
                *c = self.sbits(precision)?;
            }
            self.residual(ch_idx, order)?;
            self.lpc(ch_idx, order, &coef, shift as u32);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reserved subframe type",
            ));
        }
        if wasted > 0 {
            for i in 0..n {
                self.ch[ch_idx][i] <<= wasted;
            }
        }
        Ok(())
    }

    fn fixed(&mut self, ch_idx: usize, order: usize) {
        let s = &mut self.ch[ch_idx];
        let n = self.block_size;
        match order {
            1 => {
                for i in 1..n {
                    s[i] = s[i].wrapping_add(s[i - 1]);
                }
            }
            2 => {
                for i in 2..n {
                    s[i] = s[i].wrapping_add(2 * s[i - 1] - s[i - 2]);
                }
            }
            3 => {
                for i in 3..n {
                    s[i] = s[i].wrapping_add(3 * s[i - 1] - 3 * s[i - 2] + s[i - 3]);
                }
            }
            4 => {
                for i in 4..n {
                    s[i] = s[i].wrapping_add(4 * s[i - 1] - 6 * s[i - 2] + 4 * s[i - 3] - s[i - 4]);
                }
            }
            _ => {}
        }
    }

    fn lpc(&mut self, ch_idx: usize, order: usize, coef: &[i32], shift: u32) {
        let s = &mut self.ch[ch_idx];
        let n = self.block_size;
        for i in order..n {
            let mut sum = 0i64;
            for j in 0..order {
                sum += i64::from(coef[j]) * i64::from(s[i - 1 - j]);
            }
            s[i] = s[i].wrapping_add((sum >> shift) as i32);
        }
    }

    fn residual(&mut self, ch_idx: usize, order: usize) -> io::Result<()> {
        let method = self.bits(2)?;
        if method > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad residual coding",
            ));
        }
        let param_bits = if method == 0 { 4 } else { 5 };
        let escape = if method == 0 { 15 } else { 31 };
        let part_order = self.bits(4)? as usize;
        let parts = 1usize << part_order;
        let per = self.block_size >> part_order;
        let mut i = order;
        for p in 0..parts {
            let count = if p == 0 {
                per.checked_sub(order)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad partition"))?
            } else {
                per
            };
            let param = self.bits(param_bits)?;
            if param == escape {
                let raw = self.bits(5)? as usize;
                for _ in 0..count {
                    self.ch[ch_idx][i] = if raw == 0 { 0 } else { self.sbits(raw)? };
                    i += 1;
                }
            } else {
                let p_bits = param as usize;
                for _ in 0..count {
                    let q = self.unary()?;
                    let lo = if p_bits == 0 { 0 } else { self.bits(p_bits)? };
                    let v = (q << p_bits) | lo;
                    self.ch[ch_idx][i] = ((v >> 1) as i32) ^ -((v & 1) as i32);
                    i += 1;
                }
            }
        }
        Ok(())
    }

    fn bits(&mut self, mut n: usize) -> io::Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        let end = self.bp + n;
        if ((end + 7) >> 3) > self.data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "FLAC EOF"));
        }
        let mut v = 0u32;
        while n > 0 {
            let off = self.bp & 7;
            let avail = 8 - off;
            let take = avail.min(n);
            let b = ((self.data[self.bp >> 3] as u32) >> (avail - take)) & ((1 << take) - 1);
            v = (v << take) | b;
            n -= take;
            self.bp += take;
        }
        Ok(v)
    }

    fn sbits(&mut self, n: usize) -> io::Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sample too wide",
            ));
        }
        if n == 32 {
            let hi = self.bits(16)?;
            let lo = self.bits(16)?;
            return Ok(((hi << 16) | lo) as i32);
        }
        let v = self.bits(n)? as i32;
        let shift = 32 - n;
        Ok((v << shift) >> shift)
    }

    fn unary(&mut self) -> io::Result<u32> {
        let mut q = 0u32;
        loop {
            let idx = self.bp >> 3;
            if idx >= self.data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "FLAC unary EOF",
                ));
            }
            let off = self.bp & 7;
            let b = self.data[idx].wrapping_shl(off as u32);
            if b == 0 {
                q += (8 - off) as u32;
                self.bp += 8 - off;
                continue;
            }
            let lead = b.leading_zeros() as usize;
            q += lead as u32;
            self.bp += lead + 1;
            return Ok(q);
        }
    }

    fn utf8(&mut self) -> io::Result<Option<u64>> {
        let b = self.bits(8)? as u8;
        if (b & 0x80) == 0 {
            return Ok(Some(u64::from(b)));
        }
        let (extra, mut v) = if (b & 0xE0) == 0xC0 {
            (1usize, u64::from(b & 0x1F))
        } else if (b & 0xF0) == 0xE0 {
            (2, u64::from(b & 0x0F))
        } else if (b & 0xF8) == 0xF0 {
            (3, u64::from(b & 0x07))
        } else if (b & 0xFC) == 0xF8 {
            (4, u64::from(b & 0x03))
        } else if (b & 0xFE) == 0xFC {
            (5, u64::from(b & 0x01))
        } else if b == 0xFE {
            (6, 0u64)
        } else {
            return Ok(None);
        };
        for _ in 0..extra {
            let c = self.bits(8)? as u8;
            if (c & 0xC0) != 0x80 {
                return Ok(None);
            }
            v = (v << 6) | u64::from(c & 0x3F);
        }
        Ok(Some(v))
    }
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if (crc & 0x80) != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn u16_be(d: &[u8], p: usize) -> usize {
    (usize::from(d[p]) << 8) | usize::from(d[p + 1])
}

fn u24(d: &[u8], p: usize) -> usize {
    (usize::from(d[p]) << 16) | (usize::from(d[p + 1]) << 8) | usize::from(d[p + 2])
}

fn u64_be(d: &[u8], p: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..8 {
        v = (v << 8) | u64::from(d[p + i]);
    }
    v
}

pub fn skip_id3(d: &[u8]) -> usize {
    if d.len() > 10 && &d[..3] == b"ID3" {
        let size = (usize::from(d[6] & 0x7F) << 21)
            | (usize::from(d[7] & 0x7F) << 14)
            | (usize::from(d[8] & 0x7F) << 7)
            | usize::from(d[9] & 0x7F);
        return 10 + size + if (d[5] & 0x10) != 0 { 10 } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_minimal_constant_flac_frame() {
        // Construct a minimal valid FLAC stream: 44100 Hz, 1 ch, 16-bit, 16 constant samples of value 1234.
        let mut flac = Vec::new();
        flac.extend_from_slice(b"fLaC");
        // STREAMINFO (last block = 1, type = 0, len = 34)
        flac.push(0x80);
        flac.extend_from_slice(&[0, 0, 34]);
        flac.extend_from_slice(&16u16.to_be_bytes()); // min block 16
        flac.extend_from_slice(&16u16.to_be_bytes()); // max block 16
        flac.extend_from_slice(&[0; 6]); // min/max frame size
        // packed 64 bits: sr=44100 (20b), ch-1=0 (3b), bps-1=15 (5b), total=16 (36b)
        let packed: u64 = (44100u64 << 44) | (15u64 << 36) | 16u64;
        flac.extend_from_slice(&packed.to_be_bytes());
        flac.extend_from_slice(&[0u8; 16]); // md5

        // Frame header: sync 0x7FFC (15b), var=0 (1b) => 0xFF, 0xF8
        // bs_code=6 (8-bit size-1), sr_code=9 (44100) => 0x69
        // ch_code=0 (1ch), ss_code=4 (16-bit), reserved=0 => 0x08
        // utf8 sample/frame number = 0x00
        // block size - 1 = 15 (0x0F)
        let hdr_start = flac.len();
        flac.extend_from_slice(&[0xFF, 0xF8, 0x69, 0x08, 0x00, 0x0F]);
        let crc = crc8(&flac[hdr_start..]);
        flac.push(crc);
        // Subframe CONSTANT (0 | 000000 | 0 = 0x00) + 16-bit sample 1234 (0x04D2)
        flac.push(0x00);
        flac.extend_from_slice(&1234i16.to_be_bytes());
        // CRC-16 footer
        flac.extend_from_slice(&[0x00, 0x00]);

        let mut dec = FlacDecoder::new(flac).expect("valid minimal FLAC");
        assert_eq!(dec.sample_rate, 44100);
        assert_eq!(dec.duration_ms(), 0);
        let mut pcm = vec![0i16; 16];
        let n = dec.read_i16(&mut pcm).expect("decode frame");
        assert_eq!(n, 16);
        assert!(pcm.iter().all(|&s| s == 1234));
    }
}
