pub mod aac;
pub mod aac_tables;
pub mod flac;
pub mod mp3;
pub mod mp3_tables;
pub mod mp4;
pub mod ogg;
pub mod pcm;

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};

use self::aac::{AacSource, probe_aac_duration_ms};
use self::flac::FlacDecoder;
use self::mp3::OwnedMp3Source;
use self::mp4::Mp4Audio;
use self::ogg::OggSource;
use self::pcm::PcmSource;
use crate::config::PlayerConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    Mp3,
    Flac,
    Ogg,
    Aac,
    Pcm,
}

impl AudioFormat {
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
            return Some(Self::Ogg);
        }
        if bytes.len() >= 4 && &bytes[..4] == b"fLaC" {
            return Some(Self::Flac);
        }
        if Mp4Audio::looks_like_mp4(bytes) {
            return Some(Self::Aac);
        }
        if bytes.len() >= 12
            && ((&bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE")
                || (&bytes[..4] == b"FORM"
                    && (&bytes[8..12] == b"AIFF" || &bytes[8..12] == b"AIFC")))
        {
            return Some(Self::Pcm);
        }
        if bytes.len() >= 24 && &bytes[..4] == b".snd" {
            return Some(Self::Pcm);
        }
        if bytes.len() >= 10 && &bytes[..3] == b"ID3" {
            return Some(Self::Mp3);
        }
        if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
            return Some(Self::Mp3);
        }
        None
    }
}

pub fn probe_duration_ms(fmt: AudioFormat, bytes: &[u8]) -> u64 {
    match fmt {
        AudioFormat::Mp3 => mp3::probe_duration_ms(bytes).max(0) as u64,
        AudioFormat::Flac => FlacDecoder::new(bytes.to_vec()).map_or(0, |d| d.duration_ms()),
        AudioFormat::Ogg => OggSource::duration_ms(bytes.to_vec()),
        AudioFormat::Aac => probe_aac_duration_ms(bytes),
        AudioFormat::Pcm => PcmSource::duration_ms(bytes),
    }
}

enum DecoderSource {
    Mp3(Box<OwnedMp3Source>),
    Flac(Box<FlacDecoder>),
    Ogg(OggSource),
    Aac(Box<AacSource>),
    Pcm(PcmSource),
}

impl DecoderSource {
    fn open(fmt: AudioFormat, raw: Vec<u8>, start_ms: u64) -> io::Result<Self> {
        match fmt {
            AudioFormat::Mp3 => Ok(Self::Mp3(Box::new(OwnedMp3Source::open(raw, start_ms)?))),
            AudioFormat::Flac => {
                let mut dec = FlacDecoder::new(raw)?;
                if start_ms > 0 {
                    let target_sample = start_ms.saturating_mul(u64::from(dec.sample_rate)) / 1000;
                    dec.seek_to_sample(target_sample)?;
                }
                Ok(Self::Flac(Box::new(dec)))
            }
            AudioFormat::Ogg => Ok(Self::Ogg(OggSource::open(raw, start_ms)?)),
            AudioFormat::Aac => Ok(Self::Aac(Box::new(AacSource::open(raw, start_ms)?))),
            AudioFormat::Pcm => Ok(Self::Pcm(PcmSource::open(&raw, start_ms)?)),
        }
    }

    fn sample_rate(&self) -> u32 {
        match self {
            Self::Mp3(s) => s.sample_rate.max(8000),
            Self::Flac(s) => s.sample_rate.max(8000),
            Self::Ogg(s) => s.sample_rate.max(8000),
            Self::Aac(s) => s.sample_rate.max(8000),
            Self::Pcm(s) => s.sample_rate.max(8000),
        }
    }

    fn channels(&self) -> usize {
        match self {
            Self::Mp3(s) => s.channels.clamp(1, 2),
            Self::Flac(s) => s.out_channels.clamp(1, 2),
            Self::Ogg(s) => s.channels.clamp(1, 2),
            Self::Aac(s) => s.channels.clamp(1, 2),
            Self::Pcm(s) => s.channels.clamp(1, 2),
        }
    }

