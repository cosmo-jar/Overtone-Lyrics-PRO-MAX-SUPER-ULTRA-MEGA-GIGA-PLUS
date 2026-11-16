use std::io::{self, Cursor};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use symphonia::default::{get_codecs, get_probe};

pub struct OggSource {
    pub sample_rate: u32,
    pub channels: usize,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    pending: Vec<i16>,
    cursor: usize,
}

impl OggSource {
    pub fn open(raw: Vec<u8>, start_ms: u64) -> io::Result<Self> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(raw)), Default::default());
        let probed = get_probe()
            .format(
                &Default::default(),
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Ogg audio track"))?;
        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        let channels = track
            .codec_params
            .channels
            .map_or(2, |c| c.count())
            .clamp(1, 2);

        let decoder = get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        if start_ms > 0 {
            let _ = format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(start_ms as f64 / 1000.0),
                    track_id: Some(track_id),
                },
            );
        }

        Ok(Self {
            sample_rate,
            channels,
            format,
            decoder,
            track_id,
            pending: Vec::new(),
            cursor: 0,
        })
    }

    pub fn duration_ms(raw: Vec<u8>) -> u64 {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(raw)), Default::default());
        let Ok(probed) = get_probe().format(
            &Default::default(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) else {
            return 0;
        };
        let Some(track) = probed.format.default_track() else {
            return 0;
        };
        if let (Some(rate), Some(n_frames)) =
            (track.codec_params.sample_rate, track.codec_params.n_frames)
        {
            return n_frames.saturating_mul(1000) / u64::from(rate.max(1));
        }
        0
    }

    pub fn read_i16(&mut self, out: &mut [i16]) -> io::Result<usize> {
        let mut written = 0usize;
        while written < out.len() {
            if self.cursor >= self.pending.len() && !self.decode_packet()? {
                break;
            }
            let avail = self.pending.len() - self.cursor;
            let take = avail.min(out.len() - written);
            out[written..written + take]
                .copy_from_slice(&self.pending[self.cursor..self.cursor + take]);
            self.cursor += take;
            written += take;
        }
        Ok(written)
    }

    fn decode_packet(&mut self) -> io::Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return Ok(false),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let Ok(decoded) = self.decoder.decode(&packet) else {
                continue;
            };
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            let mut sb = SampleBuffer::<i16>::new(frames as u64, spec);
            sb.copy_interleaved_ref(decoded);
            let chans = spec.channels.count().max(1);
            let samples = sb.samples();
            self.pending.clear();
            if chans <= 2 {
                self.pending.extend_from_slice(samples);
            } else {
                for frame in samples.chunks_exact(chans) {
                    self.pending.push(frame[0]);
                    self.pending.push(frame[1]);
                }
            }
            self.cursor = 0;
            return Ok(!self.pending.is_empty());
        }
    }
}
