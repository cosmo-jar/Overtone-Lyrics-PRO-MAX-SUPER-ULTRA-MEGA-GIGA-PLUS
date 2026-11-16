use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use ureq::{Agent, Resolver};
use url::Url;

const LOG_EVERY: Duration = Duration::from_secs(120);
const MAX_REDIRECTS: usize = 5;
pub const MAX_TEXT_BYTES: usize = 8 << 20;
pub const MAX_DOWNLOAD_BYTES: usize = 64 << 20;

const DOH_SERVERS: &[&str] = &[
    "https://1.1.1.1/dns-query",
    "https://8.8.8.8/resolve",
    "https://1.0.0.1/dns-query",
    "https://8.8.4.4/resolve",
];

static EXACT_HOSTS: &[&str] = &[
    "api.music.yandex.net",
    "music.yandex.ru",
    "avatars.yandex.net",
    "oauth.yandex.ru",
    "api-v2.soundcloud.com",
    "api-auth.soundcloud.com",
    "soundcloud.com",
    "on.soundcloud.com",
    "snd.sc",
    "soundcloud.app.goo.gl",
    "api.spotify.com",
    "accounts.spotify.com",
    "i.scdn.co",
    "www.youtube.com",
    "youtube.com",
    "music.youtube.com",
    "m.youtube.com",
    "youtu.be",
    "youtubei.googleapis.com",
    "i.ytimg.com",
    "lrclib.net",
    "raw.githubusercontent.com",
    "lyrics.kugou.com",
    "krcs.kugou.com",
    "mobileservice.kugou.com",
    "music.163.com",
    "interface.music.163.com",
    "interface3.music.163.com",
    "c.y.qq.com",
    "u.y.qq.com",
    "api.lyrics.ovh",
    "genius.com",
    "www.genius.com",
    "api.genius.com",
    "rulyrics.ru",
    "www.rulyrics.ru",
    "text-pesenok.ru",
    "www.text-pesenok.ru",
    "azlyrics.com",
    "www.azlyrics.com",
    "search.azlyrics.com",
    "lyrics.com",
    "www.lyrics.com",
    "apic-desktop.musixmatch.com",
    "lyrics-api.boidu.dev",
    "1.1.1.1",
    "1.0.0.1",
    "8.8.8.8",
    "8.8.4.4",
];

static SUFFIX_HOSTS: &[&str] = &[
    ".yandex.net",
    ".yandex.ru",
    ".sndcdn.com",
    ".soundcloud.com",
    ".soundcloud.cloud",
    ".youtube.com",
    ".googlevideo.com",
    ".ytimg.com",
    ".ggpht.com",
    ".googleusercontent.com",
    ".scdn.co",
    ".spotify.com",
    ".githubusercontent.com",
    ".kugou.com",
    ".music.126.net",
    ".163.com",
    ".qq.com",
    ".musixmatch.com",
    ".lrclib.net",
    ".lyrics.ovh",
    ".genius.com",
    ".rulyrics.ru",
    ".text-pesenok.ru",
    ".azlyrics.com",
    ".lyrics.com",
    ".boidu.dev",
    ".akamaized.net",
];

#[derive(Clone, Debug)]
struct DnsCacheEntry {
    ips: Vec<IpAddr>,
    expires_at: Instant,
}

