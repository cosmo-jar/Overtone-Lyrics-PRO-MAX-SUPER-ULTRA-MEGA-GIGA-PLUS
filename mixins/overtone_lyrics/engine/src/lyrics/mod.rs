pub mod cache;
pub mod clients;
pub mod dsp_aligner;
pub mod match_track;
pub mod parsers;

use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::config::{LyricsSourceMode, PlayerConfig};
use crate::model::{LyricCandidate, LyricSourceKind, Track};
pub use crate::model::{LyricLine, WordTiming};

pub fn current_index(lines: &[LyricLine], pos_ms: u64) -> Option<usize> {
    let mut active = None;
    for (i, line) in lines.iter().enumerate() {
        if line.is_synced() && line.time_ms <= pos_ms {
            active = Some(i);
        } else if line.time_ms > pos_ms {
            break;
        }
    }
    active
}

#[derive(Clone, Debug, Default)]
pub struct AudioDspInput {
    pub pcm: Vec<i16>,
    pub channels: usize,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Default)]
pub struct LyricsLookupResult {
    pub candidates: Vec<LyricCandidate>,
    pub active_idx: usize,
    pub plain_available: bool,
}

impl LyricsLookupResult {
    pub fn active(&self) -> Option<&LyricCandidate> {
        self.candidates.get(self.active_idx)
    }

    pub fn active_lines(&self) -> &[LyricLine] {
        self.active().map_or(&[], |c| c.lines.as_slice())
    }
}

pub fn score_candidate(source: LyricSourceKind, lines: &[LyricLine], is_auto_dsp: bool) -> f32 {
    if lines.is_empty() {
        return 0.0;
    }
    let has_words = lines.iter().any(LyricLine::has_word_timings);
    let mut s = match source {
        LyricSourceKind::Custom => 200.0,
        LyricSourceKind::Amll => 120.0,
        LyricSourceKind::Platform => 105.0,
        LyricSourceKind::LrcLib => 100.0,
        LyricSourceKind::NetEase => 96.0,
        LyricSourceKind::QqMusic => 90.0,
        LyricSourceKind::Musixmatch => 88.0,
        LyricSourceKind::KuGou => 85.0,
        LyricSourceKind::BetterLyrics => 82.0,
        LyricSourceKind::AutoDsp => 45.0,
    };
    if has_words {
        s += 18.0;
    }
    if is_auto_dsp {
        s -= 25.0;
    }
    s + (lines.len().min(40) as f32) * 0.25
}

fn current_targets() -> &'static Mutex<(Option<String>, Option<String>)> {
    static TARGETS: OnceLock<Mutex<(Option<String>, Option<String>)>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new((None, None)))
}

pub fn set_active_track_ids(wanted_id: Option<&str>, upcoming_id: Option<&str>) {
    if let Ok(mut g) = current_targets().lock() {
        *g = (wanted_id.map(str::to_owned), upcoming_id.map(str::to_owned));
    }
}

pub fn is_track_still_wanted(t: &Track) -> bool {
    current_targets()
        .lock()
        .map(|g| g.0.as_deref() == Some(t.id.as_str()) || g.1.as_deref() == Some(t.id.as_str()))
        .unwrap_or(true)
}

