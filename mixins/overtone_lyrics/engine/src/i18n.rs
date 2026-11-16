use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde_json::Value;

const EN_JSON: &str = include_str!("../data/en_us.json");
const RU_JSON: &str = include_str!("../data/ru_ru.json");
const JA_JSON: &str = include_str!("../data/ja_jp.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    EnUs,
    RuRu,
    JaJp,
}

impl Language {
    pub fn from_code(code: &str) -> Self {
        let low = code.trim().to_ascii_lowercase();
        if low.starts_with("ru") {
            Self::RuRu
        } else if low.starts_with("ja") {
            Self::JaJp
        } else {
            Self::EnUs
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::EnUs => "en_us",
            Self::RuRu => "ru_ru",
            Self::JaJp => "ja_jp",
        }
    }
}

struct Translations {
    en: HashMap<String, String>,
    ru: HashMap<String, String>,
    ja: HashMap<String, String>,
}

fn parse_lang(raw: &str, extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(raw) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k, s.to_owned());
            }
        }
    }
    for &(k, v) in extra {
        map.insert(k.to_owned(), v.to_owned());
    }
    map
}

const EXTRA_EN: &[(&str, &str)] = &[
    ("overtone.set.lyrics_source_mode", "Lyrics source mode"),
    (
        "overtone.hint.lyrics_source_mode",
        "Choose whether synced lyrics come from external databases first (AMLL, LRCLIB, NetEase, QQ, KuGou) or from the music platform itself.",
    ),
    (
        "overtone.opt.lyricssourcemode.external_first",
        "External first",
    ),
    (
        "overtone.opt.lyricssourcemode.external_only",
        "External only",
    ),
    (
        "overtone.opt.lyricssourcemode.platform_first",
        "Platform first",
    ),
    (
        "overtone.opt.lyricssourcemode.platform_only",
        "Platform only",
    ),
    ("overtone.set.netease_lyrics", "NetEase Cloud Music lyrics"),
    (
        "overtone.hint.netease_lyrics",
        "Fetch LRC/word-synced YRC lyrics from NetEase Cloud Music when primary sources miss.",
    ),
    ("overtone.set.qq_lyrics", "QQ Music lyrics"),
    (
        "overtone.hint.qq_lyrics",
        "Fetch synced QRC/LRC lyrics from QQ Music.",
    ),
    (
        "overtone.set.dsp_auto_align",
        "DSP Auto-Aligner (unsynced -> timed)",
    ),
    (
        "overtone.hint.dsp_auto_align",
        "When only plain unsynced text exists and Overtone decodes audio locally, runs vocal-activity DSP + DTW to align lines to the track.",
    ),
    (
        "overtone.set.better_lyrics",
        "BetterLyrics / SimpMusic proxy",
    ),
    (
        "overtone.hint.better_lyrics",
        "Optional community proxy lookup (disabled by default).",
    ),
    (
        "overtone.set.musixmatch_token",
        "Musixmatch user token (optional)",
    ),
    ("overtone.set.world_enhanced_glow", "Enhanced soft glow"),
    (
        "overtone.hint.world_enhanced_glow",
        "Adds a dual-ring soft halo around 3D world letters when glow is active.",
    ),
    ("overtone.player.sync_line_now", "Sync line now"),
    ("overtone.player.candidate_next", "Source: %s"),
    ("overtone.player.auto_dsp_badge", "Auto-DSP"),
    ("overtone.platform.spotify", "Spotify"),
    (
        "overtone.err.lyrics_unsynced_blocked",
        "Unsynced text is hidden unless DSP alignment succeeds",
    ),
];

const EXTRA_RU: &[(&str, &str)] = &[
    (
        "overtone.set.lyrics_source_mode",
        "Приоритет источников текста",
    ),
    (
        "overtone.hint.lyrics_source_mode",
        "Откуда брать синхронизированный текст: сначала внешние базы (AMLL, LRCLIB, NetEase, QQ, KuGou) или сначала платформа.",
    ),
    (
        "overtone.opt.lyricssourcemode.external_first",
        "Сначала внешние базы",
    ),
    (
        "overtone.opt.lyricssourcemode.external_only",
        "Только внешние базы",
    ),
    (
        "overtone.opt.lyricssourcemode.platform_first",
        "Сначала платформа",
    ),
    (
        "overtone.opt.lyricssourcemode.platform_only",
        "Только платформа",
    ),
    ("overtone.set.netease_lyrics", "Тексты NetEase Cloud Music"),
    (
        "overtone.hint.netease_lyrics",
        "Искать LRC/пословный YRC текст в NetEase Cloud Music.",
    ),
    ("overtone.set.qq_lyrics", "Тексты QQ Music"),
    (
        "overtone.hint.qq_lyrics",
        "Искать синхронизированные тексты в QQ Music.",
    ),
    (
        "overtone.set.dsp_auto_align",
        "DSP авто-синхронизация текста",
    ),
    (
        "overtone.hint.dsp_auto_align",
        "Если есть только обычный текст без таймингов и звук декодируется плеером локально, выравнивает строки по голосу (VAD + DTW).",
    ),
    (
        "overtone.set.better_lyrics",
        "Прокси BetterLyrics / SimpMusic",
    ),
    (
        "overtone.hint.better_lyrics",
        "Дополнительный комьюнити-прокси для поиска текстов (выкл. по умолчанию).",
    ),
    ("overtone.set.musixmatch_token", "Токен Musixmatch (опц.)"),
    (
        "overtone.set.world_enhanced_glow",
        "Усиленное мягкое свечение",
    ),
    (
        "overtone.hint.world_enhanced_glow",
        "Добавляет двойной мягкий ореол свечения вокруг 3D-букв в мире.",
    ),
    ("overtone.player.sync_line_now", "Строка здесь"),
    ("overtone.player.candidate_next", "Источник: %s"),
    ("overtone.player.auto_dsp_badge", "Auto-DSP"),
    ("overtone.platform.spotify", "Spotify"),
    (
        "overtone.err.lyrics_unsynced_blocked",
        "Несинхронизированный текст скрыт, пока не выполнена DSP-синхронизация",
    ),
];

