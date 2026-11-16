# Overtone: Lyrics PRO MAX SUPER ULTRA MEGA GIGA PLUS

[English](#english) · [Русский](#русский)

- **Mod ID:** `overtone_lyrics`
- **Version:** `1.0.0`
- **Target:** Horizon Rust Minecraft `26.3` (`horizon_revision = 9f42064c11efe7ef3e5ed2baad7ddc2a3b36eea0`, `hooks = 415`)
- **Authors:** `cosmo-jar (fork)`, `chifir4ik`, `PabJlo`
- **License:** MIT (`Copyright (c) 2026 cosmo-jar`, `Copyright (c) 2026 chifir4ik, PabJlo`)

---

## English

Native Rust port of [Overtone](https://github.com/chifir4ik/Overtone) (`Fabric 26.3`) for **Horizon 26.3**, featuring a pure-Rust audio engine, YouTube / YouTube Music streaming (no `yt-dlp`), a multi-source synchronized lyrics cascade, an offline DSP vocal-onset aligner, and enhanced 3D world-space glow.

### Music Sources & Account Management

All 5 sources support instant switching from the platform picker (`M`), one-click logout from both the top bar and picker cards, and local caching:

1. **Yandex Music**
   - Login via Device Code or browser OAuth link, saved account switcher with per-account removal (`x`), and logout that preserves saved accounts.
   - "My Wave" infinite radio (`user:onyourwave` + genre/mood/activity/epoch stations), Liked tracks, Albums | History, Artist/Album/Playlist pages, Likes & Dislikes, plus seeking without a subscription.
2. **YouTube & YouTube Music (pure Rust, no `yt-dlp`)**
   - Works without an account: unified search across YouTube Music (`WEB_REMIX`) + YouTube (`WEB`), direct URL paste (`youtube.com/watch?v=...`, `youtu.be/...`, `music.youtube.com/playlist?list=...`), region-aware **Charts / Hits** (`FEmusic_charts` auto-detected by IP + UI language), local likes (`youtube_likes.json`), built-in `MPLYt_...` lyrics, and `captionTracks` (`fmt=json3`).
   - **Optional Account Login** (for your personal Liked Music `LM`, private playlists, and like sync):
     1. Open `https://music.youtube.com` in Chromium / Edge / Chrome while signed in and press **`F12` -> `Network`**.
     2. Filter by **`Fetch/XHR`** and pick a successful `200` request such as **`browse?ctoken=...`**, **`account_menu?prettyPrint=false`**, or **`get?prettyPrint=false`** *(avoid red `net::ERR_BLOCKED_BY_CLIENT` requests like `log_event` blocked by ad-blockers, because browsers omit `Cookie` from them)*.
     3. Right-click that `200` request -> **Copy** -> **Copy as cURL (bash)** *(important: `(bash)`, or right-click the `cookie:` header value -> **Copy value**)*.
     4. Paste the copied text into the YouTube login field in Overtone (`Ctrl+V`) and click **Confirm** (the mod extracts `Cookie` and signs InnerTube requests with `SAPISIDHASH`).
3. **SoundCloud**
   - Anonymous search & playback, or link a profile URL (`soundcloud.com/<user>`) / paste `oauth_token` for likes & playlists; progressive MP3 + HLS / fMP4 AAC streaming.
4. **Spotify**
   - PKCE browser OAuth (`http://127.0.0.1:43821/callback`) using your own Spotify Developer App `Client ID` (`show_dialog=true` enabled so switching accounts works cleanly after logout).
   - Remote playback control, volume curve sync, queue/search/liked/playlists, and external lyrics sync *(Spotify Web API requires Premium for playback control)*.
5. **Local Files**
   - Folder scan and drag-and-drop onto the player window for `.mp3`, `.flac`, `.ogg`, `.m4a`, `.aac`, `.wav`, `.aiff`, `.au`, ID3 / Vorbis / FLAC / MP4 tag parsing (with Windows-1251 fallback), and `.lrc` / `.elrc` / `.ttml` sidecar files.

### Pure-Rust Audio Engine

- Built-in decoders (no FFmpeg / no `yt-dlp`): MP3 (`CBR` & `VBR/Xing/LAME`), AAC-LC (raw / `ADTS` / `MP4` muxed & fragmented `fMP4`), FLAC, Ogg Vorbis (`symphonia`), PCM WAV.
- Linear resampler -> `cpal` lock-free audio output ringbuffer + real-time FFT spectrum analyzer.
- Independent mod volume slider (decoupled from Minecraft's vanilla music volume).

### Synchronized Lyrics Cascade & Auto-DSP

- **Source Priority Modes:** `ExternalFirst`, `ExternalOnly`, `PlatformFirst`, `PlatformOnly`.
- **Level A — Synchronized Cascade:** `AMLL-TTML-DB` (word-level `.ttml`), `LRCLIB` (`/api/get` + `/api/search`), `NetEase Cloud Music` (`yrc` + `lrc`), `QQ Music` (`qrc` + `lrc`), `KuGou` (`krc`), optional `Musixmatch` (`userToken`) & `BetterLyrics` proxy, built-in provider lyrics (Yandex / YouTube Music `MPLYt_` & `json3`), and local `.lrc` / `.ttml` overrides.
- **Strict `TrackMatch` Validation:** `ё -> е` normalization strictly inside internal lookup keys (display titles/lyrics untouched), remix/live/cover/acoustic filtering, duration & script checks, top-candidate switcher, and live **Re-anchor (`Here`)** timestamp shift.
- **Level B — Built-in DSP Auto-Aligner:** Unsynced (`UNSYNCED`) lyrics are never rendered raw as static walls of text. Plain lyrics (from providers or `Genius`, `RuLyrics`, `text-pesenok.ru`, `AZLyrics`, `Lyrics.com`, `lyrics.ovh`, plain `LRCLIB`/`NetEase`/`QQ`) are aligned against locally decoded audio (Yandex, SoundCloud, YouTube, Local files) via `300-3400 Hz` vocal-band + center-channel + spectral-flux analysis and constrained DTW.

### HUD, 3D World Lyrics, Cinema Camera & Hotkeys

- **6 HUD Widget Layouts:** `Card`, `Classic`, `Bar`, `Compact`, `Hotbar`, `Vanilla`, drag-and-drop HUD layout editor, and HSV Color Picker.
- **2D Subtitles & Karaoke:** Word-level sweep synchronized identically with 3D world word progress.
- **3D World Lyrics:** `Follow` / `Fixed` modes, `Line` / `Scatter` layouts, 8 entrance animations (`Fly`, `Rise`, `Drop`, `Pop`, `Wave`, `Type`, `Spin`, `Glitch`), 7 exit animations (`Fade`, `Fall`, `Shatter`, `Float`, `Wind`, `Shrink`, `Sink`) with block collision/occlusion, plus toggleable dual-ring **`Enhanced Glow`**.
- **4 Visualizer Styles:** `Bars`, `Blocks`, `Mirror`, `Wave` (`Auto` / `Mod` / `System`).
- **Cinema Camera (`Auto-showroom`):** Overhead Cinema views automatically pitch `Follow` 3D world lyrics toward the camera without changing normal fixed horizontal mode.
- **Discord Rich Presence** & localizations (`en_us`, `ru_ru`, `ja_jp`).
- **Default Hotkeys (rebindable):** `M` Open Player · `[` / `]` Prev / Next · `;` Play / Pause · `-` / `=` Volume · `H` Toggle HUD · `J` HUD Settings · `O` 3D World Lyrics · `K` Cinema Camera.

### Security & Privacy Notice

> [!CAUTION]
> **Never share your Horizon `config/overtone` folder** (on Windows: `%AppData%\horizon\config\overtone`, i.e. `...\AppData\Roaming\horizon\config\overtone`).
> That folder stores `tokens.json` with your local OAuth tokens (Yandex Music, Spotify, SoundCloud) and YouTube Music session cookies. Anyone with access to it can access your linked accounts.

---

## Русский

Нативный Rust-порт [Overtone](https://github.com/chifir4ik/Overtone) (`Fabric 26.3`) для **Horizon 26.3** со встроенным аудиодвижком на чистом Rust, поддержкой YouTube / YouTube Music (без `yt-dlp`), многоуровневым каскадом синхронизированных текстов, офлайн DSP-синхронизацией по вокалу и усиленным 3D-свечением текста в мире.

### Музыкальные площадки и аккаунты

Доступны 5 площадок с переключением без прерывания трека и кнопкой **`Выйти`** как в верхней шапке плеера, так и на карточках выбора площадки:

1. **Яндекс Музыка**
   - Вход по Device Code или OAuth-ссылке в браузере, список сохранённых аккаунтов (быстрое переключение и удаление по `x`), выход с сохранением списка аккаунтов.
   - «Моя волна» (`user:onyourwave` + станции по жанрам и настроению), «Лайки», «Альбомы | История», страницы артистов/альбомов/плейлистов, лайки/дизлайки и перемотка работают **без подписки**.
2. **YouTube и YouTube Music (чистый Rust, без `yt-dlp`)**
   - Работает сразу без входа: поиск по YouTube Music и обычному YouTube, вставка прямых ссылок (`youtube.com/watch?v=...`, `youtu.be/...`, `music.youtube.com/playlist?list=...`), вкладка **«Чарты / Хиты»** (страна определяется автоматически по IP, язык — по локали мода), локальные лайки (`youtube_likes.json`), встроенные тексты YTM (`MPLYt_...`) и субтитры `captionTracks` (`json3`).
   - **Вход в аккаунт YouTube Music** (открывает личные «Понравившиеся» `LM`, личные плейлисты и синхронизацию лайков):
     1. Открой `https://music.youtube.com` в браузере (Chrome / Edge / Яндекс), войди в Google-аккаунт и нажми **`F12` → вкладка `Network` (`Сеть`)**.
     2. Включи фильтр **`Fetch/XHR`** и выбери любой успешный запрос со статусом **`200`**, например **`browse?ctoken=...`**, **`account_menu?prettyPrint=false`** или **`get?prettyPrint=false`**.
        > Важно: **не выбирай** красные заблокированные запросы вроде `log_event?alt=json` (`ERR_BLOCKED_BY_CLIENT`, `Provisional headers are shown`) — блокировщик рекламы режет их до отправки, и браузер не подставляет туда `Cookie`.
     3. Нажми на выбранный запрос (`200`) **правой кнопкой → `Copy` (`Копировать`) → `Copy as cURL (bash)`** *(именно `(bash)`, не `(cmd)`; либо во вкладке `Headers → Request headers` скопируй значение `cookie:` через `Copy value`)*.
     4. Вставь скопированное целиком (`Ctrl+V`) в поле входа YouTube в моде и нажми **Подтвердить** — мод сам извлечёт `Cookie` и подпишет запросы заголовком `SAPISIDHASH`.
3. **SoundCloud**
   - Анонимный поиск и воспроизведение или привязка по ссылке на профиль / `oauth_token` (лайки и плейлисты); поддержка progressive MP3 и HLS / fMP4 AAC.
4. **Spotify**
   - PKCE OAuth через браузер (`http://127.0.0.1:43821/callback`) со своим `Client ID` приложения (`show_dialog=true` позволяет сменить аккаунт после выхода).
   - Управление воспроизведением, синхронизация громкости, очередь/поиск/лайки и внешние тексты *(для управления воспроизведением через API Spotify требуется Premium)*.
5. **Мои файлы (Локальная медиатека)**
   - Поддержка drag-and-drop файлов и папок в окно плеера: `.mp3`, `.flac`, `.ogg`, `.m4a`, `.aac`, `.wav`, `.aiff`, `.au`, чтение тегов ID3 / Vorbis / FLAC / MP4 (с автоисправлением Windows-1251) и локальных `.lrc` / `.elrc` / `.ttml`.

### Аудиодвижок и визуализация

- Встроенные декодеры без FFmpeg и внешних программ: `MP3`, `AAC-LC` (`ADTS` + `MP4`/`fMP4`), `FLAC`, `Ogg Vorbis`, `WAV` -> ресемплер -> `cpal` + FFT-анализатор спектра.
- Независимый ползунок громкости мода (отвязан от ползунка музыки Minecraft).

### Система текстов и Auto-DSP

- **Режимы источников:** `ExternalFirst` / `ExternalOnly` / `PlatformFirst` / `PlatformOnly`.
- **Уровень A — Каскад синхронизированных баз:** `AMLL-TTML-DB` (пословно), `LRCLIB`, `NetEase Cloud Music` (`yrc` + `lrc`), `QQ Music` (`qrc` + `lrc`), `KuGou` (`krc`), опционально `Musixmatch` и `BetterLyrics`, встроенные тексты площадок (Яндекс, YouTube Music `MPLYt_` и `json3`), а также локальные `.lrc` / `.ttml`.
- **Строгий `TrackMatch`:** нормализация `ё -> е` только во внутренних ключах поиска, фильтрация несовпадающих ремиксов/лайвов/каверов, проверка длительности и алфавита, переключение кандидатов и живая кнопка **`Строка здесь`** (подгонка текущей строки с пропорциональным сдвигом остатка).
- **Уровень B — Встроенный DSP Auto-Aligner:** несинхронизированный текст без таймингов никогда не выводится статичной стеной. Для обычных текстов (встроенных или из `Genius`, `RuLyrics`, `text-pesenok.ru`, `AZLyrics`, `Lyrics.com`, `lyrics.ovh`, plain `LRCLIB`/`NetEase`/`QQ`) на потоках с локальным декодированием (Яндекс, SoundCloud, YouTube, файлы) анализируется вокальная полоса `300–3400 Гц`, центральный канал и Spectral Flux с выравниванием через Constrained DTW.

### Интерфейс, 3D-текст, Кино-камера и клавиши

- **6 стилей HUD:** `Card`, `Classic`, `Bar`, `Compact`, `Hotbar`, `Vanilla`, перетаскивание позиции HUD, HSV Color Picker, переводы `ru_ru`, `en_us`, `ja_jp`, Discord Rich Presence.
- **2D субтитры и караоке:** пословная подсветка синхронна с 3D-словами в мире 1:1.
- **3D-текст в мире:** режимы `Follow` / `Fixed`, раскладки `Line` / `Scatter`, 8 анимаций входа и 7 анимаций выхода (включая осколки и коллизию с блоками), плюс переключаемое усиленное свечение **`Enhanced Glow`**.
- **4 стиля визуализатора:** `Bars`, `Blocks`, `Mirror`, `Wave` (`Auto` / `Mod` / `System`).
- **Кино-камера (`Auto-showroom`):** на кадрах сверху 3D-текст автоматически наклоняется к камере (при обычной игре сохраняется фиксированное горизонтальное положение).
- **Клавиши по умолчанию (настраиваются):** `M` Плеер · `[` / `]` Пред./След. · `;` Пауза · `-` / `=` Громкость · `H` HUD · `J` Настройки HUD · `O` 3D-текст · `K` Кино-камера.

### Безопасность и конфиденциальность

> [!CAUTION]
> **Никому не передавайте папку `horizon\config\overtone`** (на Windows: `%AppData%\horizon\config\overtone`, то есть `...\AppData\Roaming\horizon\config\overtone`) и не выкладывайте её в общий доступ!
> Там локально хранится файл `tokens.json` с вашими токенами (Яндекс Музыка, Spotify, SoundCloud) и сессионными `Cookie` для YouTube Music — любой, кто получит его, получит доступ к вашим аккаунтам.