static DNS_CACHE: OnceLock<Mutex<HashMap<String, DnsCacheEntry>>> = OnceLock::new();
static LOGGED_HOSTS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn dns_cache() -> &'static Mutex<HashMap<String, DnsCacheEntry>> {
    DNS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn logged_hosts() -> &'static Mutex<HashMap<String, Instant>> {
    LOGGED_HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn host_allowed(url_str: &str) -> Result<(Url, String), String> {
    let parsed = Url::parse(url_str).map_err(|e| format!("invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(format!("blocked non-https URL: {}", parsed.scheme()));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing URL host".to_owned())?
        .to_ascii_lowercase();
    if is_host_allowlisted(&host) {
        Ok((parsed, host))
    } else {
        Err(format!("blocked host: {host}"))
    }
}

pub fn is_host_allowlisted(host: &str) -> bool {
    let low = host.to_ascii_lowercase();
    EXACT_HOSTS.iter().any(|h| *h == low) || SUFFIX_HOSTS.iter().any(|s| low.ends_with(s))
}

struct DohFallbackResolver {
    doh_agent: Agent,
}

impl DohFallbackResolver {
    fn new() -> Self {
        let doh_agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout_read(Duration::from_secs(5))
            .build();
        Self { doh_agent }
    }

    fn resolve_doh(&self, host: &str) -> Vec<IpAddr> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return vec![ip];
        }
        if let Ok(guard) = dns_cache().lock()
            && let Some(entry) = guard.get(host)
            && entry.expires_at > Instant::now()
        {
            return entry.ips.clone();
        }
        for server in DOH_SERVERS {
            let url = format!("{server}?name={}&type=A", url_encode(host));
            let resp = self
                .doh_agent
                .get(&url)
                .set("Accept", "application/dns-json")
                .call();
            let Ok(resp) = resp else { continue };
            let Ok(text) = resp.into_string() else {
                continue;
            };
            let Ok(val) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let mut found = Vec::new();
            let mut ttl_sec = 300u64;
            if let Some(arr) = val.get("Answer").and_then(Value::as_array) {
                for ans in arr {
                    if ans.get("type").and_then(Value::as_u64) != Some(1) {
                        continue;
                    }
                    if let Some(ip_str) = ans.get("data").and_then(Value::as_str)
                        && let Ok(ip) = ip_str.parse::<Ipv4Addr>()
                    {
                        found.push(IpAddr::V4(ip));
                    }
                    if let Some(t) = ans.get("TTL").and_then(Value::as_u64) {
                        ttl_sec = ttl_sec.max(t);
                    }
                }
            }
            if !found.is_empty() {
                if let Ok(mut guard) = dns_cache().lock() {
                    guard.insert(
                        host.to_owned(),
                        DnsCacheEntry {
                            ips: found.clone(),
                            expires_at: Instant::now() + Duration::from_secs(ttl_sec.min(1800)),
                        },
                    );
                }
                return found;
            }
        }
        Vec::new()
    }
}

impl Resolver for DohFallbackResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        // Prefer IPv4 addresses ahead of IPv6 so broken IPv6 routes on Windows dual-stack
        // networks do not stall TLS handshakes, and fall back to DoH if OS DNS is empty.
        let mut addrs: Vec<SocketAddr> = match netloc.to_socket_addrs() {
            Ok(iter) => iter.collect(),
            Err(_) => Vec::new(),
        };
        if addrs.is_empty() {
            let (host, port) = match netloc.rsplit_once(':') {
                Some((h, p)) => (
                    h.trim_matches(|c| c == '[' || c == ']'),
                    p.parse::<u16>().unwrap_or(443),
                ),
                None => (netloc, 443),
            };
            let ips = self.resolve_doh(host);
            if ips.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::HostUnreachable,
                    format!("DNS + DoH failed for {host}"),
                ));
            }
            addrs = ips
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect();
        }
        addrs.sort_by_key(|a| u8::from(!a.is_ipv4()));
        Ok(addrs)
    }
}

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .timeout_write(Duration::from_secs(15))
            .redirects(0)
            .resolver(DohFallbackResolver::new())
            .build()
    })
}

fn log_transport_error(host: &str, err: &str) {
    let now = Instant::now();
    if let Ok(mut logged) = logged_hosts().lock() {
        let should_log = logged
            .get(host)
            .is_none_or(|&t| now.saturating_duration_since(t) > LOG_EVERY);
        if should_log {
            logged.insert(host.to_owned(), now);
            eprintln!("[overtone_lyrics] Cannot connect to {host}: {err}");
        }
    }
}

#[derive(Clone, Debug)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
    pub final_url: String,
    pub content_range: Option<String>,
    pub content_length: Option<u64>,
}

