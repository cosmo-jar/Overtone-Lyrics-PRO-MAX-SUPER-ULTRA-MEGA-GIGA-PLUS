use std::f32::consts::PI;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const FFT_SIZE: usize = 2048;
const RING_CAP: usize = 4096;
pub const BANDS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    Mix,
    Left,
    Right,
}

struct SpectrumRing {
    left: [f32; RING_CAP],
    right: [f32; RING_CAP],
    write: usize,
    filled: usize,
    sample_rate: u32,
    last_push: Option<Instant>,
    prev_energy: f32,
    smoothed_beat: f32,
}

impl SpectrumRing {
    fn new() -> Self {
        Self {
            left: [0.0; RING_CAP],
            right: [0.0; RING_CAP],
            write: 0,
            filled: 0,
            sample_rate: 44100,
            last_push: None,
            prev_energy: 0.0,
            smoothed_beat: 0.0,
        }
    }

    fn push_stereo(&mut self, samples: &[f32], channels: usize, sample_rate: u32) {
        if sample_rate > 0 {
            self.sample_rate = sample_rate;
        }
        let ch = channels.max(1);
        let mut energy_acc = 0.0f32;
        let mut frames = 0usize;
        for frame in samples.chunks_exact(ch) {
            let l = frame[0];
            let r = if ch > 1 { frame[1] } else { l };
            self.left[self.write] = l;
            self.right[self.write] = r;
            self.write = (self.write + 1) % RING_CAP;
            self.filled = (self.filled + 1).min(RING_CAP);
            energy_acc += l * l + r * r;
            frames += 1;
        }
        if frames > 0 {
            let rms = (energy_acc / (frames as f32 * 2.0)).sqrt();
            let delta = (rms - self.prev_energy).max(0.0);
            self.prev_energy = self.prev_energy * 0.85 + rms * 0.15;
            self.smoothed_beat = (self.smoothed_beat * 0.72 + delta * 3.8).clamp(0.0, 1.0);
            self.last_push = Some(Instant::now());
        }
    }

    fn is_live(&self) -> bool {
        self.filled >= FFT_SIZE
            && self
                .last_push
                .is_some_and(|t| t.elapsed().as_millis() < 350)
    }

    fn compute_bands(&self, out: &mut [f32], lo_hz: f32, hi_hz: f32, mode: ChannelMode) -> bool {
        if !self.is_live() || out.is_empty() {
            out.fill(0.0);
            return false;
        }
        let mut real = [0.0f32; FFT_SIZE];
        let mut imag = [0.0f32; FFT_SIZE];
        let start = (self.write + RING_CAP - FFT_SIZE) % RING_CAP;
        for (i, slot) in real.iter_mut().enumerate() {
            let idx = (start + i) % RING_CAP;
            let s = match mode {
                ChannelMode::Left => self.left[idx],
                ChannelMode::Right => self.right[idx],
                ChannelMode::Mix => (self.left[idx] + self.right[idx]) * 0.5,
            };
            let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos());
            *slot = s * w;
        }
        fft_radix2(&mut real, &mut imag);

        let sr = self.sample_rate.max(8000) as f32;
        let hz_per_bin = sr / FFT_SIZE as f32;
        let max_bin = FFT_SIZE / 2 - 1;
        let lo = lo_hz.max(20.0);
        let hi = hi_hz.min(sr * 0.48).max(lo + 10.0);
        let ratio = hi / lo;
        let n = out.len();

        for (b, slot) in out.iter_mut().enumerate() {
            let f0 = lo * ratio.powf(b as f32 / n as f32);
            let f1 = lo * ratio.powf((b + 1) as f32 / n as f32);
            let k0 = ((f0 / hz_per_bin).floor() as usize).clamp(1, max_bin);
            let k1 = ((f1 / hz_per_bin).ceil() as usize).clamp(k0, max_bin);
            let mut mag_sum = 0.0f32;
            let mut cnt = 0usize;
            for k in k0..=k1 {
                let m = (real[k] * real[k] + imag[k] * imag[k]).sqrt() / (FFT_SIZE as f32 * 0.25);
                mag_sum += m;
                cnt += 1;
            }
            let avg = mag_sum / cnt.max(1) as f32;
            *slot = (avg.sqrt() * 1.45).clamp(0.0, 2.5);
        }
        true
    }
}

