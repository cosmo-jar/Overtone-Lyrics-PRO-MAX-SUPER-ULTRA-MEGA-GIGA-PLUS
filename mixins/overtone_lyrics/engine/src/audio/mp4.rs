use std::io;

/// Extracts raw AAC frames + AudioSpecificConfig from MP4 or fragmented fMP4 (SoundCloud HLS).
pub struct Mp4Audio {
    data: Vec<u8>,
    config: Vec<u8>,
    timescale: u64,
    track_id: u32,
    default_duration: u64,
    default_size: u64,
    offsets: Vec<usize>,
    sizes: Vec<usize>,
    starts: Vec<u64>,
    clock: u64,
}

impl Mp4Audio {
    pub fn looks_like_mp4(d: &[u8]) -> bool {
        if d.len() < 12 {
            return false;
        }
        matches!(&d[4..8], b"ftyp" | b"styp" | b"moov" | b"moof")
    }

    pub fn parse(data: Vec<u8>) -> io::Result<Self> {
        let mut m = Self {
            data,
            config: Vec::new(),
            timescale: 1000,
            track_id: u32::MAX,
            default_duration: 0,
            default_size: 0,
            offsets: Vec::new(),
            sizes: Vec::new(),
            starts: Vec::new(),
            clock: 0,
        };
        m.walk_top_level();
        if m.config.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "No AAC audio in MP4",
            ));
        }
        if m.offsets.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "No AAC frames in MP4",
            ));
        }
        Ok(m)
    }

    pub fn decoder_config(&self) -> &[u8] {
        &self.config
    }

    pub fn count(&self) -> usize {
        self.offsets.len()
    }

    pub fn frame(&self, i: usize) -> &[u8] {
        &self.data[self.offsets[i]..self.offsets[i] + self.sizes[i]]
    }

    pub fn duration_ms(&self) -> u64 {
        self.clock.saturating_mul(1000) / self.timescale.max(1)
    }

    pub fn index_at(&self, ms: u64) -> usize {
        if self.starts.is_empty() {
            return 0;
        }
        let target = ms.saturating_mul(self.timescale) / 1000;
        let mut lo = 0usize;
        let mut hi = self.starts.len() - 1;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.starts[mid] <= target {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    fn walk_top_level(&mut self) {
        let mut pos = 0usize;
        while pos + 8 <= self.data.len() {
            let mut size = self.u32(pos) as u64;
            let mut header = 8usize;
            if size == 1 && pos + 16 <= self.data.len() {
                size = self.u64(pos + 8);
                header = 16;
            } else if size == 0 {
                size = (self.data.len() - pos) as u64;
            }
            if size < header as u64 {
                return;
            }
            let end = ((pos as u64).saturating_add(size)).min(self.data.len() as u64) as usize;
            let t = self.box_type(pos + 4);
            if t == *b"moov" {
                self.read_moov(pos + header, end);
            } else if t == *b"moof" && end as u64 == pos as u64 + size {
                self.read_moof(pos, pos + header, end);
            }
            pos = end;
        }
    }

    fn read_moov(&mut self, start: usize, end: usize) {
        let mut p = start;
        while p + 8 <= end {
            let size = self.u32(p) as usize;
            if size < 8 || p + size > end {
                return;
            }
            let t = self.box_type(p + 4);
            if t == *b"trak" && self.config.is_empty() {
                self.read_trak(p + 8, p + size);
            } else if t == *b"mvex" {
                self.read_mvex(p + 8, p + size);
            }
            p += size;
        }
    }

    fn read_mvex(&mut self, start: usize, end: usize) {
        let mut p = start;
        while p + 8 <= end {
            let size = self.u32(p) as usize;
            if size < 8 || p + size > end {
                return;
            }
            if self.box_type(p + 4) == *b"trex"
                && p + 28 <= p + size
                && self.u32(p + 12) == self.track_id
            {
                self.default_duration = u64::from(self.u32(p + 20));
                self.default_size = u64::from(self.u32(p + 24));
            }
            p += size;
        }
    }

    fn read_trak(&mut self, start: usize, end: usize) {
        let Some((tk_s, _)) = self.child(start, end, b"tkhd") else {
            return;
        };
        let Some((md_s, md_e)) = self.child(start, end, b"mdia") else {
            return;
        };
        let Some((hd_s, _)) = self.child(md_s, md_e, b"hdlr") else {
            return;
        };
        if hd_s + 12 > md_e || &self.data[hd_s + 8..hd_s + 12] != b"soun" {
            return;
        }
        let Some((mh_s, _)) = self.child(md_s, md_e, b"mdhd") else {
            return;
        };
        let Some((mi_s, mi_e)) = self.child(md_s, md_e, b"minf") else {
            return;
        };
        let Some((st_s, st_e)) = self.child(mi_s, mi_e, b"stbl") else {
            return;
        };
        let Some((sd_s, sd_e)) = self.child(st_s, st_e, b"stsd") else {
            return;
        };
        let Some(cfg) = self.read_entry_config(sd_s + 8, sd_e) else {
            return;
        };
        let tk_ver = self.data.get(tk_s).copied().unwrap_or(0);
        let mh_ver = self.data.get(mh_s).copied().unwrap_or(0);
        self.track_id = self.u32(tk_s + if tk_ver == 1 { 20 } else { 12 });
        self.timescale = u64::from(self.u32(mh_s + if mh_ver == 1 { 20 } else { 12 })).max(1);
        self.config = cfg;
        self.read_flat_samples(st_s, st_e);
    }

    fn read_entry_config(&self, start: usize, end: usize) -> Option<Vec<u8>> {
        if start + 36 > end || &self.data[start + 4..start + 8] != b"mp4a" {
            return None;
        }
        let version =
            (usize::from(self.data[start + 16]) << 8) | usize::from(self.data[start + 17]);
        let children = start
            + 8
            + 28
            + match version {
                1 => 16,
                2 => 36,
                _ => 0,
            };
        let box_end = start + self.u32(start) as usize;
        let (es_s, es_e) = self.child(children, box_end.min(end), b"esds")?;
        self.read_descriptors(es_s + 4, es_e)
    }

    fn read_descriptors(&self, mut pos: usize, end: usize) -> Option<Vec<u8>> {
        while pos + 2 <= end {
            let tag = self.data[pos];
            pos += 1;
            let mut len = 0usize;
            for _ in 0..4 {
                if pos >= end {
                    break;
                }
                let b = self.data[pos];
                pos += 1;
                len = (len << 7) | usize::from(b & 0x7F);
                if (b & 0x80) == 0 {
                    break;
                }
            }
            let body_end = end.min(pos + len);
            if tag == 0x05 {
                return Some(self.data[pos..body_end].to_vec());
            } else if tag == 0x03 && pos + 3 <= body_end {
                let flags = self.data[pos + 2];
                let mut inner = pos + 3 + if (flags & 0x80) != 0 { 2 } else { 0 };
                if (flags & 0x40) != 0 && inner < body_end {
                    inner += 1 + usize::from(self.data[inner]);
                }
                if (flags & 0x20) != 0 {
                    inner += 2;
                }
                if let Some(r) = self.read_descriptors(inner, body_end) {
                    return Some(r);
                }
            } else if tag == 0x04
                && pos + 13 <= body_end
                && let Some(r) = self.read_descriptors(pos + 13, body_end)
            {
                return Some(r);
            }
            pos = body_end;
        }
        None
    }

    fn read_flat_samples(&mut self, start: usize, end: usize) {
        let Some((stsz_s, _)) = self.child(start, end, b"stsz") else {
            return;
        };
        let Some((stsc_s, _)) = self.child(start, end, b"stsc") else {
            return;
        };
        let Some((stts_s, _)) = self.child(start, end, b"stts") else {
            return;
        };
        let stco = self.child(start, end, b"stco");
        let co64 = self.child(start, end, b"co64");
        let Some((chunk_s, _)) = stco.or(co64) else {
            return;
        };

        let n = self.u32(stsz_s + 8) as usize;
        if n == 0 {
            return;
        }
        let fixed = self.u32(stsz_s + 4) as usize;
        let chunks = self.u32(chunk_s + 4) as usize;
        let runs = self.u32(stsc_s + 4) as usize;

        let mut durations = vec![0u64; n];
        let mut di = 0usize;
        let entries = self.u32(stts_s + 4) as usize;
        let mut p = stts_s + 8;
        for _ in 0..entries {
            if di >= n {
                break;
            }
            let cnt = self.u32(p) as usize;
            let delta = u64::from(self.u32(p + 4));
            for _ in 0..cnt {
                if di >= n {
                    break;
                }
                durations[di] = delta;
                di += 1;
            }
            p += 8;
        }

        let mut sample = 0usize;
        for run in 0..runs {
            if sample >= n {
                break;
            }
            let rp = stsc_s + 8 + run * 12;
            let first_chunk = self.u32(rp) as usize;
            let per_chunk = self.u32(rp + 4) as usize;
            let last_chunk = if run + 1 < runs {
                (self.u32(rp + 12) as usize).saturating_sub(1)
            } else {
                chunks
            };
            for c in first_chunk..=last_chunk {
                if sample >= n {
                    break;
                }
                let mut off = if stco.is_some() {
                    u64::from(self.u32(chunk_s + 8 + (c - 1) * 4))
                } else {
                    self.u64(chunk_s + 8 + (c - 1) * 8)
                };
                for _ in 0..per_chunk {
                    if sample >= n {
                        break;
                    }
                    let size = if fixed != 0 {
                        fixed
                    } else {
                        self.u32(stsz_s + 12 + sample * 4) as usize
                    };
                    self.add(off, size, durations[sample]);
                    off += size as u64;
                    sample += 1;
                }
            }
        }
    }

    fn read_moof(&mut self, moof_start: usize, start: usize, end: usize) {
        let mut p = start;
        while p + 8 <= end {
            let size = self.u32(p) as usize;
            if size < 8 || p + size > end {
                return;
            }
            if self.box_type(p + 4) == *b"traf" {
                self.read_traf(moof_start, p + 8, p + size);
            }
            p += size;
        }
    }

    fn read_traf(&mut self, moof_start: usize, start: usize, end: usize) {
        let Some((tfhd_s, _)) = self.child(start, end, b"tfhd") else {
            return;
        };
        let flags = self.u32(tfhd_s) & 0xFFFFFF;
        if self.u32(tfhd_s + 4) != self.track_id {
            return;
        }
        let mut at = tfhd_s + 8;
        let mut base = moof_start as u64;
        let mut dur = self.default_duration;
        let mut sz = self.default_size;
        if (flags & 0x1) != 0 {
            base = self.u64(at);
            at += 8;
        }
        if (flags & 0x2) != 0 {
            at += 4;
        }
        if (flags & 0x8) != 0 {
            dur = u64::from(self.u32(at));
            at += 4;
        }
        if (flags & 0x10) != 0 {
            sz = u64::from(self.u32(at));
        }

        let mut cursor = base;
        let mut p = start;
        while p + 8 <= end {
            let box_size = self.u32(p) as usize;
            if box_size < 8 || p + box_size > end {
                return;
            }
            if self.box_type(p + 4) == *b"trun" {
                cursor = self.read_trun(p + 8, base, cursor, dur, sz);
            }
            p += box_size;
        }
    }

    fn read_trun(
        &mut self,
        mut at: usize,
        base: u64,
        mut cursor: u64,
        def_dur: u64,
        def_sz: u64,
    ) -> u64 {
        let flags = self.u32(at) & 0xFFFFFF;
        let samples = self.u32(at + 4) as usize;
        at += 8;
        if (flags & 0x1) != 0 {
            let rel = self.u32(at) as i32;
            cursor = (base as i64 + i64::from(rel)).max(0) as u64;
            at += 4;
        }
        if (flags & 0x4) != 0 {
            at += 4;
        }
        for _ in 0..samples {
            let mut dur = def_dur;
            let mut size = def_sz;
            if (flags & 0x100) != 0 {
                dur = u64::from(self.u32(at));
                at += 4;
            }
            if (flags & 0x200) != 0 {
                size = u64::from(self.u32(at));
                at += 4;
            }
            if (flags & 0x400) != 0 {
                at += 4;
            }
            if (flags & 0x800) != 0 {
                at += 4;
            }
            self.add(cursor, size as usize, dur);
            cursor += size;
        }
        cursor
    }

    fn add(&mut self, offset: u64, size: usize, duration: u64) {
        if size == 0 || (offset as usize).saturating_add(size) > self.data.len() {
            return;
        }
        self.offsets.push(offset as usize);
        self.sizes.push(size);
        self.starts.push(self.clock);
        self.clock += duration;
    }

    fn child(&self, start: usize, end: usize, target: &[u8; 4]) -> Option<(usize, usize)> {
        let mut p = start;
        while p + 8 <= end {
            let size = self.u32(p) as usize;
            if size < 8 || p + size > end {
                return None;
            }
            if self.box_type(p + 4) == *target {
                return Some((p + 8, p + size));
            }
            p += size;
        }
        None
    }

    fn u32(&self, p: usize) -> u32 {
        if p + 4 > self.data.len() {
            0
        } else {
            u32::from_be_bytes([
                self.data[p],
                self.data[p + 1],
                self.data[p + 2],
                self.data[p + 3],
            ])
        }
    }

    fn u64(&self, p: usize) -> u64 {
        (u64::from(self.u32(p)) << 32) | u64::from(self.u32(p + 4))
    }

    fn box_type(&self, p: usize) -> [u8; 4] {
        if p + 4 > self.data.len() {
            [0; 4]
        } else {
            [
                self.data[p],
                self.data[p + 1],
                self.data[p + 2],
                self.data[p + 3],
            ]
        }
    }
}
