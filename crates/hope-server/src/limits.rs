//! Bounded, in-memory request limiters.
//!
//! Limiters count **failures** per client key (callers `check` first, `hit` on failure and
//! `clear` on success), so a successful login, enrollment or accepted report never consumes
//! budget. There is deliberately no global ceiling: any shared budget that attackers can fill from
//! many addresses would lock the administrator or legitimate agents out. CPU cost of password
//! hashing is bounded separately by a concurrency semaphore that fails transiently (503).
//! The key table is bounded; when full it drops expired entries, then the least recently hit
//! entry. Keys are never pooled, so new clients always get their own fresh budget.
use axum::{
    extract::{ConnectInfo, Request, State as Extract},
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub const MAX_KEYS: usize = 4096;

pub struct Limiter {
    window: f64,
    per_key: u32,
    inner: Mutex<HashMap<String, Entry>>,
}
#[derive(Clone, Copy)]
struct Entry {
    count: u32,
    start: f64,
    last: f64,
}

impl Limiter {
    pub fn new(window_seconds: f64, per_key: u32) -> Self {
        Self {
            window: window_seconds,
            per_key,
            inner: Mutex::default(),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Seconds to wait when the key has exhausted its failures. Consumes nothing.
    pub fn check(&self, key: &str, at: f64) -> Result<(), u64> {
        match self.lock().get(key) {
            Some(e) if at - e.start < self.window && e.count >= self.per_key => {
                Err((e.start + self.window - at).ceil().max(1.0) as u64)
            }
            _ => Ok(()),
        }
    }
    /// Record one failure for the key.
    pub fn hit(&self, key: &str, at: f64) {
        let mut map = self.lock();
        if !map.contains_key(key) && map.len() >= MAX_KEYS {
            let window = self.window;
            map.retain(|_, e| at - e.start < window);
            if map.len() >= MAX_KEYS
                && let Some(oldest) = map
                    .iter()
                    .min_by(|a, b| a.1.last.total_cmp(&b.1.last))
                    .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        let entry = map.entry(key.to_string()).or_insert(Entry {
            count: 0,
            start: at,
            last: at,
        });
        if at - entry.start >= self.window {
            *entry = Entry {
                count: 0,
                start: at,
                last: at,
            };
        }
        entry.count += 1;
        entry.last = at;
    }
    pub fn clear(&self, key: &str) {
        self.lock().remove(key);
    }
    pub fn len(&self) -> usize {
        self.lock().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Reverse proxies whose `X-Forwarded-For` header may be believed (`HOPE_TRUSTED_PROXIES`, a
/// comma-separated list of addresses or CIDR ranges).
///
/// When the variable is unset and `HOPE_BIND` is loopback-only (the documented same-host reverse
/// proxy setup), loopback is trusted by default: only local processes can reach such a listener,
/// and without this every client behind the proxy would share one rate-limit key. Any other bind
/// trusts nothing unless configured.
#[derive(Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
    warned_untrusted: Arc<AtomicBool>,
    warned_unparseable: Arc<AtomicBool>,
}

/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are treated as their IPv4 address.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}
/// Rate-limit key: IPv4 as is; IPv6 masked to its /64 so one host cannot rotate through addresses.
pub fn key_of(ip: IpAddr) -> String {
    match canonical(ip) {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

impl TrustedProxies {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let mut nets = vec![];
        for part in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let net = part
                .parse::<IpNet>()
                .or_else(|_| part.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| anyhow::anyhow!("HOPE_TRUSTED_PROXIES contains an invalid entry"))?;
            nets.push(net);
        }
        Ok(Self {
            nets,
            ..Self::default()
        })
    }
    /// `configured` is `HOPE_TRUSTED_PROXIES`; `bind` is `HOPE_BIND` (default loopback).
    pub fn resolve(configured: Option<&str>, bind: Option<&str>) -> anyhow::Result<Self> {
        if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
            return Self::parse(value);
        }
        let loopback_only = bind
            .unwrap_or("127.0.0.1:8080")
            .parse::<SocketAddr>()
            .is_ok_and(|addr| addr.ip().is_loopback());
        if loopback_only {
            Self::parse("127.0.0.0/8,::1")
        } else {
            Ok(Self::default())
        }
    }
    pub fn from_env() -> anyhow::Result<Self> {
        Self::resolve(
            std::env::var("HOPE_TRUSTED_PROXIES").ok().as_deref(),
            std::env::var("HOPE_BIND").ok().as_deref(),
        )
    }
    fn trusts(&self, ip: &IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(ip))
    }
    /// The nearest address not belonging to a trusted proxy: the socket peer unless it is trusted,
    /// then walking `X-Forwarded-For` from the right, skipping trusted proxies, the first
    /// untrusted hop. Client-supplied left-hand entries are never preferred, so a caller cannot
    /// choose its own rate-limit key.
    pub fn client(&self, peer: Option<IpAddr>, forwarded: Option<&str>) -> String {
        let Some(peer) = peer.map(canonical) else {
            return "unknown".into();
        };
        if !self.trusts(&peer) {
            if forwarded.is_some() && !self.warned_untrusted.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "ignoring X-Forwarded-For from an untrusted peer; set HOPE_TRUSTED_PROXIES if a reverse proxy sits in front"
                );
            }
            return key_of(peer);
        }
        if let Some(header) = forwarded {
            for hop in header.rsplit(',') {
                match hop.trim().parse::<IpAddr>().map(canonical) {
                    Ok(ip) if self.trusts(&ip) => continue,
                    Ok(ip) => return key_of(ip),
                    Err(_) => {
                        if !self.warned_unparseable.swap(true, Ordering::Relaxed) {
                            tracing::warn!(
                                "unparseable X-Forwarded-For hop from a trusted proxy; using the proxy address"
                            );
                        }
                        break;
                    }
                }
            }
        }
        key_of(peer)
    }
}

/// Rate-limit key of the requesting client, set by [`client_key`].
#[derive(Clone)]
pub struct ClientKey(pub String);

pub async fn client_key(
    Extract(state): Extract<crate::state::State>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let forwarded = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    let key = state
        .0
        .proxies
        .client(peer, (!forwarded.is_empty()).then_some(forwarded.as_str()));
    request.extensions_mut().insert(ClientKey(key));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failures_are_per_key_and_success_budget_is_untouched() {
        let limiter = Limiter::new(60.0, 3);
        for _ in 0..3 {
            assert!(limiter.check("a", 0.0).is_ok());
            limiter.hit("a", 0.0);
        }
        assert!(limiter.check("a", 1.0).is_err());
        assert!(limiter.check("b", 1.0).is_ok(), "other keys are unaffected");
        assert!(limiter.check("a", 61.0).is_ok(), "window expires");
        limiter.clear("a");
        assert!(limiter.check("a", 1.0).is_ok());
    }
    #[test]
    fn key_table_is_bounded_and_legitimate_keys_get_their_own_budget() {
        let limiter = Limiter::new(60.0, 3);
        limiter.hit("legit-existing", 0.0);
        for i in 0..(MAX_KEYS * 2) {
            limiter.hit(&format!("attacker{i}"), 1.0 + i as f64 * 0.001);
        }
        assert!(limiter.len() <= MAX_KEYS);
        // A new legitimate client is not pooled with attackers: it starts at zero failures...
        for _ in 0..2 {
            limiter.hit("legit-new", 100.0);
        }
        assert!(limiter.check("legit-new", 100.0).is_ok());
        limiter.hit("legit-new", 100.0);
        assert!(
            limiter.check("legit-new", 100.0).is_err(),
            "own budget, own limit"
        );
        // ... and an attacker key that was evicted starts fresh rather than blocking anyone.
        assert!(limiter.check("attacker0", 100.0).is_ok());
    }
    #[test]
    fn forwarded_header_is_only_trusted_from_configured_proxies() {
        let none = TrustedProxies::default();
        let peer: IpAddr = "10.0.0.9".parse().unwrap();
        assert_eq!(none.client(Some(peer), Some("1.2.3.4")), "10.0.0.9");
        let proxies = TrustedProxies::parse("10.0.0.0/24, 127.0.0.1").unwrap();
        // Rightmost untrusted hop wins; left-hand entries are client controlled.
        assert_eq!(
            proxies.client(Some(peer), Some("6.6.6.6, 1.2.3.4, 10.0.0.2")),
            "1.2.3.4"
        );
        assert_eq!(proxies.client(Some(peer), Some("garbage")), "10.0.0.9");
        assert_eq!(proxies.client(None, Some("1.2.3.4")), "unknown");
        let outsider: IpAddr = "203.0.113.5".parse().unwrap();
        assert_eq!(
            proxies.client(Some(outsider), Some("1.2.3.4")),
            "203.0.113.5"
        );
        assert!(TrustedProxies::parse("nope").is_err());
    }
    #[test]
    fn ipv6_is_masked_to_64_and_v4_mapped_addresses_are_unwrapped() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(key_of(a), key_of(b));
        assert_ne!(key_of(a), key_of(c));
        let none = TrustedProxies::default();
        assert_eq!(
            none.client(Some("::ffff:192.0.2.7".parse().unwrap()), None),
            "192.0.2.7"
        );
        // Trusted-proxy matching also sees the mapped form as IPv4.
        let proxies = TrustedProxies::parse("127.0.0.1").unwrap();
        assert_eq!(
            proxies.client(
                Some("::ffff:127.0.0.1".parse().unwrap()),
                Some("::ffff:198.51.100.4")
            ),
            "198.51.100.4"
        );
        assert_eq!(
            proxies.client(
                Some("::ffff:127.0.0.1".parse().unwrap()),
                Some("2001:db8:1:2:ffff::5")
            ),
            key_of(a)
        );
    }
    #[test]
    fn loopback_bind_trusts_loopback_by_default() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let on_loopback = TrustedProxies::resolve(None, Some("127.0.0.1:8080")).unwrap();
        assert_eq!(
            on_loopback.client(Some(lo), Some("198.51.100.4")),
            "198.51.100.4"
        );
        assert_eq!(
            TrustedProxies::resolve(Some(""), None)
                .unwrap()
                .client(Some(lo), Some("198.51.100.4")),
            "198.51.100.4"
        );
        let wildcard = TrustedProxies::resolve(None, Some("0.0.0.0:8080")).unwrap();
        assert_eq!(wildcard.client(Some(lo), Some("198.51.100.4")), "127.0.0.1");
        let explicit = TrustedProxies::resolve(Some("10.0.0.1"), Some("127.0.0.1:8080")).unwrap();
        assert_eq!(explicit.client(Some(lo), Some("198.51.100.4")), "127.0.0.1");
    }
}
