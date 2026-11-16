use std::f32::consts::PI;

use crate::model::{LyricLine, WordTiming};

#[derive(Clone, Debug)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * (cutoff_hz / sample_rate).clamp(0.001, 0.49);
        let cos_w = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w) * 0.5) / a0,
            b1: (1.0 - cos_w) / a0,
            b2: ((1.0 - cos_w) * 0.5) / a0,
            a1: (-2.0 * cos_w) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn highpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * (cutoff_hz / sample_rate).clamp(0.001, 0.49);
        let cos_w = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w) * 0.5) / a0,
            b1: -(1.0 + cos_w) / a0,
            b2: ((1.0 + cos_w) * 0.5) / a0,
            a1: (-2.0 * cos_w) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

fn goertzel_power(samples: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
    if samples.is_empty() || freq_hz >= sample_rate * 0.49 {
        return 0.0;
    }
    let k = (0.5 + (samples.len() as f32 * freq_hz) / sample_rate).floor();
    let omega = (2.0 * PI * k) / samples.len() as f32;
    let coeff = 2.0 * omega.cos();
    let mut s_prev = 0.0f32;
    let mut s_prev2 = 0.0f32;
    for &x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    (s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2).max(0.0)
        / (samples.len() as f32 * samples.len() as f32).max(1.0)
}