impl HttpReply {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub fn send(
    method: &str,
    url_str: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    max_bytes: usize,
) -> Result<HttpReply, String> {
    let mut current_url = url_str.to_owned();
    let mut current_method = method.to_owned();
    let mut current_body = body.map(|b| b.to_vec());

    for _ in 0..=MAX_REDIRECTS {
        let (_parsed, host) = host_allowed(&current_url)?;

        let mut last_err = None;
        let mut resp_opt = None;
        for attempt in 0..2 {
            let mut req = agent().request(&current_method, &current_url);
            for &(k, v) in headers {
                if k.eq_ignore_ascii_case("authorization")
                    && !current_url.eq_ignore_ascii_case(url_str)
                {
                    continue;
                }
                req = req.set(k, v);
            }
            if headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("user-agent"))
            {
                req = req.set(
                    "User-Agent",
                    "OvertoneLyrics/1.0 (+https://github.com/cosmo-jar)",
                );
            }

            let res = if let Some(ref bytes) = current_body {
                req.send_bytes(bytes)
            } else {
                req.call()
            };
            match res {
                Ok(r) => {
                    resp_opt = Some(r);
                    break;
                }
                Err(ureq::Error::Status(_code, r)) => {
                    resp_opt = Some(r);
                    break;
                }
                Err(ureq::Error::Transport(t)) => {
                    let msg = t.to_string();
                    if attempt == 0 {
                        if let Ok(mut cache) = dns_cache().lock() {
                            cache.remove(&host);
                        }
                    } else {
                        log_transport_error(&host, &msg);
                    }
                    last_err = Some(msg);
                }
            }
        }
        let resp = match resp_opt {
            Some(r) => r,
            None => {
                let msg = last_err.unwrap_or_default();
                return Err(format!("Cannot connect to {host}: {msg}"));
            }
        };

        let status = resp.status();
        if (300..400).contains(&status)
            && let Some(loc) = resp.header("location")
        {
            let next = Url::parse(&current_url)
                .and_then(|base| base.join(loc))
                .map_err(|_| "Redirect without valid location".to_owned())?;
            if status == 301 || status == 302 || status == 303 {
                current_method = "GET".to_owned();
                current_body = None;
            }
            current_url = next.to_string();
            continue;
        }

        let content_range = resp.header("content-range").map(|s| s.to_owned());
        let content_length = resp
            .header("content-length")
            .and_then(|s| s.trim().parse::<u64>().ok());

        let mut reader = resp.into_reader().take((max_bytes + 1) as u64);
        let mut out = Vec::new();
        reader
            .read_to_end(&mut out)
            .map_err(|e| format!("read error from {host}: {e}"))?;
        if out.len() > max_bytes {
            return Err("Response exceeded size cap".to_owned());
        }
        return Ok(HttpReply {
            status,
            body: out,
            final_url: current_url,
            content_range,
            content_length,
        });
    }
    Err("Too many redirects".to_owned())
}

pub fn post_form(
    url_str: &str,
    headers: &[(&str, &str)],
    form: &[(&str, &str)],
) -> Result<HttpReply, String> {
    let encoded: String = form
        .iter()
        .map(|&(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let mut all_headers = headers.to_vec();
    all_headers.push(("Content-Type", "application/x-www-form-urlencoded"));
    send(
        "POST",
        url_str,
        &all_headers,
        Some(encoded.as_bytes()),
        MAX_TEXT_BYTES,
    )
}

pub fn get_text(url_str: &str, headers: &[(&str, &str)]) -> Result<String, String> {
    let reply = send("GET", url_str, headers, None, MAX_TEXT_BYTES)?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    Ok(reply.text())
}

pub fn get_json(url_str: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let txt = get_text(url_str, headers)?;
    serde_json::from_str(&txt).map_err(|e| format!("JSON parse error: {e}"))
}

pub fn get_bytes(url_str: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let reply = send("GET", url_str, headers, None, MAX_DOWNLOAD_BYTES)?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    Ok(reply.body)
}

pub fn post_json(
    url_str: &str,
    headers: &[(&str, &str)],
    payload: &Value,
) -> Result<Value, String> {
    let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
    let mut all_headers = headers.to_vec();
    all_headers.push(("Content-Type", "application/json"));
    let reply = send("POST", url_str, &all_headers, Some(&body), MAX_TEXT_BYTES)?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    serde_json::from_slice(&reply.body).map_err(|e| format!("JSON parse error: {e}"))
}

pub fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub fn build_query_url(base: &str, params: &[(&str, &str)]) -> String {
    let mut u = String::from(base);
    let mut first = !base.contains('?');
    for &(k, v) in params {
        u.push(if first { '?' } else { '&' });
        first = false;
        u.push_str(&url_encode(k));
        u.push('=');
        u.push_str(&url_encode(v));
    }
    u
}

// =========================================================================
// Pure-Rust RFC 1321 MD5 implementation (Horizon Cargo.lock has no md-5)
// =========================================================================

pub fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

pub fn md5_hex(input: &[u8]) -> String {
    let bytes = md5_digest(input);
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1321_md5_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }

    #[test]
    fn allowlist_blocks_arbitrary_domains_and_http() {
        assert!(host_allowed("https://lrclib.net/api/get").is_ok());
        assert!(host_allowed("https://music.163.com/api/song/lyric").is_ok());
        assert!(host_allowed("http://lrclib.net/api/get").is_err());
        assert!(host_allowed("https://evil.example.org/steal").is_err());
    }
}