    fn read_i16(&mut self, out: &mut [i16]) -> io::Result<usize> {
        match self {
            Self::Mp3(s) => Ok(s.read_i16(out)),
            Self::Flac(s) => s.read_i16(out),
            Self::Ogg(s) => s.read_i16(out),
            Self::Aac(s) => Ok(s.read_i16(out)),
            Self::Pcm(s) => Ok(s.read_i16(out)),
        }
    }
}

/// Decodes an entire encoded track to interleaved `i16` PCM for Level B Vocal-VAD DTW alignment.
pub fn decode_full_pcm_for_dsp(fmt: AudioFormat, raw: &[u8]) -> io::Result<(u32, usize, Vec<i16>)> {
    if fmt == AudioFormat::Mp3 {
        let (pcm, ch, rate) = mp3::decode_all_pcm16(raw)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Failed to decode MP3"))?;
        return Ok((rate, ch, pcm));
    }
    let mut src = DecoderSource::open(fmt, raw.to_vec(), 0)?;
    let rate = src.sample_rate();
    let ch = src.channels();
    let mut all = Vec::with_capacity((rate as usize) * ch * 120);
    let mut buf = [0i16; 8192];
    loop {
        let n = src.read_i16(&mut buf)?;
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
    }
    Ok((rate, ch, all))
}

/// Lists system audio output device names cleanly matching `Mp3Player.outputDevices()`.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devs) = host.output_devices() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in devs {
        if let Ok(name) = d.name()
            && !name.is_empty()
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

fn same_device_name(device_name: &str, openal_name: &str) -> bool {
    let m = device_name.trim().to_ascii_lowercase();
    let o = openal_name.trim().to_ascii_lowercase();
    let o = o.strip_prefix("openal soft on ").unwrap_or(&o).trim();
    if m.len() < 8 || o.len() < 8 {
        return false;
    }
    m == o || o.starts_with(&m) || m.starts_with(o)
}

fn pick_output_device(wanted_explicit: &str, game_device: &str) -> Vec<Device> {
    let host = cpal::default_host();
    let mut candidates = Vec::new();
    let all_devs: Vec<Device> = host
        .output_devices()
        .map(|it| it.collect())
        .unwrap_or_default();

    let wanted = wanted_explicit.trim();
    if !wanted.is_empty() {
        for d in &all_devs {
            if d.name().ok().as_deref() == Some(wanted) {
                candidates.push(d.clone());
                break;
            }
        }
    } else if !game_device.trim().is_empty() {
        for d in &all_devs {
            if let Ok(n) = d.name()
                && same_device_name(&n, game_device)
            {
                candidates.push(d.clone());
                break;
            }
        }
    }

    if let Some(def) = host.default_output_device() {
        candidates.push(def);
    }
    for d in all_devs {
        candidates.push(d);
    }
    candidates
}

fn pick_stream_config(dev: &Device) -> Option<(StreamConfig, SampleFormat)> {
    if let Ok(def) = dev.default_output_config() {
        let fmt = def.sample_format();
        let cfg = def.config();
        if cfg.channels >= 1 && cfg.sample_rate.0 >= 8000 {
            return Some((cfg, fmt));
        }
    }
    let supported = dev.supported_output_configs().ok()?;
    for range in supported {
        if range.channels() >= 2 {
            let rate = 44100.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
            let with_rate = range.with_sample_rate(cpal::SampleRate(rate));
            return Some((with_rate.config(), with_rate.sample_format()));
        }
    }
    None
}

struct SharedPlayback {
    ring: Mutex<VecDeque<i16>>,
    eof: AtomicBool,
    stop: AtomicBool,
    paused: AtomicBool,
    volume_bits: AtomicU32,
    game_factor_bits: AtomicU32,
    played_device_frames: AtomicU64,
    start_ms: u64,
    device_rate: u32,
    device_channels: usize,
}