/// Counts approximate vocal syllables in a line (handles Cyrillic + Latin + CJK).
pub fn estimate_syllables(line: &str) -> f32 {
    let mut vowels = 0usize;
    let mut in_vowel_run = false;
    let mut cjk = 0usize;
    let mut word_chars = 0usize;
    for ch in line.chars() {
        let is_v = matches!(ch.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
            || matches!(
                ch,
                'а' | 'е'
                    | 'ё'
                    | 'и'
                    | 'о'
                    | 'у'
                    | 'ы'
                    | 'э'
                    | 'ю'
                    | 'я'
                    | 'А'
                    | 'Е'
                    | 'Ё'
                    | 'И'
                    | 'О'
                    | 'У'
                    | 'Ы'
                    | 'Э'
                    | 'Ю'
                    | 'Я'
            );
        if is_v {
            if !in_vowel_run {
                vowels += 1;
            }
            in_vowel_run = true;
            word_chars += 1;
        } else {
            in_vowel_run = false;
            if ('\u{3040}'..='\u{9FFF}').contains(&ch) || ('\u{AC00}'..='\u{D7AF}').contains(&ch) {
                cjk += 1;
            } else if ch.is_alphanumeric() {
                word_chars += 1;
            }
        }
    }
    let syl = vowels + cjk;
    if syl > 0 {
        syl as f32
    } else if word_chars > 0 {
        (word_chars as f32 / 3.0).max(1.0)
    } else {
        1.0
    }
}

/// Computes short-time vocal-activity curve at fixed step (default ~100ms per step for DTW).
pub fn extract_vocal_activity(
    pcm: &[i16],
    channels: usize,
    sample_rate: u32,
    frame_ms: u32,
) -> Vec<f32> {
    let ch = channels.max(1);
    let sr = sample_rate.max(8000);
    let step = ((sr as usize) * (frame_ms as usize)) / 1000;
    if step == 0 || pcm.len() < step * ch {
        return Vec::new();
    }

    let total_frames = pcm.len() / ch;
    let mut center_vocal = vec![0.0f32; total_frames];
    let mut side_vocal = if ch >= 2 {
        vec![0.0f32; total_frames]
    } else {
        Vec::new()
    };
    let mut sub_bass = vec![0.0f32; total_frames];
    let mut hp = Biquad::highpass(sr as f32, 300.0, 0.707);
    let mut lp = Biquad::lowpass(sr as f32, 3400.0, 0.707);
    let mut side_hp = Biquad::highpass(sr as f32, 300.0, 0.707);
    let mut side_lp = Biquad::lowpass(sr as f32, 3400.0, 0.707);
    let mut sub_lp = Biquad::lowpass(sr as f32, 220.0, 0.707);

    // Separate center vocal band (300-3400 Hz), stereo side band, and sub-bass (<220 Hz)
    for (idx, frame) in pcm.chunks_exact(ch).enumerate() {
        let l = f32::from(frame[0]) / 32768.0;
        let (mid, side) = if ch >= 2 {
            let r = f32::from(frame[1]) / 32768.0;
            ((l + r) * 0.5, (l - r) * 0.5)
        } else {
            (l, 0.0)
        };
        let mono = if ch >= 2 {
            let side_att = (side.abs() * 0.45).min(mid.abs() * 0.5);
            if mid >= 0.0 {
                mid - side_att
            } else {
                mid + side_att
            }
        } else {
            mid
        };
        center_vocal[idx] = lp.process(hp.process(mono));
        if ch >= 2 {
            side_vocal[idx] = side_lp.process(side_hp.process(side));
        }
        sub_bass[idx] = sub_lp.process(mid);
    }

    let formant_freqs = [420.0f32, 780.0, 1350.0, 2200.0, 3100.0];
    let mut prev_bands = [0.0f32; 5];
    let mut scores = Vec::with_capacity(center_vocal.len() / step + 1);

    for (chunk_idx, chunk) in center_vocal.chunks(step).enumerate() {
        if chunk.len() < step / 2 {
            break;
        }
        let s0 = chunk_idx * step;
        let s1 = (s0 + chunk.len()).min(total_frames);
        let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt();
        let side_rms = if ch >= 2 {
            (side_vocal[s0..s1].iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt()
        } else {
            0.0
        };
        let sub_rms =
            (sub_bass[s0..s1].iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt();

        // Downweight wide-stereo synths/guitars and kick/sub-heavy instrumental intros
        let center_focus = (rms / (rms + side_rms * 1.15 + 1e-4)).clamp(0.25, 1.0);
        let vocal_over_bass = (rms / (rms + sub_rms * 0.65 + 1e-4)).clamp(0.22, 1.0);

        let mut flux = 0.0f32;
        let mut formant_sum = 0.0f32;
        for (i, &f) in formant_freqs.iter().enumerate() {
            let p = goertzel_power(chunk, f, sr as f32).sqrt();
            flux += (p - prev_bands[i]).max(0.0);
            prev_bands[i] = p;
            formant_sum += p;
        }
        // Combined vocal score: sustained center vocal RMS + formant clarity + onset flux.
        // Compress dynamic range mildly so quieter verses still stand out before loud chorus drops.
        let score =
            ((rms * 0.55 + formant_sum * 1.4 + flux * 0.85) * center_focus * vocal_over_bass)
                .max(0.0)
                .powf(0.55);
        scores.push(score);
    }

    if scores.is_empty() {
        return scores;
    }
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p10 = sorted[(sorted.len() * 10) / 100];
    let p92 = sorted[((sorted.len() * 92) / 100).min(sorted.len() - 1)].max(p10 + 1e-4);

    for v in &mut scores {
        *v = ((*v - p10) / (p92 - p10)).clamp(0.0, 1.0);
    }
    // Gentle 3-tap temporal smoothing
    let orig = scores.clone();
    for i in 1..scores.len().saturating_sub(1) {
        scores[i] = orig[i - 1] * 0.25 + orig[i] * 0.5 + orig[i + 1] * 0.25;
    }
    scores
}

/// Finds the sustained active vocal `[span_start, span_end]` (inclusive) on a 100ms activity curve,
/// trimming silent/instrumental intros as well as fading outros.
fn detect_vocal_span(activity: &[f32]) -> (usize, usize) {
    let mut act_sorted = activity.to_vec();
    act_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p75 = act_sorted[((act_sorted.len() * 75) / 100).min(act_sorted.len() - 1)];
    let onset_thresh = (p75 * 0.40).clamp(0.22, 0.42);

    let win = if activity.len() >= 120 { 10 } else { 6 }.min(activity.len());
    let req_hits = (win * 6) / 10;
    let max_intro = ((activity.len() * 55) / 100).min(240);
    let first_active = (0..=max_intro.saturating_sub(win))
        .find(|&start| {
            let w = &activity[start..start + win];
            let mean = w.iter().sum::<f32>() / win as f32;
            let hits = w.iter().filter(|&&v| v >= onset_thresh * 0.80).count();
            mean >= onset_thresh && hits >= req_hits
        })
        .map(|start| {
            let lo = start.saturating_sub(3);
            let hi = (start + win).min(activity.len());
            (lo..hi)
                .find(|&k| activity[k] >= onset_thresh * 0.72)
                .unwrap_or(start)
        })
        .or_else(|| activity.iter().position(|&v| v > 0.22))
        .unwrap_or(0)
        .saturating_sub(1);

    let tail_thresh = (onset_thresh * 0.55).max(0.18);
    let last_active = (first_active + 4..activity.len())
        .rev()
        .find(|&e| {
            let w_start = e.saturating_sub(4);
            let w = &activity[w_start..=e];
            let mean = w.iter().sum::<f32>() / w.len() as f32;
            mean >= tail_thresh && activity[e] >= 0.16
        })
        .or_else(|| activity.iter().rposition(|&v| v > 0.18))
        .unwrap_or(activity.len() - 1);

    let span_start = first_active.min(activity.len().saturating_sub(2));
    let span_end = last_active.max(span_start + 1);
    (span_start, span_end)
}

/// Scores how strongly each frame `j` marks a phrase onset (vocal rise after a breath/dip or interlude).
fn compute_boundary_scores(vslice: &[f32]) -> Vec<f32> {
    let n = vslice.len();
    let mut sorted = vslice.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let local_med = sorted[sorted.len() / 2];

    let mut scores = vec![0.0f32; n + 1];
    for j in 1..n {
        let pre_lo = j.saturating_sub(4);
        let pre_min = vslice[pre_lo..j]
            .iter()
            .copied()
            .reduce(f32::min)
            .unwrap_or(0.0);

        let post_hi = (j + 5).min(n);
        let post_peak_hi = (j + 3).min(n);
        let post_peak = vslice[j..post_peak_hi]
            .iter()
            .copied()
            .reduce(f32::max)
            .unwrap_or(0.0);
        let post_mean = vslice[j..post_hi].iter().sum::<f32>() / (post_hi - j).max(1) as f32;

        let rise = (post_peak - pre_min).max(0.0);
        let dip = (local_med * 0.90 - pre_min).max(0.0);

        // Reward phrase re-entries immediately after multi-frame instrumental/breath pauses.
        let lookback = j.saturating_sub(15);
        let consec_low = vslice[lookback..j]
            .iter()
            .rev()
            .take_while(|&&v| v < 0.28)
            .count();
        let pause_bonus = (consec_low.saturating_sub(2) as f32 * 0.45).min(2.5);

        let steady_cut =
            if vslice[j - 1] > 0.45 && vslice[j] > 0.45 && (vslice[j] - vslice[j - 1]).abs() < 0.06
            {
                0.35
            } else {
                0.0
            };

        scores[j] = rise + dip * 0.75 + pause_bonus + post_mean * 0.25 - steady_cut;
    }
    scores
}

/// Aligns lines onto `[0..vslice.len()]` via constrained line-level Viterbi DP over syllable tempo,
/// vocal-body coverage, and phrase-onset boundaries. Returns `starts[0..=L]`.
fn align_lines_viterbi(
    weights: &[f32],
    stanza_after: &[bool],
    vslice: &[f32],
) -> Option<Vec<usize>> {
    let l = weights.len();
    let n = vslice.len();
    if l == 0 || n < l.max(2) {
        return None;
    }

    let total_w: f32 = weights.iter().sum::<f32>().max(1.0);
    let mut cum_w = Vec::with_capacity(l + 1);
    cum_w.push(0.0f32);
    let mut acc = 0.0f32;
    for &w in weights {
        acc += w;
        cum_w.push(acc);
    }

    let mut sorted = vslice.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let dip_thresh = (sorted[(sorted.len() * 20) / 100] * 1.15).max(0.18);

    let mut pref_dip = vec![0.0f64; n + 1];
    let mut pref_act = vec![0.0f64; n + 1];
    for idx in 0..n {
        pref_dip[idx + 1] = pref_dip[idx] + if vslice[idx] < dip_thresh { 1.0 } else { 0.0 };
        pref_act[idx + 1] = pref_act[idx] + f64::from(vslice[idx]);
    }

    let bscore = compute_boundary_scores(vslice);
    let inf = 1e12f64;
    let mut dp = vec![vec![inf; n + 1]; l + 1];
    let mut back = vec![vec![-1i32; n + 1]; l + 1];

    // First line begins near the detected vocal onset so intros are never re-entered or skipped twice.
    let start_hi = (n - l).min(10);
    for j in 0..=start_hi {
        dp[0][j] = (j as f64 / n.max(1) as f64) * 8.0 - f64::from(bscore[j].min(2.5)) * 0.8;
    }

    let min_step = (n / l.max(1)).clamp(1, 5);
    let rad = ((n as f32) * 0.16).round().clamp(25.0, 160.0) as usize;

    for i in 1..=l {
        let exp_s = (cum_w[i] / total_w) * n as f32;
        let exp_prev = (cum_w[i - 1] / total_w) * n as f32;
        let exp_dur = ((weights[i - 1] / total_w) * n as f32).max(1.0);
        let gap_mult = if stanza_after[i - 1] { 2.2 } else { 1.55 };
        let min_d = ((exp_dur * 0.65).round() as usize).max(min_step.max(3));
        let max_d = ((exp_dur * gap_mult).round() as usize).max(min_d + 4);

        let (j_lo, j_hi) = if i == l {
            (n, n)
        } else {
            let floor_j = i * min_step;
            let ceil_j = n.saturating_sub((l - i) * min_step);
            let s_int = exp_s.round() as usize;
            (
                s_int.saturating_sub(rad).max(floor_j).min(ceil_j),
                (s_int + rad).min(ceil_j),
            )
        };

        let (p_lo_base, p_hi_base) = if i == 1 {
            (0usize, start_hi)
        } else {
            let s_prev = exp_prev.round() as usize;
            (
                s_prev.saturating_sub(rad).max((i - 1) * min_step),
                (s_prev + rad).min(n - 1),
            )
        };

        for j in j_lo..=j_hi {
            let p_lo = p_lo_base.max(j.saturating_sub(max_d));
            let p_hi = p_hi_base.min(j.saturating_sub(min_d));
            if p_lo > p_hi {
                continue;
            }
            let anchor_c = if i < l {
                f64::from((j as f32 - exp_s).abs() / n.max(1) as f32) * 2.8
            } else {
                0.0
            };
            let bnd_c = if i < l {
                -f64::from(bscore[j].min(3.5)) * 0.85
            } else {
                0.0
            };
            let mut best_c = inf;
            let mut best_p = -1i32;
            for p in p_lo..=p_hi {
                let base_c = dp[i - 1][p];
                if base_c >= inf * 0.5 {
                    continue;
                }
                let d = (j - p) as f64;
                let log_dev = (d / f64::from(exp_dur)).ln().abs();
                let tempo_c = log_dev * log_dev * 7.5;

                // Ensure the line's singing core carries active vocal energy (gaps belong in trailing tails).
                let body_hi = (p + ((d * 0.70).round() as usize).max(3)).min(j);
                let body_len = (body_hi - p).max(1) as f64;
                let body_act = (pref_act[body_hi] - pref_act[p]) / body_len;
                let silent_c = (0.32 - body_act).max(0.0) * 24.0;

                // Discourage straddling an internal interlude gap across the middle of a single line.
                let core_lo = (p + ((d * 0.20).round() as usize).max(2)).min(j);
                let core_hi = j
                    .saturating_sub(((d * 0.35).round() as usize).max(2))
                    .max(core_lo + 1)
                    .min(j);
                let core_dips = pref_dip[core_hi] - pref_dip[core_lo];
                let split_c = (core_dips - 2.0).max(0.0) * 1.2;

                let cand = base_c + tempo_c + split_c + silent_c + anchor_c + bnd_c;
                if cand < best_c {
                    best_c = cand;
                    best_p = p as i32;
                }
            }
            dp[i][j] = best_c;
            back[i][j] = best_p;
        }
    }

    if back[l][n] < 0 {
        return None;
    }

    let mut starts = vec![0usize; l + 1];
    let mut cur = n;
    for i in (0..=l).rev() {
        starts[i] = cur;
        if i > 0 {
            let p = back[i][cur];
            if p < 0 {
                return None;
            }
            cur = p as usize;
        }
    }
    Some(starts)
}

/// Generates per-word karaoke spans inside `[s_ms, next_ms]` anchored to syllable pace and 50ms vocal activity,
/// stopping at the vocal phrase end before any trailing instrumental pause.
fn build_word_timings(
    text: &str,
    s_ms: u64,
    next_ms: u64,
    exp_dur_ms: u64,
    act_50: &[f32],
    act_step_ms: u64,
) -> Vec<WordTiming> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    if toks.is_empty() || next_ms <= s_ms {
        return Vec::new();
    }

    let step = act_step_ms.max(1);
    let idx0 = (s_ms / step) as usize;
    let idx1 = ((next_ms / step) as usize).min(act_50.len());
    let full_dur_ms = next_ms - s_ms;

    let min_keep_ms = full_dur_ms.min(((exp_dur_ms as f32 * 0.72) as u64).max(650));
    let mut v_end_ms = next_ms;
    if idx1 > idx0 + 2 {
        let mut e = idx1;
        while e > idx0 && ((e - idx0) as u64 * step) > min_keep_ms && act_50[e - 1] < 0.24 {
            e -= 1;
        }
        v_end_ms = s_ms
            .saturating_add(((e - idx0) as u64) * step)
            .clamp(s_ms.saturating_add(min_keep_ms), next_ms);
    }

    let min_word_ms = ((v_end_ms - s_ms) / (toks.len() as u64 * 2)).clamp(50, 90);
    let span_ms = (v_end_ms - s_ms).max(toks.len() as u64 * min_word_ms);
    let v_end_clamped = (s_ms + span_ms).min(next_ms);
    let eff_span = v_end_clamped.saturating_sub(s_ms).max(toks.len() as u64);

    let mut w_weights = Vec::with_capacity(toks.len());
    for tok in &toks {
        let punct = if tok
            .chars()
            .last()
            .is_some_and(|c| matches!(c, ',' | ';' | ':' | '.' | '!' | '?' | '—' | '-'))
        {
            0.65
        } else {
            0.0
        };
        w_weights.push(estimate_syllables(tok) + 0.45 + punct);
    }
    let total_ww: f32 = w_weights.iter().sum::<f32>().max(1.0);

    // Cumulative blended time + vocal energy curve inside [s_ms..v_end_clamped].
    let e_end = ((v_end_clamped / step) as usize)
        .max(idx0 + 1)
        .min(act_50.len());
    let mut cum_e = Vec::new();
    if idx0 < e_end && !act_50.is_empty() {
        cum_e.push(0.0f32);
        let mut s = 0.0f32;
        for &v in &act_50[idx0..e_end] {
            s += v.max(0.12);
            cum_e.push(s);
        }
        if s > 0.0 {
            for v in &mut cum_e {
                *v /= s;
            }
        }
    }
    if cum_e.len() < 2 {
        cum_e = vec![0.0, 1.0];
    }
    let m = (cum_e.len() - 1) as f32;
    let map_frac = |u: f32| -> f32 {
        let mut prev_b = 0.0f32;
        for (idx, &ce) in cum_e.iter().enumerate().skip(1) {
            let t1 = idx as f32 / m;
            let b1 = 0.68 * t1 + 0.32 * ce;
            if u <= b1 || idx + 1 == cum_e.len() {
                let t0 = (idx - 1) as f32 / m;
                let alpha = ((u - prev_b) / (b1 - prev_b).max(1e-6)).clamp(0.0, 1.0);
                return t0 + alpha * (t1 - t0);
            }
            prev_b = b1;
        }
        1.0
    };

    let mut out = Vec::with_capacity(toks.len());
    let mut acc = 0.0f32;
    let mut cur_ms = s_ms;
    let step_floor = eff_span / (toks.len() as u64).max(1);
    let w_floor = min_word_ms.min(step_floor.max(1));
    for (k, tok) in toks.iter().enumerate() {
        let st = cur_ms;
        acc += w_weights[k];
        let rem_min = (toks.len() - k - 1) as u64 * w_floor;
        let max_end = v_end_clamped.saturating_sub(rem_min);
        let en = if k + 1 == toks.len() {
            v_end_clamped.max(st + 1)
        } else {
            let frac = map_frac((acc / total_ww).clamp(0.0, 1.0));
            let target = s_ms + (frac * eff_span as f32).round() as u64;
            target.clamp(st + w_floor, max_end.max(st + w_floor))
        };
        out.push(WordTiming {
            time_ms: st,
            end_ms: Some(en),
            text: (*tok).to_owned(),
        });
        cur_ms = en;
    }
    out
}

/// Aligns plain text lines (empty strings denote stanza/instrumental breaks) to decoded PCM audio.
/// Uses line-level Viterbi DP over syllable pacing, vocal activity, and phrase-onset boundaries,
/// plus per-word vocal-envelope timings.
pub fn align_plain_lyrics(
    plain_lines: &[String],
    pcm: &[i16],
    channels: usize,
    sample_rate: u32,
    track_duration_ms: u64,
) -> Vec<LyricLine> {
    let mut clean_lines: Vec<String> = Vec::new();
    let mut stanza_after: Vec<bool> = Vec::new();
    for raw in plain_lines {
        let t = raw.trim();
        if t.is_empty() {
            if let Some(last) = stanza_after.last_mut() {
                *last = true;
            }
            continue;
        }
        clean_lines.push(t.to_owned());
        stanza_after.push(false);
    }
    if clean_lines.is_empty() {
        return Vec::new();
    }

    let step_ms = 100u64;
    let activity = extract_vocal_activity(pcm, channels, sample_rate, step_ms as u32);
    if activity.len() < 4 {
        return fallback_linear(&clean_lines, track_duration_ms.max(15_000));
    }

    let (span_start, span_end) = detect_vocal_span(&activity);
    let vslice = &activity[span_start..=span_end];
    let n = vslice.len();

    let weights: Vec<f32> = clean_lines
        .iter()
        .enumerate()
        .map(|(i, l)| estimate_syllables(l) + 1.8 + if stanza_after[i] { 2.2 } else { 0.0 })
        .collect();
    let total_w: f32 = weights.iter().sum::<f32>().max(1.0);

    let max_ms = ((pcm.len() / channels.max(1)) as u64 * 1000 / u64::from(sr_from(sample_rate)))
        .max(track_duration_ms);
    let Some(starts) = align_lines_viterbi(&weights, &stanza_after, vslice) else {
        return fallback_linear(&clean_lines, max_ms.max(15_000));
    };

    let word_step_ms = 50u64;
    let act_50 = extract_vocal_activity(pcm, channels, sample_rate, word_step_ms as u32);
    let span_ms = (n as u64) * step_ms;

    let mut result = Vec::with_capacity(clean_lines.len());
    let mut prev_ms = 0u64;
    let line_ms_starts: Vec<u64> = starts
        .iter()
        .enumerate()
        .map(|(idx, &rel)| {
            let mut ms = ((span_start + rel) as u64) * step_ms;
            ms = ms.min(max_ms.saturating_sub(if idx < clean_lines.len() { 200 } else { 0 }));
            if idx > 0 && idx < clean_lines.len() && ms <= prev_ms {
                ms = (prev_ms + 250).min(max_ms.saturating_sub(200));
            }
            prev_ms = ms;
            ms
        })
        .collect();

    for (idx, text) in clean_lines.into_iter().enumerate() {
        let s_ms = line_ms_starts[idx];
        let next_ms = line_ms_starts
            .get(idx + 1)
            .copied()
            .unwrap_or(max_ms)
            .max(s_ms + 250);
        let exp_dur_ms = ((weights[idx] / total_w) * span_ms as f32).round() as u64;
        let words = build_word_timings(&text, s_ms, next_ms, exp_dur_ms, &act_50, word_step_ms);
        result.push(LyricLine::with_words(s_ms, text, words));
    }
    result
}

fn sr_from(sr: u32) -> u32 {
    sr.max(8000)
}

fn fallback_linear(lines: &[String], duration_ms: u64) -> Vec<LyricLine> {
    let intro = (duration_ms as f32 * 0.06).min(8000.0) as u64;
    let usable = duration_ms.saturating_sub(intro + 2000).max(5000);
    let weights: Vec<f32> = lines.iter().map(|l| estimate_syllables(l)).collect();
    let total_w: f32 = weights.iter().sum::<f32>().max(1.0);
    let mut acc = 0.0f32;
    let starts: Vec<u64> = lines
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            let ms = intro + ((acc / total_w) * usable as f32) as u64;
            acc += weights[idx];
            ms
        })
        .collect();
    let end_ms = intro + usable;
    let mut out = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        let s_ms = starts[idx];
        let next_ms = starts.get(idx + 1).copied().unwrap_or(end_ms);
        let words = build_word_timings(line, s_ms, next_ms, next_ms.saturating_sub(s_ms), &[], 50);
        out.push(LyricLine::with_words(s_ms, line.as_str(), words));
    }
    out
}

