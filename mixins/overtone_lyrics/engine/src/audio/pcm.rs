use std::io;

pub struct PcmSource {
    pub sample_rate: u32,
    pub channels: usize,
    samples: Vec<i16>,
    cursor: usize,
}

impl PcmSource {
    pub fn open(raw: &[u8], start_ms: u64) -> io::Result<Self> {
        let (sample_rate, channels, samples) =
            if raw.len() >= 12 && &raw[..4] == b"RIFF" && &raw[8..12] == b"WAVE" {
                decode_wav(raw)?
            } else if raw.len() >= 12
                && &raw[..4] == b"FORM"
                && (&raw[8..12] == b"AIFF" || &raw[8..12] == b"AIFC")
            {
                decode_aiff(raw)?
            } else if raw.len() >= 24 && &raw[..4] == b".snd" {
                decode_au(raw)?
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported PCM container",
                ));
            };
        let skip_frames = ((start_ms as f64 * f64::from(sample_rate)) / 1000.0) as usize;
        let cursor = (skip_frames * channels).min(samples.len());
        Ok(Self {
            sample_rate,
            channels,
            samples,
            cursor,
        })
    }

    pub fn read_i16(&mut self, out: &mut [i16]) -> usize {
        if self.cursor >= self.samples.len() {
            return 0;
        }
        let avail = self.samples.len() - self.cursor;
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.samples[self.cursor..self.cursor + n]);
        self.cursor += n;
        n
    }

    pub fn duration_ms(raw: &[u8]) -> u64 {
        Self::open(raw, 0).map_or(0, |s| {
            let frames = s.samples.len() / s.channels.max(1);
            (frames as u64).saturating_mul(1000) / u64::from(s.sample_rate.max(1))
        })
    }
}

fn decode_wav(d: &[u8]) -> io::Result<(u32, usize, Vec<i16>)> {
    let mut pos = 12usize;
    let mut fmt_tag = 0u16;
    let mut chans = 0usize;
    let mut rate = 0u32;
    let mut bps = 0u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32::from_le_bytes([d[pos + 4], d[pos + 5], d[pos + 6], d[pos + 7]]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(d.len());
        if id == b"fmt " && end >= body + 16 {
            fmt_tag = u16::from_le_bytes([d[body], d[body + 1]]);
            chans = usize::from(u16::from_le_bytes([d[body + 2], d[body + 3]]));
            rate = u32::from_le_bytes([d[body + 4], d[body + 5], d[body + 6], d[body + 7]]);
            bps = u16::from_le_bytes([d[body + 14], d[body + 15]]);
        } else if id == b"data" {
            data_slice = Some(&d[body..end]);
        }
        pos = end + (size & 1);
    }

    let raw = data_slice
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "WAV without data chunk"))?;
    if chans == 0 || rate == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WAV without fmt chunk",
        ));
    }
    let samples = convert_pcm_bytes(raw, fmt_tag == 3, bps, false)?;
    Ok((rate, chans.min(2), downsample_to_max2(samples, chans)))
}

fn decode_aiff(d: &[u8]) -> io::Result<(u32, usize, Vec<i16>)> {
    let mut pos = 12usize;
    let mut chans = 0usize;
    let mut bps = 0u16;
    let mut rate = 0u32;
    let mut ssnd: Option<&[u8]> = None;

    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32::from_be_bytes([d[pos + 4], d[pos + 5], d[pos + 6], d[pos + 7]]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(d.len());
        if id == b"COMM" && end >= body + 18 {
            chans = usize::from(u16::from_be_bytes([d[body], d[body + 1]]));
            bps = u16::from_be_bytes([d[body + 6], d[body + 7]]);
            rate = parse_ieee_ext80(&d[body + 8..body + 18]);
        } else if id == b"SSND" && end >= body + 8 {
            let offset =
                u32::from_be_bytes([d[body], d[body + 1], d[body + 2], d[body + 3]]) as usize;
            let start = (body + 8 + offset).min(end);
            ssnd = Some(&d[start..end]);
        }
        pos = end + (size & 1);
    }

    let raw =
        ssnd.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "AIFF without SSND"))?;
    if chans == 0 || rate == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "AIFF without COMM",
        ));
    }
    let samples = convert_pcm_bytes(raw, false, bps, true)?;
    Ok((rate, chans.min(2), downsample_to_max2(samples, chans)))
}

