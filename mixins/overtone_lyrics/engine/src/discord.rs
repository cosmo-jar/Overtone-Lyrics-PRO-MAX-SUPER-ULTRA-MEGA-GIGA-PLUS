//! Discord Rich Presence over local IPC (Windows named pipes & Unix domain sockets).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config;
use crate::i18n;
use crate::model::{Platform, Track};

const CLIENT_ID: &str = "1562608168792498358";
const MOD_DISPLAY_NAME: &str = "Overtone: Lyrics PRO MAX SUPER ULTRA MEGA GIGA PLUS";
const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const RECONNECT_MS: u64 = 15_000;
const MIN_SEND_MS: u64 = 2_500;

struct IpcConn {
    file: File,
}

impl IpcConn {
    fn open_any() -> Option<Self> {
        for p in candidate_paths() {
            #[cfg(windows)]
            {
                if let Ok(file) = OpenOptions::new().read(true).write(true).open(&p) {
                    let mut conn = Self { file };
                    let hello = json!({ "v": 1, "client_id": CLIENT_ID });
                    if let Ok((op, _)) = conn.call(OP_HANDSHAKE, &hello.to_string())
                        && op != OP_CLOSE
                    {
                        return Some(conn);
                    }
                }
            }
            #[cfg(unix)]
            {
                let _ = &p;
                if let Ok(stream) = std::os::unix::net::UnixStream::connect(&p) {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(4)));
                    // Convert UnixStream to File isn't portable, so on Unix keep cfg(unix) block if needed.
                    drop(stream);
                }
            }
        }
        None
    }

    fn call(&mut self, op: u32, payload: &str) -> std::io::Result<(u32, String)> {
        let body = payload.as_bytes();
        let mut pkt = Vec::with_capacity(8 + body.len());
        pkt.extend_from_slice(&op.to_le_bytes());
        pkt.extend_from_slice(&(body.len() as u32).to_le_bytes());
        pkt.extend_from_slice(body);
        self.file.write_all(&pkt)?;
        self.file.flush()?;

        let mut hdr = [0u8; 8];
        self.file.read_exact(&mut hdr)?;
        let reply_op = u32::from_le_bytes(hdr[..4].try_into().unwrap());
        let len = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize;
        if len > (1 << 20) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad discord frame",
            ));
        }
        let mut reply_body = vec![0u8; len];
        self.file.read_exact(&mut reply_body)?;
        Ok((reply_op, String::from_utf8_lossy(&reply_body).into_owned()))
    }
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        for i in 0..10 {
            out.push(PathBuf::from(format!(r"\\.\pipe\discord-ipc-{i}")));
        }
    }
    #[cfg(unix)]
    {
        let mut bases: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok())
            .filter(|v| !v.trim().is_empty())
            .collect();
        bases.push("/tmp".to_string());
        let subs = [
            "",
            "app/com.discordapp.Discord/",
            "snap.discord/",
            ".flatpak/com.discordapp.Discord/xdg-run/",
        ];
        for i in 0..10 {
            for b in &bases {
                for s in subs {
                    let p = PathBuf::from(b).join(format!("{s}discord-ipc-{i}"));
                    if p.exists() && !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

fn clip(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    let mut padded = if t.chars().count() < 2 {
        format!("{t}  ")
    } else {
        t.to_string()
    };
    if padded.chars().count() <= max_chars {
        if t.chars().count() < 2 {
            padded.chars().take(2).collect()
        } else {
            padded
        }
    } else {
        padded = padded.chars().take(max_chars.saturating_sub(1)).collect();
        padded.push('…');
        padded
    }
}

#[derive(Debug, Clone)]
pub struct PresenceSnapshot {
    pub track: Option<Track>,
    pub idle: bool,
    pub playing: bool,
    pub position_ms: i64,
    pub enabled: bool,
}

fn build_activity(snap: &PresenceSnapshot) -> Option<Value> {
    let t = snap.track.as_ref()?;
    if snap.idle {
        return None;
    }
    let mut a = json!({
        "type": 2,
        "details": clip(&t.title, 128),
    });
    let by = t.artist.trim();
    let state = if snap.playing {
        by.to_string()
    } else if by.is_empty() {
        i18n::get("discord.paused")
    } else {
        format!("{} · {}", i18n::get("discord.paused"), by)
    };
    if !state.trim().is_empty() {
        a["state"] = Value::String(clip(&state, 128));
    }
    if snap.playing {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let start = ((now_ms - snap.position_ms) / 1000) * 1000;
        let mut ts = json!({ "start": start });
        if t.duration_ms > 0 {
            ts["end"] = Value::Number((start + t.duration_ms.min(i64::MAX as u64) as i64).into());
        }
        a["timestamps"] = ts;
    }
    let large_image = match t.cover_url.as_deref() {
        Some(u) if u.starts_with("https://") => u,
        _ => "overtone",
    };
    let large_text = if t.album.trim().is_empty() {
        MOD_DISPLAY_NAME
    } else {
        t.album.trim()
    };
    let mut assets = json!({
        "large_image": large_image,
        "large_text": clip(large_text, 128),
    });
    if let Some(plat) = Platform::by_id_or_none(&t.provider) {
        assets["small_image"] = Value::String(plat.id().to_string());
        assets["small_text"] = Value::String(i18n::get(&format!("platform.{}", plat.id())));
    }
    a["assets"] = assets;
    Some(a)
}

fn same_except_drift(now: &Value, last: &Value) -> bool {
    let Some(a) = now
        .get("timestamps")
        .and_then(|v| v.get("start"))
        .and_then(|v| v.as_i64())
    else {
        return false;
    };
    let Some(b) = last
        .get("timestamps")
        .and_then(|v| v.get("start"))
        .and_then(|v| v.as_i64())
    else {
        return false;
    };
    if (a - b).abs() > 2500 {
        return false;
    }
    let mut x = now.clone();
    let mut y = last.clone();
    if let Some(m) = x.as_object_mut() {
        m.remove("timestamps");
    }
    if let Some(m) = y.as_object_mut() {
        m.remove("timestamps");
    }
    x == y
}

struct DiscordWorkerState {
    conn: Option<IpcConn>,
    next_connect_at: Instant,
    last_sent_at: Instant,
    last_activity: Option<Value>,
    nonce_seq: u64,
}

#[derive(Clone)]
pub struct DiscordPresence {
    latest: Arc<Mutex<PresenceSnapshot>>,
    worker: Arc<Mutex<DiscordWorkerState>>,
}

impl Default for DiscordPresence {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscordPresence {
    pub fn new() -> Self {
        Self {
            latest: Arc::new(Mutex::new(PresenceSnapshot {
                track: None,
                idle: true,
                playing: false,
                position_ms: 0,
                enabled: config::PlayerConfig::load().discord_presence,
            })),
            worker: Arc::new(Mutex::new(DiscordWorkerState {
                conn: None,
                next_connect_at: Instant::now(),
                last_sent_at: Instant::now() - Duration::from_secs(10),
                last_activity: None,
                nonce_seq: 1,
            })),
        }
    }

    pub fn update_snapshot(&self, snap: PresenceSnapshot) {
        *self.latest.lock().unwrap() = snap;
    }

    pub fn start_thread(&self) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("overtone-discord".to_string())
            .spawn(move || {
                loop {
                    me.tick();
                    std::thread::sleep(Duration::from_millis(2000));
                }
            })
            .ok();
    }

    pub fn tick(&self) {
        let snap = self.latest.lock().unwrap().clone();
        let mut st = self.worker.lock().unwrap();
        if !snap.enabled {
            Self::disconnect_locked(&mut st, true);
            return;
        }
        let now = Instant::now();
        if st.conn.is_none() {
            if now < st.next_connect_at {
                return;
            }
            st.next_connect_at = now + Duration::from_millis(RECONNECT_MS);
            st.conn = IpcConn::open_any();
            if st.conn.is_none() {
                return;
            }
            st.last_activity = None;
        }

        let activity = build_activity(&snap);
        if let (Some(cur), Some(prev)) = (&activity, &st.last_activity)
            && (cur == prev || same_except_drift(cur, prev))
        {
            return;
        }
        if activity.is_none() && st.last_activity.is_none() {
            return;
        }
        if now.duration_since(st.last_sent_at).as_millis() < MIN_SEND_MS as u128 {
            return;
        }
        if Self::send_locked(&mut st, activity.clone()).is_ok() {
            st.last_activity = activity;
            st.last_sent_at = now;
        } else {
            Self::disconnect_locked(&mut st, false);
        }
    }

    fn send_locked(st: &mut DiscordWorkerState, activity: Option<Value>) -> std::io::Result<()> {
        let pid = std::process::id();
        st.nonce_seq += 1;
        let nonce = format!("ovt-{}", st.nonce_seq);
        let mut args = json!({ "pid": pid });
        if let Some(a) = activity {
            args["activity"] = a;
        }
        let cmd = json!({
            "cmd": "SET_ACTIVITY",
            "args": args,
            "nonce": nonce,
        });
        let Some(conn) = st.conn.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no discord pipe",
            ));
        };
        let (op, _) = conn.call(OP_FRAME, &cmd.to_string())?;
        if op == OP_CLOSE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "discord closed pipe",
            ));
        }
        Ok(())
    }

    fn disconnect_locked(st: &mut DiscordWorkerState, clear: bool) {
        if st.conn.is_some() && clear {
            let _ = Self::send_locked(st, None);
        }
        st.conn = None;
        st.last_activity = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_json_and_drift() {
        let snap = PresenceSnapshot {
            track: Some(Track {
                id: "1".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_ms: 180_000,
                cover_url: Some("https://example.com/art.jpg".into()),
                provider: Platform::Yandex.id().to_owned(),
            }),
            idle: false,
            playing: true,
            position_ms: 10_000,
            enabled: true,
        };
        let a = build_activity(&snap).unwrap();
        assert_eq!(a["type"], 2);
        assert_eq!(a["details"], "Song");
        let mut b = a.clone();
        b["timestamps"]["start"] = json!(a["timestamps"]["start"].as_i64().unwrap() + 1000);
        assert!(same_except_drift(&a, &b));
    }
}