/// One-click manual re-anchor ("Sync current line to now"): locks `lines[line_idx]` to `actual_ms`
/// and proportionally shifts the remaining tail while preserving monotonic progression.
pub fn reanchor_tail_from(lines: &mut [LyricLine], line_idx: usize, actual_ms: u64) {
    if line_idx >= lines.len() || lines[line_idx].is_unsynced() {
        return;
    }
    let old_anchor = lines[line_idx].time_ms;
    let delta = actual_ms as i64 - old_anchor as i64;
    lines[line_idx].time_ms = actual_ms;
    for w in &mut lines[line_idx].words {
        w.time_ms = (w.time_ms as i64 + delta).max(0) as u64;
        w.end_ms = w.end_ms.map(|e| (e as i64 + delta).max(0) as u64);
    }

    let mut prev_ms = actual_ms;
    for line in lines.iter_mut().skip(line_idx + 1) {
        if line.is_unsynced() {
            continue;
        }
        let shifted = (line.time_ms as i64 + delta).max(0) as u64;
        let final_ms = shifted.max(prev_ms + 150);
        let word_shift = final_ms as i64 - line.time_ms as i64;
        line.time_ms = final_ms;
        for w in &mut line.words {
            w.time_ms = (w.time_ms as i64 + word_shift).max(0) as u64;
            w.end_ms = w.end_ms.map(|e| (e as i64 + word_shift).max(0) as u64);
        }
        prev_ms = final_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_plain_lines_onto_synthetic_vocal_bursts() {
        let sr = 16000u32;
        let total_sec = 10usize;
        let mut pcm = vec![0i16; sr as usize * total_sec];
        // Three simulated vocal energy bursts around 1.5s..2.8s, 4.0s..5.5s, 7.0s..8.5s at 900 Hz
        let bursts = [(1500usize, 2800usize), (4000, 5500), (7000, 8500)];
        for (start_ms, end_ms) in bursts {
            let s0 = (start_ms * sr as usize) / 1000;
            let s1 = (end_ms * sr as usize) / 1000;
            for (i, slot) in pcm.iter_mut().enumerate().take(s1).skip(s0) {
                let sample = (2.0 * PI * 900.0 * i as f32 / sr as f32).sin() * 14000.0;
                *slot = sample as i16;
            }
        }
        let plain = vec![
            "First vocal line".to_owned(),
            String::new(),
            "Second vocal line here".to_owned(),
            String::new(),
            "Third vocal ending".to_owned(),
        ];
        let aligned = align_plain_lyrics(&plain, &pcm, 1, sr, 10_000);
        assert_eq!(aligned.len(), 3);
        assert!(aligned[0].time_ms < aligned[1].time_ms);
        assert!(aligned[1].time_ms < aligned[2].time_ms);
        assert!(
            aligned[0].time_ms >= 1000 && aligned[0].time_ms <= 2500,
            "got {}",
            aligned[0].time_ms
        );
        assert!(aligned[2].time_ms >= 5500, "got {}", aligned[2].time_ms);
    }

    #[test]
    fn reanchor_tail_shifts_following_lines_and_keeps_prefix() {
        let mut lines = vec![
            LyricLine::new(1000, "A"),
            LyricLine::new(5000, "B"),
            LyricLine::new(8000, "C"),
        ];
        reanchor_tail_from(&mut lines, 1, 6200);
        assert_eq!(lines[0].time_ms, 1000);
        assert_eq!(lines[1].time_ms, 6200);
        assert_eq!(lines[2].time_ms, 9200);
    }

    #[test]
    fn skips_stereo_bass_instrumental_intro_before_anchoring_first_line() {
        let sr = 16000u32;
        let total_sec = 26usize;
        let total_frames = sr as usize * total_sec;
        let mut pcm = vec![0i16; total_frames * 2];

        // 0..12s: stereo wide synth pad (650 Hz inverted in R) + heavy 90 Hz bass pulse
        for f in 0..(12 * sr as usize) {
            let t = f as f32 / sr as f32;
            let bass = (2.0 * PI * 90.0 * t).sin() * 9000.0;
            let pad = (2.0 * PI * 650.0 * t).sin() * 3500.0;
            pcm[f * 2] = (bass + pad).clamp(-32000.0, 32000.0) as i16;
            pcm[f * 2 + 1] = (bass - pad).clamp(-32000.0, 32000.0) as i16;
        }
        // 12..25s: centered vocal-band lines around 12..16s, 17..20.5s, 21.5..25s
        for &(s_ms, e_ms) in &[(12000usize, 16000usize), (17000, 20500), (21500, 25000)] {
            let s0 = (s_ms * sr as usize) / 1000;
            let s1 = (e_ms * sr as usize) / 1000;
            for f in s0..s1 {
                let t = f as f32 / sr as f32;
                let vox =
                    ((2.0 * PI * 780.0 * t).sin() + 0.6 * (2.0 * PI * 1350.0 * t).sin()) * 11000.0;
                pcm[f * 2] = vox.clamp(-32000.0, 32000.0) as i16;
                pcm[f * 2 + 1] = vox.clamp(-32000.0, 32000.0) as i16;
            }
        }

        let plain = vec![
            "First sample vocal line here".to_owned(),
            "Second sample vocal line here".to_owned(),
            "Third sample vocal line here".to_owned(),
        ];
        let aligned = align_plain_lyrics(&plain, &pcm, 2, sr, 26_000);
        assert_eq!(aligned.len(), 3);
        assert!(
            aligned[0].time_ms >= 10_500 && aligned[0].time_ms <= 13_500,
            "first line must wait for vocal entry around 12s, got {}ms",
            aligned[0].time_ms
        );
        assert!(aligned[1].time_ms > aligned[0].time_ms + 2500);
        assert!(aligned[2].time_ms > aligned[1].time_ms + 2500);
    }

    #[test]
    fn prevents_single_line_stall_between_verse_and_chorus() {
        let sr = 8000u32;
        let total_sec = 140usize;
        let mut pcm = vec![0i16; sr as usize * total_sec * 2];

        // Moderate centered vocal bursts across the early verse (5s..25s)
        for &(s_ms, e_ms) in &[
            (5000usize, 8000usize),
            (9000, 12000),
            (13000, 16500),
            (17500, 21000),
            (22000, 25500),
        ] {
            let s0 = (s_ms * sr as usize) / 1000;
            let s1 = (e_ms * sr as usize) / 1000;
            for f in s0..s1 {
                let t = f as f32 / sr as f32;
                let vox =
                    ((2.0 * PI * 760.0 * t).sin() + 0.5 * (2.0 * PI * 1300.0 * t).sin()) * 4000.0;
                pcm[f * 2] = vox as i16;
                pcm[f * 2 + 1] = vox as i16;
            }
        }
        // High-energy centered drop starting at 48s through 132s
        for f in (48 * sr as usize)..(132 * sr as usize) {
            let t = f as f32 / sr as f32;
            let vox =
                ((2.0 * PI * 820.0 * t).sin() + 0.6 * (2.0 * PI * 1400.0 * t).sin()) * 15000.0;
            pcm[f * 2] = vox.clamp(-32000.0, 32000.0) as i16;
            pcm[f * 2 + 1] = vox.clamp(-32000.0, 32000.0) as i16;
        }

        let plain: Vec<String> = (1..=20)
            .map(|idx| format!("Synthetic line number {idx} words"))
            .collect();
        let aligned = align_plain_lyrics(&plain, &pcm, 2, sr, 140_000);
        assert_eq!(aligned.len(), 20);
        assert!(
            aligned[0].time_ms <= 10_000,
            "line 0 drifted too late: {}ms",
            aligned[0].time_ms
        );
        assert!(
            aligned[1].time_ms <= 22_000,
            "line 1 stalled past early verse into drop: {}ms",
            aligned[1].time_ms
        );
    }

    #[test]
    fn generates_word_timings_that_respect_syllables_and_pauses() {
        let sr = 8000u32;
        let total_sec = 12usize;
        let mut pcm = vec![0i16; sr as usize * total_sec];
        // Line 0 vocal active 1.0..2.8s, silent interlude 2.8..5.5s, line 1 vocal 5.5..8.5s
        for &(s_ms, e_ms) in &[(1000usize, 2800usize), (5500, 8500)] {
            let s0 = (s_ms * sr as usize) / 1000;
            let s1 = (e_ms * sr as usize) / 1000;
            for (i, slot) in pcm.iter_mut().enumerate().take(s1).skip(s0) {
                let sample = (2.0 * PI * 850.0 * i as f32 / sr as f32).sin() * 14000.0;
                *slot = sample as i16;
            }
        }
        let plain = vec![
            "I remember".to_owned(),
            String::new(),
            "Everlasting melodies tonight".to_owned(),
        ];
        let aligned = align_plain_lyrics(&plain, &pcm, 1, sr, 12_000);
        assert_eq!(aligned.len(), 2);
        let w0 = &aligned[0].words;
        assert_eq!(w0.len(), 2);
        assert_eq!(w0[0].time_ms, aligned[0].time_ms);
        // Word endings must stop near vocal pause rather than crawling through the entire 5.5s gap
        let w0_end = w0.last().and_then(|w| w.end_ms).unwrap_or(0);
        assert!(
            w0_end < aligned[1].time_ms,
            "word end {w0_end}ms should stop before interlude end {}ms",
            aligned[1].time_ms
        );
        // Longer word ("remember" = 3 syl) receives noticeably more time than "I" (1 syl)
        let d0 = w0[0].end_ms.unwrap() - w0[0].time_ms;
        let d1 = w0[1].end_ms.unwrap() - w0[1].time_ms;
        assert!(d1 > d0, "expected d1 ({d1}ms) > d0 ({d0}ms)");
    }
}