impl SharedPlayback {
    fn new(
        start_ms: u64,
        volume: f32,
        game_factor: f32,
        device_rate: u32,
        device_channels: usize,
    ) -> Self {
        Self {
            ring: Mutex::new(VecDeque::with_capacity(
                (device_rate as usize * device_channels).max(4096),
            )),
            eof: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            volume_bits: AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()),
            game_factor_bits: AtomicU32::new(game_factor.clamp(0.0, 1.0).to_bits()),
            played_device_frames: AtomicU64::new(0),
            start_ms,
            device_rate: device_rate.max(8000),
            device_channels: device_channels.max(1),
        }
    }

    fn gain(&self) -> f32 {
        let v = f32::from_bits(self.volume_bits.load(Ordering::Relaxed));
        let g = f32::from_bits(self.game_factor_bits.load(Ordering::Relaxed));
        (v * g).clamp(0.0, 1.0)
    }

    fn position_ms(&self) -> u64 {
        let played = self.played_device_frames.load(Ordering::Relaxed);
        self.start_ms + played.saturating_mul(1000) / u64::from(self.device_rate.max(1))
    }
}

struct ActiveSession {
    shared: Arc<SharedPlayback>,
    stream: Option<Stream>,
    thread: Option<thread::JoinHandle<()>>,
    paused: bool,
}

impl ActiveSession {
    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(s) = self.stream.take() {
            let _ = s.pause();
            drop(s);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct LinearResampler {
    src_rate: u32,
    src_ch: usize,
    dst_rate: u32,
    dst_ch: usize,
    pos: f64,
    prev: [f32; 2],
}

impl LinearResampler {
    fn new(src_rate: u32, src_ch: usize, dst_rate: u32, dst_ch: usize) -> Self {
        Self {
            src_rate: src_rate.max(8000),
            src_ch: src_ch.clamp(1, 2),
            dst_rate: dst_rate.max(8000),
            dst_ch: dst_ch.max(1),
            pos: 0.0,
            prev: [0.0; 2],
        }
    }

    fn process(&mut self, src: &[i16], out: &mut Vec<i16>) {
        out.clear();
        let frames = src.len() / self.src_ch;
        if frames == 0 {
            return;
        }
        if self.src_rate == self.dst_rate && self.src_ch == self.dst_ch {
            out.extend_from_slice(src);
            return;
        }

        let step = f64::from(self.src_rate) / f64::from(self.dst_rate);
        let sample_at = |idx: isize, ch: usize| -> f32 {
            if idx < 0 {
                self.prev[ch.min(1)]
            } else {
                let f = (idx as usize).min(frames - 1);
                f32::from(src[f * self.src_ch + ch.min(self.src_ch - 1)])
            }
        };

        while self.pos < frames as f64 {
            let i0 = self.pos.floor() as isize - 1;
            let frac = (self.pos - self.pos.floor()) as f32;
            let l0 = sample_at(i0, 0);
            let l1 = sample_at(i0 + 1, 0);
            let l = (l0 + (l1 - l0) * frac).round().clamp(-32768.0, 32767.0) as i16;
            let r = if self.src_ch > 1 {
                let r0 = sample_at(i0, 1);
                let r1 = sample_at(i0 + 1, 1);
                (r0 + (r1 - r0) * frac).round().clamp(-32768.0, 32767.0) as i16
            } else {
                l
            };

            if self.dst_ch == 1 {
                let mono = ((i32::from(l) + i32::from(r)) / 2) as i16;
                out.push(mono);
            } else {
                out.push(l);
                out.push(r);
                for _ in 2..self.dst_ch {
                    out.push(0);
                }
            }
            self.pos += step;
        }
        self.pos -= frames as f64;
        let last = frames - 1;
        self.prev[0] = f32::from(src[last * self.src_ch]);
        self.prev[1] = if self.src_ch > 1 {
            f32::from(src[last * self.src_ch + 1])
        } else {
            self.prev[0]
        };
    }
}

pub struct AudioEngine {
    inner: Mutex<EngineInner>,
    last_error: Arc<Mutex<Option<String>>>,
}

struct EngineInner {
    format: Option<AudioFormat>,
    data: Option<Arc<Vec<u8>>>,
    on_end: Option<Arc<dyn Fn() + Send + Sync>>,
    session: Option<ActiveSession>,
    paused: bool,
    volume: f32,
    game_factor: f32,
    game_device: String,
    last_position_ms: u64,
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                format: None,
                data: None,
                on_end: None,
                session: None,
                paused: false,
                volume: 0.8,
                game_factor: 1.0,
                game_device: String::new(),
                last_position_ms: 0,
            }),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn play<F>(&self, fmt: AudioFormat, encoded: Vec<u8>, start_ms: u64, on_end: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let on_end_arc: Arc<dyn Fn() + Send + Sync> = Arc::new(on_end);
        let data_arc = Arc::new(encoded);
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        if let Some(mut s) = guard.session.take() {
            s.shutdown();
        }
        guard.format = Some(fmt);
        guard.data = Some(Arc::clone(&data_arc));
        guard.on_end = Some(Arc::clone(&on_end_arc));
        guard.paused = false;
        guard.last_position_ms = start_ms;
        self.start_locked(&mut guard, fmt, data_arc, start_ms, on_end_arc);
    }