/// Resolves synced lyrics according to `LyricsSourceMode` with Level A cascade and
/// Level B Vocal-Activity DSP Auto-Aligner fallback when only unsynced text exists.
/// Strictly NEVER returns raw unsynced lines to the UI/HUD.
pub fn resolve_lyrics<F>(
    t: &Track,
    cfg: &PlayerConfig,
    platform_synced: F,
    platform_plain: impl Fn() -> Vec<String> + Send + 'static,
    dsp_audio: Option<Arc<AudioDspInput>>,
) -> LyricsLookupResult
where
    F: FnOnce(&Track) -> Vec<LyricLine> + Send + 'static,
{
    if let Some(custom) = cache::load_custom(t) {
        let synced_only: Vec<LyricLine> = custom.into_iter().filter(|l| !l.is_unsynced()).collect();
        if !synced_only.is_empty() {
            return LyricsLookupResult {
                candidates: vec![LyricCandidate {
                    source: LyricSourceKind::Custom,
                    score: score_candidate(LyricSourceKind::Custom, &synced_only, false),
                    has_word_timings: synced_only.iter().any(LyricLine::has_word_timings),
                    lines: synced_only,
                    is_auto_dsp: false,
                }],
                active_idx: 0,
                plain_available: false,
            };
        }
    }

    let allow_platform = !matches!(cfg.lyrics_source_mode, LyricsSourceMode::ExternalOnly);
    let allow_external =
        cfg.online_lyrics && !matches!(cfg.lyrics_source_mode, LyricsSourceMode::PlatformOnly);

    if let Some((src, lines)) = cache::get_cached(t) {
        let synced: Vec<LyricLine> = lines.into_iter().filter(|l| !l.is_unsynced()).collect();
        let src_allowed = ((src.is_external() && allow_external)
            || (!src.is_external() && allow_platform))
            && (src != LyricSourceKind::AutoDsp || cfg.dsp_auto_align);
        if !synced.is_empty() && src_allowed {
            return LyricsLookupResult {
                candidates: vec![LyricCandidate {
                    source: src,
                    score: score_candidate(src, &synced, src == LyricSourceKind::AutoDsp),
                    has_word_timings: synced.iter().any(LyricLine::has_word_timings),
                    lines: synced,
                    is_auto_dsp: src == LyricSourceKind::AutoDsp,
                }],
                active_idx: 0,
                plain_available: false,
            };
        }
    }

    let platform_first = matches!(
        cfg.lyrics_source_mode,
        LyricsSourceMode::PlatformFirst | LyricsSourceMode::PlatformOnly
    );

    type PlainBucket = Arc<Mutex<Vec<(LyricSourceKind, Vec<String>)>>>;
    let mut handles = Vec::new();
    let plain_bucket: PlainBucket = Arc::new(Mutex::new(Vec::new()));

    if allow_platform {
        let track_c = t.clone();
        let pb = Arc::clone(&plain_bucket);
        handles.push((
            LyricSourceKind::Platform,
            thread::spawn(move || {
                let synced = platform_synced(&track_c)
                    .into_iter()
                    .filter(|l| !l.is_unsynced())
                    .collect::<Vec<_>>();
                if synced.is_empty() {
                    let plain = platform_plain();
                    if !plain.is_empty()
                        && let Ok(mut g) = pb.lock()
                    {
                        g.push((LyricSourceKind::Platform, plain));
                    }
                }
                synced
            }),
        ));
    }

    if allow_external {
        if cfg.amll_lyrics {
            let tc = t.clone();
            handles.push((
                LyricSourceKind::Amll,
                thread::spawn(move || clients::fetch_amll(&tc).synced),
            ));
        }
        {
            let tc = t.clone();
            let pb = Arc::clone(&plain_bucket);
            handles.push((
                LyricSourceKind::LrcLib,
                thread::spawn(move || {
                    let res = clients::fetch_lrclib(&tc);
                    if res.synced.is_empty()
                        && !res.plain.is_empty()
                        && let Ok(mut g) = pb.lock()
                    {
                        g.push((LyricSourceKind::LrcLib, res.plain));
                    }
                    res.synced
                }),
            ));
        }
        if cfg.netease_lyrics {
            let tc = t.clone();
            let pb = Arc::clone(&plain_bucket);
            handles.push((
                LyricSourceKind::NetEase,
                thread::spawn(move || {
                    let res = clients::fetch_netease(&tc);
                    if res.synced.is_empty()
                        && !res.plain.is_empty()
                        && let Ok(mut g) = pb.lock()
                    {
                        g.push((LyricSourceKind::NetEase, res.plain));
                    }
                    res.synced
                }),
            ));
        }
        if cfg.qq_lyrics {
            let tc = t.clone();
            let pb = Arc::clone(&plain_bucket);
            handles.push((
                LyricSourceKind::QqMusic,
                thread::spawn(move || {
                    let res = clients::fetch_qq_music(&tc);
                    if res.synced.is_empty()
                        && !res.plain.is_empty()
                        && let Ok(mut g) = pb.lock()
                    {
                        g.push((LyricSourceKind::QqMusic, res.plain));
                    }
                    res.synced
                }),
            ));
        }
        if cfg.kugou_lyrics {
            let tc = t.clone();
            handles.push((
                LyricSourceKind::KuGou,
                thread::spawn(move || clients::fetch_kugou(&tc).synced),
            ));
        }
        if !cfg.musixmatch_token.trim().is_empty() {
            let tc = t.clone();
            let tok = cfg.musixmatch_token.clone();
            handles.push((
                LyricSourceKind::Musixmatch,
                thread::spawn(move || clients::fetch_musixmatch(&tc, &tok).synced),
            ));
        }
        if cfg.better_lyrics {
            let tc = t.clone();
            handles.push((
                LyricSourceKind::BetterLyrics,
                thread::spawn(move || clients::fetch_better_lyrics(&tc).synced),
            ));
        }
    }

    let mut candidates: Vec<LyricCandidate> = Vec::new();
    for (src, h) in handles {
        if let Ok(lines) = h.join()
            && !lines.is_empty()
            && lines.iter().all(|l| !l.is_unsynced())
        {
            let mut s = score_candidate(src, &lines, false);
            if platform_first && src == LyricSourceKind::Platform {
                s += 40.0;
            }
            let has_word_timings = lines.iter().any(LyricLine::has_word_timings);
            candidates.push(LyricCandidate {
                source: src,
                lines,
                score: s,
                has_word_timings,
                is_auto_dsp: false,
            });
        }
    }

    // Collect plain-text fallback when zero Level-A synced candidates exist so we can run DSP
    // (when enabled) or inform the user that plain lyrics exist if DSP is disabled.
    let mut plain_available = false;
    if candidates.is_empty() {
        let mut plain_sources = plain_bucket.lock().map(|g| g.clone()).unwrap_or_default();
        if plain_sources.is_empty() && allow_external {
            let ext_plain = clients::fetch_all_plain_cascade(t);
            if !ext_plain.is_empty() {
                plain_sources.push((LyricSourceKind::AutoDsp, ext_plain));
            }
        }
        plain_available = !plain_sources.is_empty();
        if cfg.dsp_auto_align
            && let Some(audio) = dsp_audio
            && let Some((_, best_plain)) = plain_sources.into_iter().next()
        {
            let aligned = dsp_aligner::align_plain_lyrics(
                &best_plain,
                &audio.pcm,
                audio.channels,
                audio.sample_rate,
                t.duration_ms,
            );
            if !aligned.is_empty() {
                candidates.push(LyricCandidate {
                    source: LyricSourceKind::AutoDsp,
                    score: score_candidate(LyricSourceKind::AutoDsp, &aligned, true),
                    has_word_timings: aligned.iter().any(LyricLine::has_word_timings),
                    lines: aligned,
                    is_auto_dsp: true,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some(best) = candidates.first() {
        cache::put_cached(t, best.source, &best.lines);
    }

    LyricsLookupResult {
        candidates,
        active_idx: 0,
        plain_available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_mode_switch_and_no_raw_unsynced_lyrics() {
        let track = Track {
            id: "test-1".into(),
            title: "Test Track".into(),
            artist: "Test Artist".into(),
            album: String::new(),
            duration_ms: 12_000,
            cover_url: None,
            provider: "yandex".into(),
        };
        let mut cfg = PlayerConfig {
            online_lyrics: false,
            lyrics_source_mode: LyricsSourceMode::PlatformOnly,
            dsp_auto_align: false,
            ..PlayerConfig::default()
        };

        // Platform returns only unsynced lines -> MUST return 0 candidates (never show raw unsynced)
        // while flagging plain_available = true so the UI can suggest enabling DSP.
        let res = resolve_lyrics(
            &track,
            &cfg,
            |_| vec![LyricLine::new(LyricLine::UNSYNCED, "Raw static line")],
            || vec!["Raw static line".to_owned()],
            None,
        );
        assert!(res.candidates.is_empty());
        assert!(res.plain_available);

        let res_empty = resolve_lyrics(&track, &cfg, |_| Vec::new(), Vec::new, None);
        assert!(res_empty.candidates.is_empty());
        assert!(!res_empty.plain_available);

        // Turn DSP on + provide audio -> produces AutoDsp candidate with real timestamps
        cfg.dsp_auto_align = true;
        let audio = Arc::new(AudioDspInput {
            pcm: vec![8000i16; 16000 * 8],
            channels: 1,
            sample_rate: 16000,
        });
        let res2 = resolve_lyrics(
            &track,
            &cfg,
            |_| Vec::new(),
            || vec!["Line one".to_owned(), "Line two".to_owned()],
            Some(audio),
        );
        assert_eq!(res2.candidates.len(), 1);
        assert_eq!(res2.candidates[0].source, LyricSourceKind::AutoDsp);
        assert!(!res2.candidates[0].lines[0].is_unsynced());
    }
}