const EXTRA_JA: &[(&str, &str)] = &[
    ("overtone.set.lyrics_source_mode", "歌詞ソースモード"),
    (
        "overtone.hint.lyrics_source_mode",
        "外部データベース（AMLL・LRCLIB・NetEase・QQ・KuGou）と再生プラットフォームの優先順位を選びます。",
    ),
    ("overtone.opt.lyricssourcemode.external_first", "外部DB優先"),
    ("overtone.opt.lyricssourcemode.external_only", "外部DBのみ"),
    (
        "overtone.opt.lyricssourcemode.platform_first",
        "プラットフォーム優先",
    ),
    (
        "overtone.opt.lyricssourcemode.platform_only",
        "プラットフォームのみ",
    ),
    ("overtone.set.netease_lyrics", "NetEase 歌詞"),
    (
        "overtone.hint.netease_lyrics",
        "NetEase Cloud Music から LRC/YRC 歌詞を取得します。",
    ),
    ("overtone.set.qq_lyrics", "QQ Music 歌詞"),
    (
        "overtone.hint.qq_lyrics",
        "QQ Music から同期歌詞を取得します。",
    ),
    ("overtone.set.dsp_auto_align", "DSP 自動同期"),
    (
        "overtone.hint.dsp_auto_align",
        "タイムスタンプなしの歌詞のみ見つかった場合、ローカル音声のボーカル活動（VAD + DTW）で自動同期します。",
    ),
    (
        "overtone.set.better_lyrics",
        "BetterLyrics / SimpMusic プロキシ",
    ),
    (
        "overtone.hint.better_lyrics",
        "コミュニティプロキシ検索（デフォルト無効）。",
    ),
    (
        "overtone.set.musixmatch_token",
        "Musixmatch トークン (任意)",
    ),
    ("overtone.set.world_enhanced_glow", "ソフトグロー強化"),
    (
        "overtone.hint.world_enhanced_glow",
        "3Dワールド歌詞の周囲に二重リングのソフト発光を追加します。",
    ),
    ("overtone.player.sync_line_now", "今の行を同期"),
    ("overtone.player.candidate_next", "ソース: %s"),
    ("overtone.player.auto_dsp_badge", "Auto-DSP"),
    ("overtone.platform.spotify", "Spotify"),
    (
        "overtone.err.lyrics_unsynced_blocked",
        "DSP同期が完了するまで非同期テキストは表示されません",
    ),
];

static STORE: OnceLock<Translations> = OnceLock::new();
static CURRENT_LANG: RwLock<Language> = RwLock::new(Language::EnUs);

fn store() -> &'static Translations {
    STORE.get_or_init(|| Translations {
        en: parse_lang(EN_JSON, EXTRA_EN),
        ru: parse_lang(RU_JSON, EXTRA_RU),
        ja: parse_lang(JA_JSON, EXTRA_JA),
    })
}

pub fn set_language(lang: Language) {
    if let Ok(mut guard) = CURRENT_LANG.write() {
        *guard = lang;
    }
}

pub fn current_language() -> Language {
    CURRENT_LANG.read().map(|g| *g).unwrap_or(Language::EnUs)
}

pub fn full_key(key: &str) -> String {
    if key.starts_with("overtone.") || key.starts_with("key.") || key.starts_with("screen.") {
        key.to_owned()
    } else {
        format!("overtone.{key}")
    }
}

pub fn raw(key: &str) -> String {
    let k = full_key(key);
    let s = store();
    let table = match current_language() {
        Language::EnUs => &s.en,
        Language::RuRu => &s.ru,
        Language::JaJp => &s.ja,
    };
    table.get(&k).or_else(|| s.en.get(&k)).cloned().unwrap_or(k)
}

pub fn fmt(key: &str, args: &[&str]) -> String {
    let template = raw(key);
    if args.is_empty() {
        return template;
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut arg_idx = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && chars.peek() == Some(&'s') {
            chars.next();
            if let Some(a) = args.get(arg_idx) {
                out.push_str(a);
                arg_idx += 1;
            } else {
                out.push_str("%s");
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn get(key: &str) -> String {
    raw(key)
}

pub fn tracks(count: usize) -> String {
    let m10 = count % 10;
    let m100 = count % 100;
    let key = if m10 == 1 && m100 != 11 {
        "local.count.one"
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        "local.count.few"
    } else {
        "local.count.many"
    };
    let s = count.to_string();
    fmt(key, &[&s])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_all_languages_and_pluralization() {
        let _ = store();
        set_language(Language::EnUs);
        assert_eq!(get("platform.yandex"), "Yandex Music");
        set_language(Language::RuRu);
        assert_eq!(tracks(1), "1 трек");
        assert_eq!(tracks(2), "2 трека");
        assert_eq!(tracks(5), "5 треков");
        assert_eq!(tracks(11), "11 треков");
        assert_eq!(tracks(21), "21 трек");
        set_language(Language::JaJp);
        assert!(!get("settings").is_empty());
        set_language(Language::EnUs);
        assert_eq!(fmt("player.repeat", &["all"]), "Repeat: all");
    }
}