    pub fn seek(&self, ms: u64) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let (Some(fmt), Some(data)) = (guard.format, guard.data.clone()) else {
            return;
        };
        let Some(on_end) = guard.on_end.clone() else {
            return;
        };
        if let Some(mut s) = guard.session.take() {
            s.shutdown();
        }
        guard.last_position_ms = ms;
        self.start_locked(&mut guard, fmt, data, ms, on_end);
        if guard.paused
            && let Some(s) = &mut guard.session
        {
            s.paused = true;
            s.shared.paused.store(true, Ordering::SeqCst);
        }
    }

    pub fn stop(&self) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        if let Some(mut s) = guard.session.take() {
            s.shutdown();
        }
        guard.format = None;
        guard.data = None;
        guard.on_end = None;
        guard.paused = false;
        guard.last_position_ms = 0;
        crate::viz::clear_spectrum();
    }

    pub fn pause(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.paused = true;
            if let Some(s) = &mut guard.session {
                s.paused = true;
                s.shared.paused.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn resume(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.paused = false;
            if let Some(s) = &mut guard.session {
                s.paused = false;
                s.shared.paused.store(false, Ordering::SeqCst);
            }
        }
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if let Ok(mut guard) = self.inner.lock() {
            guard.volume = v;
            if let Some(s) = &guard.session {
                s.shared.volume_bits.store(v.to_bits(), Ordering::Relaxed);
            }
        }
    }

    pub fn set_game_volume_factor(&self, factor: f32) {
        let f = factor.clamp(0.0, 1.0);
        if let Ok(mut guard) = self.inner.lock() {
            if (guard.game_factor - f).abs() < f32::EPSILON {
                return;
            }
            guard.game_factor = f;
            if let Some(s) = &guard.session {
                s.shared
                    .game_factor_bits
                    .store(f.to_bits(), Ordering::Relaxed);
            }
        }
    }

    pub fn set_game_device(&self, name: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.game_device = name.to_string();
        }
    }

    pub fn reopen_audio(&self) {
        let Ok(guard) = self.inner.lock() else {
            return;
        };
        let pos = guard
            .session
            .as_ref()
            .map_or(guard.last_position_ms, |s| s.shared.position_ms());
        let has_track = guard.data.is_some();
        drop(guard);
        if has_track {
            self.seek(pos);
        }
    }

    pub fn is_playing(&self) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        match &guard.session {
            Some(s) => {
                !guard.paused
                    && !s.shared.stop.load(Ordering::Relaxed)
                    && !s.shared.eof.load(Ordering::Relaxed)
            }
            None => false,
        }
    }

    pub fn position_ms(&self) -> u64 {
        let Ok(guard) = self.inner.lock() else {
            return 0;
        };
        guard
            .session
            .as_ref()
            .map_or(guard.last_position_ms, |s| s.shared.position_ms())
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }

    fn set_error(&self, msg: impl Into<String>) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = Some(msg.into());
        }
    }

    fn start_locked(
        &self,
        guard: &mut EngineInner,
        fmt: AudioFormat,
        data: Arc<Vec<u8>>,
        start_ms: u64,
        on_end: Arc<dyn Fn() + Send + Sync>,
    ) {
        if let Ok(mut e) = self.last_error.lock() {
            *e = None;
        }
        let wanted = PlayerConfig::load().audio_device;
        let devs = pick_output_device(&wanted, &guard.game_device);
        let mut opened: Option<(Device, StreamConfig, SampleFormat)> = None;
        for d in devs {
            if let Some((cfg, sf)) = pick_stream_config(&d) {
                opened = Some((d, cfg, sf));
                break;
            }
        }
        let Some((dev, cfg, sample_fmt)) = opened else {
            self.set_error("No audio output device available");
            return;
        };

        let dst_rate = cfg.sample_rate.0.max(8000);
        let dst_ch = usize::from(cfg.channels).max(1);
        let shared = Arc::new(SharedPlayback::new(
            start_ms,
            guard.volume,
            guard.game_factor,
            dst_rate,
            dst_ch,
        ));

        let shared_cb = Arc::clone(&shared);
        let err_slot = Arc::clone(&self.last_error);
        let err_cb = move |err: cpal::StreamError| {
            if let Ok(mut g) = err_slot.lock() {
                *g = Some(err.to_string());
            }
        };

        let stream_res = match sample_fmt {
            SampleFormat::F32 => dev.build_output_stream(
                &cfg,
                move |out: &mut [f32], _| {
                    fill_output_buffer(&shared_cb, out.len(), |i, s| {
                        out[i] = f32::from(s) / 32768.0;
                    });
                },
                err_cb,
                None,
            ),
            SampleFormat::I16 => dev.build_output_stream(
                &cfg,
                move |out: &mut [i16], _| {
                    fill_output_buffer(&shared_cb, out.len(), |i, s| {
                        out[i] = s;
                    });
                },
                err_cb,
                None,
            ),
            SampleFormat::U16 => dev.build_output_stream(
                &cfg,
                move |out: &mut [u16], _| {
                    fill_output_buffer(&shared_cb, out.len(), |i, s| {
                        out[i] = (i32::from(s) + 32768) as u16;
                    });
                },
                err_cb,
                None,
            ),
            other => {
                self.set_error(format!("Unsupported cpal sample format: {other:?}"));
                return;
            }
        };

        let stream = match stream_res {
            Ok(s) => s,
            Err(e) => {
                self.set_error(format!("Failed to build audio stream: {e}"));
                return;
            }
        };
        if let Err(e) = stream.play() {
            self.set_error(format!("Failed to start audio stream: {e}"));
            return;
        }

        let shared_feed = Arc::clone(&shared);
        let err_feed = Arc::clone(&self.last_error);
        let handle = thread::Builder::new()
            .name("overtone-audio-feeder".into())
            .spawn(move || {
                let mut src = match DecoderSource::open(fmt, (*data).clone(), start_ms) {
                    Ok(s) => s,
                    Err(e) => {
                        if let Ok(mut g) = err_feed.lock() {
                            *g = Some(e.to_string());
                        }
                        shared_feed.eof.store(true, Ordering::SeqCst);
                        return;
                    }
                };
                let src_rate = src.sample_rate();
                let src_ch = src.channels();
                let mut resampler = LinearResampler::new(src_rate, src_ch, dst_rate, dst_ch);
                let ring_cap = (dst_rate as usize * dst_ch * 15 / 100).max(dst_ch * 2048);
                let mut dec_buf = vec![0i16; 2048 * src_ch];
                let mut resampled = Vec::new();

                while !shared_feed.stop.load(Ordering::Relaxed) {
                    if shared_feed.paused.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    let ring_len = shared_feed.ring.lock().map_or(0, |r| r.len());
                    if ring_len >= ring_cap {
                        thread::sleep(Duration::from_millis(6));
                        continue;
                    }

                    match src.read_i16(&mut dec_buf) {
                        Ok(0) => {
                            shared_feed.eof.store(true, Ordering::SeqCst);
                            break;
                        }
                        Ok(n) => {
                            crate::viz::push_samples_i16(&dec_buf[..n], src_ch, src_rate);
                            resampler.process(&dec_buf[..n], &mut resampled);
                            if !resampled.is_empty()
                                && let Ok(mut r) = shared_feed.ring.lock()
                            {
                                r.extend(resampled.iter().copied());
                            }
                        }
                        Err(e) => {
                            if !shared_feed.stop.load(Ordering::Relaxed)
                                && let Ok(mut g) = err_feed.lock()
                            {
                                *g = Some(e.to_string());
                            }
                            shared_feed.eof.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }

                // Wait for remaining ring samples to play out before signaling on_end.
                while !shared_feed.stop.load(Ordering::Relaxed) {
                    let rem = shared_feed.ring.lock().map_or(0, |r| r.len());
                    if rem == 0 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(12));
                }
                if !shared_feed.stop.load(Ordering::SeqCst) {
                    on_end();
                }
            })
            .ok();

        guard.session = Some(ActiveSession {
            shared,
            stream: Some(stream),
            thread: handle,
            paused: false,
        });
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn fill_output_buffer<W>(shared: &SharedPlayback, out_len: usize, mut write: W)
where
    W: FnMut(usize, i16),
{
    if shared.stop.load(Ordering::Relaxed) || shared.paused.load(Ordering::Relaxed) {
        for i in 0..out_len {
            write(i, 0);
        }
        return;
    }

    let gain = shared.gain();
    let ch = shared.device_channels.max(1);
    let mut popped = 0usize;
    if let Ok(mut ring) = shared.ring.try_lock() {
        let take = ring.len().min(out_len);
        for i in 0..take {
            let s = ring.pop_front().unwrap_or(0);
            let scaled = (f32::from(s) * gain).round().clamp(-32768.0, 32767.0) as i16;
            write(i, scaled);
        }
        popped = take;
    }
    for i in popped..out_len {
        write(i, 0);
    }
    let frames = (popped / ch) as u64;
    if frames > 0 {
        shared
            .played_device_frames
            .fetch_add(frames, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_resampler_upsamples_mono_to_stereo() {
        let mut r = LinearResampler::new(22050, 1, 44100, 2);
        let src = [1000i16, 2000, 3000, 4000];
        let mut out = Vec::new();
        r.process(&src, &mut out);
        assert_eq!(out.len(), 16); // 4 * 2 * 2ch
        assert_eq!(out[0], out[1]);
    }

    #[test]
    fn decodes_wav_pcm_for_dsp() {
        // Minimal 16-bit mono 8000Hz WAV header + 4 samples
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&44u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8u32.to_le_bytes());
        for s in [100i16, -200, 300, -400] {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        assert_eq!(AudioFormat::detect(&wav), Some(AudioFormat::Pcm));
        let (rate, ch, pcm) = decode_full_pcm_for_dsp(AudioFormat::Pcm, &wav).expect("decode wav");
        assert_eq!(rate, 8000);
        assert_eq!(ch, 1);
        assert_eq!(pcm, vec![100, -200, 300, -400]);
    }
}