fn decode_au(d: &[u8]) -> io::Result<(u32, usize, Vec<i16>)> {
    let offset = u32::from_be_bytes([d[4], d[5], d[6], d[7]]) as usize;
    let encoding = u32::from_be_bytes([d[12], d[13], d[14], d[15]]);
    let rate = u32::from_be_bytes([d[16], d[17], d[18], d[19]]);
    let chans = u32::from_be_bytes([d[20], d[21], d[22], d[23]]) as usize;
    if offset >= d.len() || chans == 0 || rate == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid AU header",
        ));
    }
    let (is_float, bps) = match encoding {
        2 => (false, 8u16),
        3 => (false, 16),
        4 => (false, 24),
        5 => (false, 32),
        6 => (true, 32),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported AU encoding",
            ));
        }
    };
    let samples = convert_pcm_bytes(&d[offset..], is_float, bps, true)?;
    Ok((rate, chans.min(2), downsample_to_max2(samples, chans)))
}

fn convert_pcm_bytes(
    raw: &[u8],
    is_float: bool,
    bps: u16,
    big_endian: bool,
) -> io::Result<Vec<i16>> {
    match (is_float, bps) {
        (false, 8) => Ok(raw.iter().map(|&b| ((b as i16) - 128) << 8).collect()),
        (false, 16) => Ok(raw
            .chunks_exact(2)
            .map(|c| {
                if big_endian {
                    i16::from_be_bytes([c[0], c[1]])
                } else {
                    i16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect()),
        (false, 24) => Ok(raw
            .chunks_exact(3)
            .map(|c| {
                if big_endian {
                    i16::from_be_bytes([c[0], c[1]])
                } else {
                    i16::from_le_bytes([c[1], c[2]])
                }
            })
            .collect()),
        (false, 32) => Ok(raw
            .chunks_exact(4)
            .map(|c| {
                let v = if big_endian {
                    i32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    i32::from_le_bytes([c[0], c[1], c[2], c[3]])
                };
                (v >> 16) as i16
            })
            .collect()),
        (true, 32) => Ok(raw
            .chunks_exact(4)
            .map(|c| {
                let f = if big_endian {
                    f32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    f32::from_le_bytes([c[0], c[1], c[2], c[3]])
                };
                (f.clamp(-1.0, 1.0) * 32767.0) as i16
            })
            .collect()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PCM bit depth",
        )),
    }
}

fn downsample_to_max2(samples: Vec<i16>, chans: usize) -> Vec<i16> {
    if chans <= 2 {
        return samples;
    }
    let mut out = Vec::with_capacity((samples.len() / chans) * 2);
    for frame in samples.chunks_exact(chans) {
        out.push(frame[0]);
        out.push(frame[1]);
    }
    out
}

fn parse_ieee_ext80(b: &[u8]) -> u32 {
    let exp = (((u16::from(b[0]) & 0x7F) << 8) | u16::from(b[1])) as i32 - 16383;
    let mut mant = 0u64;
    for &byte in &b[2..10] {
        mant = (mant << 8) | u64::from(byte);
    }
    if !(0..=31).contains(&exp) {
        return 44100;
    }
    ((mant >> (63 - exp)) & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_wav_and_computes_duration() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + 44100 * 4).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        for i in 0..44100 * 2 {
            let s = ((i % 100) as i16) * 200;
            wav.extend_from_slice(&s.to_le_bytes());
        }
        assert_eq!(PcmSource::duration_ms(&wav), 1000);
        let mut src = PcmSource::open(&wav, 500).unwrap();
        let mut buf = vec![0i16; 1024];
        let n = src.read_i16(&mut buf);
        assert_eq!(n, 1024);
    }
}