fn spectrum_ring() -> &'static Mutex<SpectrumRing> {
    static RING: OnceLock<Mutex<SpectrumRing>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(SpectrumRing::new()))
}

pub fn push_samples_i16(samples: &[i16], channels: usize, sample_rate: u32) {
    let mut fbuf = Vec::with_capacity(samples.len());
    for &s in samples {
        fbuf.push(f32::from(s) / 32768.0);
    }
    push_samples_f32(&fbuf, channels, sample_rate);
}

pub fn push_samples_f32(samples: &[f32], channels: usize, sample_rate: u32) {
    if let Ok(mut guard) = spectrum_ring().lock() {
        guard.push_stereo(samples, channels, sample_rate);
    }
}

pub fn clear_spectrum() {
    if let Ok(mut guard) = spectrum_ring().lock() {
        guard.filled = 0;
        guard.smoothed_beat = 0.0;
        guard.last_push = None;
    }
}

pub fn bands(out: &mut [f32], lo_hz: f32, hi_hz: f32, mode: ChannelMode) -> bool {
    spectrum_ring()
        .lock()
        .map(|g| g.compute_bands(out, lo_hz, hi_hz, mode))
        .unwrap_or(false)
}

pub fn fft_radix2(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    debug_assert_eq!(n, imag.len());
    debug_assert!(n.is_power_of_two());

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let half = len / 2;
        let ang = -2.0 * PI / len as f32;
        let (w_sin, w_cos) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let mut wr = 1.0f32;
            let mut wi = 0.0f32;
            for k in 0..half {
                let even = start + k;
                let odd = even + half;
                let tr = wr * real[odd] - wi * imag[odd];
                let ti = wr * imag[odd] + wi * real[odd];
                real[odd] = real[even] - tr;
                imag[odd] = imag[even] - ti;
                real[even] += tr;
                imag[even] += ti;
                let next_wr = wr * w_cos - wi * w_sin;
                wi = wr * w_sin + wi * w_cos;
                wr = next_wr;
            }
        }
        len <<= 1;
    }
}

pub struct VisualizerState {
    band_l: Vec<f32>,
    band_r: Vec<f32>,
    raw: Vec<f32>,
    pub levels: Vec<f32>,
    velocity: Vec<f32>,
    pub peaks: Vec<f32>,
    hold: Vec<f32>,
    gain: f32,
    last_time: Option<Instant>,
    start_time: Instant,
    pub live: bool,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self::new()
    }
}

impl VisualizerState {
    pub fn new() -> Self {
        Self {
            band_l: Vec::new(),
            band_r: Vec::new(),
            raw: Vec::new(),
            levels: Vec::new(),
            velocity: Vec::new(),
            peaks: Vec::new(),
            hold: Vec::new(),
            gain: 1.0,
            last_time: None,
            start_time: Instant::now(),
            live: false,
        }
    }

    pub fn bands(&self) -> &[f32] {
        &self.levels
    }

    pub fn update(
        &mut self,
        count: usize,
        sensitivity: i32,
        smoothing: i32,
        auto_sens: bool,
        demo: bool,
        stereo: bool,
    ) {
        if count == 0 {
            return;
        }
        let half = count.div_ceil(2);
        if self.levels.len() != count {
            self.raw.resize(count, 0.0);
            self.levels.resize(count, 0.0);
            self.velocity.resize(count, 0.0);
            self.peaks.resize(count, 0.0);
            self.hold.resize(count, 0.0);
        }
        let needed = if stereo { half } else { count };
        if self.band_l.len() != needed {
            self.band_l.resize(needed, 0.0);
            self.band_r.resize(needed, 0.0);
        }

        let now = Instant::now();
        let dt = self.last_time.map_or(0.016, |t| {
            now.saturating_duration_since(t).as_secs_f32().min(0.1)
        });
        self.last_time = Some(now);

        self.live = if stereo {
            bands(&mut self.band_l, 50.0, 10000.0, ChannelMode::Left)
                & bands(&mut self.band_r, 50.0, 10000.0, ChannelMode::Right)
        } else {
            bands(&mut self.band_l, 50.0, 10000.0, ChannelMode::Mix)
        };

        if !self.live {
            let t = self.start_time.elapsed().as_secs_f64();
            let b_len = self.band_l.len() as f64;
            for i in 0..self.band_l.len() {
                let v = if demo {
                    (0.3 + 0.3
                        * (t * 4.2 - i as f64 * 0.35).sin().max(0.0)
                        * (1.0 - i as f64 / b_len)
                        + 0.1 * (t * 2.3 + i as f64).sin()) as f32
                } else {
                    0.0
                };
                self.band_l[i] = v;
                self.band_r[i] = v;
            }
        }

        if stereo {
            for i in 0..half {
                let li = half - 1 - i;
                let ri = count - half + i;
                self.raw[li] = self.band_l[i];
                self.raw[ri] = if li == ri {
                    (self.band_l[i] + self.band_r[i]) * 0.5
                } else {
                    self.band_r[i]
                };
            }
        } else {
            self.raw[..count].copy_from_slice(&self.band_l[..count]);
        }

        let mut max_v = 0.0f32;
        for i in 0..count {
            let mult = if self.live {
                self.gain * (sensitivity as f32) / 100.0
            } else {
                1.0
            };
            let v = self.raw[i] * mult;
            max_v = max_v.max(v);
            self.raw[i] = v.clamp(0.0, 1.0);
        }

        if auto_sens && self.live {
            if max_v > 1.0 {
                self.gain *= 0.98f32.powf(dt * 60.0);
            } else if max_v > 0.01 {
                self.gain *= 1.002f32.powf(dt * 60.0);
            }
            self.gain = self.gain.clamp(0.02, 60.0);
        } else if !auto_sens {
            self.gain = 1.0;
        }

        let s = (smoothing.min(95) as f32) / 100.0;
        let attack = 1.0 - (-dt * (60.0 - 35.0 * s)).exp();
        let gravity = 14.0 - 12.0 * s;

        for i in 0..count {
            let v = self.raw[i];
            if v >= self.levels[i] {
                self.levels[i] += (v - self.levels[i]) * attack;
                self.velocity[i] = 0.0;
            } else {
                self.velocity[i] += gravity * dt;
                self.levels[i] = (self.levels[i] - self.velocity[i] * dt).max(v);
            }
            if self.levels[i] >= self.peaks[i] {
                self.peaks[i] = self.levels[i];
                self.hold[i] = 0.35;
            } else {
                self.hold[i] -= dt;
                if self.hold[i] <= 0.0 {
                    self.peaks[i] = (self.peaks[i] - dt * 0.7).max(self.levels[i]);
                }
            }
        }
    }
}

/// System loopback audio capture (Spotify / "Whole system" mode on Windows WASAPI via CPAL loopback).
pub struct SystemCapture;

impl SystemCapture {
    pub fn set_running(enabled: bool) {
        static ACTIVE: OnceLock<Mutex<Option<cpal::Stream>>> = OnceLock::new();
        let lock = ACTIVE.get_or_init(|| Mutex::new(None));
        let Ok(mut guard) = lock.lock() else { return };
        if !enabled {
            *guard = None;
            return;
        }
        if guard.is_some() {
            return;
        }
        // Attempt WASAPI loopback via cpal default output device input stream (supported on Windows).
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        if let Some(dev) = host.default_output_device()
            && let Ok(cfg) = dev.default_output_config()
        {
            let rate = cfg.sample_rate();
            let chans = cfg.channels() as usize;
            let stream_cfg: cpal::StreamConfig = cfg.into();
            if let Ok(stream) = dev.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _| push_samples_f32(data, chans, rate.0),
                |_| {},
                None,
            ) && stream.play().is_ok()
            {
                *guard = Some(stream);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_detects_sine_peak_and_visualizer_advances() {
        let n = 1024;
        let mut real = vec![0.0f32; n];
        let mut imag = vec![0.0f32; n];
        for (i, r) in real.iter_mut().enumerate() {
            *r = (2.0 * PI * 32.0 * i as f32 / n as f32).sin();
        }
        fft_radix2(&mut real, &mut imag);
        let mag32 = (real[32] * real[32] + imag[32] * imag[32]).sqrt();
        let mag10 = (real[10] * real[10] + imag[10] * imag[10]).sqrt();
        assert!(mag32 > mag10 * 10.0);

        let mut viz = VisualizerState::new();
        viz.update(24, 100, 65, true, true, true);
        assert_eq!(viz.levels.len(), 24);
        assert!(viz.levels.iter().any(|&v| v > 0.01));
    }
}
