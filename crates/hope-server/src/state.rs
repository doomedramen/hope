use anyhow::{Context, Result};
use hope_protocol::{Capabilities, Host};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct State(pub Arc<Inner>);

pub struct Inner {
    pub db: SqlitePool,
    pub events: broadcast::Sender<String>,
    pub writes: Mutex<()>,
    pub scans: Mutex<HashMap<String, CancellationToken>>,
    /// Identifiers of SSH jobs whose tasks are running in this process.
    pub installs: Mutex<HashSet<String>>,
    pub ssh_limits: std::sync::Mutex<crate::ssh::Limits>,
    pub server_url: String,
    pub secure_cookie: bool,
    pub capabilities: Capabilities,
    pub release_dir: String,
    pub enrollment_ca: Option<String>,
    pub scheduler: crate::scanner::Scheduler,
    pub enrichment: crate::enrichment::Queue,
    /// Cancelled when the process begins shutting down.
    pub shutdown: CancellationToken,
    pub proxies: crate::limits::TrustedProxies,
    /// Failed logins per client (no global ceiling; see `limits`).
    pub login_limit: crate::limits::Limiter,
    /// Bounds concurrent password hashing; exhaustion is a transient 503, never a lockout.
    pub hashing: tokio::sync::Semaphore,
    /// `scheme://host[:port]` of the canonical URL, normalized for Origin comparison.
    pub canonical_origin: String,
    /// Probe and enrichment tasks that may still write to the database.
    pub active_tasks: std::sync::atomic::AtomicUsize,
    /// Failed enrollments per client.
    pub enroll_limit: crate::limits::Limiter,
    /// Rejected agent report credentials per client.
    pub report_denied: crate::limits::Limiter,
    /// Reports per authenticated agent identity (no global ceiling).
    pub report_limit: crate::limits::Limiter,
    /// Client for reading device web pages and UPnP descriptions (LAN only, never proxied).
    pub http: reqwest::Client,
    /// Client for notification services (honours the usual proxy variables).
    pub notifier: reqwest::Client,
    /// What devices announced in the latest mDNS and SSDP sweeps, by address.
    pub sightings: std::sync::Mutex<HashMap<std::net::Ipv4Addr, crate::fingerprint::Announcement>>,
    /// Addresses known to route traffic: the server's default gateway and traceroute hops.
    pub gateways: std::sync::Mutex<HashSet<String>>,
}

/// Serialized origin of an absolute URL (lowercase host, default port omitted), or None.
pub fn normalize_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// Counts a background task that may write to the database; shutdown waits for these.
pub struct TaskGuard(State);
impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0
            .0
            .active_tasks
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

impl State {
    pub async fn open(
        path: &str,
        server_url: String,
        release_dir: String,
        capabilities: Capabilities,
    ) -> Result<Self> {
        Self::open_with_scheduler(path, server_url, release_dir, capabilities, None).await
    }
    /// Like `open`, with an explicit custom CA path for agents instead of `HOPE_AGENT_CA`.
    pub async fn open_with_agent_ca(
        path: &str,
        server_url: String,
        release_dir: String,
        capabilities: Capabilities,
        agent_ca: Option<String>,
    ) -> Result<Self> {
        let state =
            Self::open_with_scheduler(path, server_url, release_dir, capabilities, None).await?;
        // Nothing else holds the state yet, so the override is applied before sharing.
        let mut inner =
            Arc::try_unwrap(state.0).map_err(|_| anyhow::anyhow!("state already shared"))?;
        inner.enrollment_ca = agent_ca.filter(|s| !s.trim().is_empty());
        Ok(Self(Arc::new(inner)))
    }
    /// Replace the trusted reverse-proxy list (otherwise read from `HOPE_TRUSTED_PROXIES`).
    pub fn with_trusted_proxies(self, proxies: crate::limits::TrustedProxies) -> Result<Self> {
        let mut inner =
            Arc::try_unwrap(self.0).map_err(|_| anyhow::anyhow!("state already shared"))?;
        inner.proxies = proxies;
        Ok(Self(Arc::new(inner)))
    }
    pub(crate) async fn open_with_scheduler(
        path: &str,
        server_url: String,
        release_dir: String,
        mut capabilities: Capabilities,
        scheduler: Option<crate::scanner::Scheduler>,
    ) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let db = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        crate::migrations::run(&db).await?;
        sqlx::query("UPDATE scans SET status='interrupted', finished_at=?, error='Server restarted; start a new scan' WHERE status IN ('queued','running')")
            .bind(now()).execute(&db).await?;
        sqlx::query("UPDATE install_jobs SET status='interrupted', finished_at=?, error_code='SERVER_RESTARTED', message=? WHERE status='running'")
            .bind(now()).bind(crate::ssh::SERVER_RESTARTED_MESSAGE).execute(&db).await?;
        let (events, _) = broadcast::channel(256);
        let scheduler =
            scheduler.unwrap_or_else(|| crate::scanner::Scheduler::native(capabilities.arp));
        capabilities.arp = capabilities.arp && scheduler.arp.available();
        if !capabilities.arp && capabilities.limitations.is_empty() {
            capabilities.limitations.push(
                "Active ARP is unavailable: the server could not open a raw packet socket on any network interface, which needs the CAP_NET_RAW capability. Devices are still found by ping and TCP, but MAC addresses, manufacturers and most device types are missing. The server log says why for each interface (journalctl -u hope | grep ARP)."
                    .into(),
            );
        }
        let state = Self(Arc::new(Inner {
            db,
            events,
            writes: Mutex::new(()),
            scans: Mutex::new(HashMap::new()),
            installs: Mutex::new(HashSet::new()),
            ssh_limits: std::sync::Mutex::new(crate::ssh::Limits::default()),
            hashing: tokio::sync::Semaphore::new(2),
            canonical_origin: normalize_origin(&server_url).unwrap_or_default(),
            active_tasks: std::sync::atomic::AtomicUsize::new(0),
            secure_cookie: server_url.starts_with("https://"),
            server_url,
            capabilities,
            release_dir,
            enrollment_ca: std::env::var("HOPE_AGENT_CA")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            scheduler,
            enrichment: crate::enrichment::Queue::new(),
            shutdown: CancellationToken::new(),
            proxies: crate::limits::TrustedProxies::from_env()?,
            login_limit: crate::limits::Limiter::new(60.0, 10),
            enroll_limit: crate::limits::Limiter::new(60.0, 20),
            report_denied: crate::limits::Limiter::new(60.0, 30),
            report_limit: crate::limits::Limiter::new(60.0, 120),
            http: crate::fingerprint::http_client(),
            notifier: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("H.O.P.E notifications")
                .build()
                .unwrap_or_default(),
            sightings: std::sync::Mutex::new(HashMap::new()),
            gateways: std::sync::Mutex::new(HashSet::new()),
        }));
        Ok(state)
    }
    pub fn track(&self) -> TaskGuard {
        self.0
            .active_tasks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TaskGuard(self.clone())
    }
    /// Snapshot of the known gateway addresses.
    pub fn gateway_set(&self) -> HashSet<String> {
        self.0
            .gateways
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
    /// Merge what the latest sweeps saw at any of the host's addresses into its evidence.
    pub fn apply_sightings(&self, host: &mut Host) {
        let Ok(sightings) = self.0.sightings.lock() else {
            return;
        };
        for address in &host.addresses {
            let Some(seen) = address.parse().ok().and_then(|ip| sightings.get(&ip)) else {
                continue;
            };
            let e = &mut host.evidence;
            e.mdns_name = seen.mdns_name.clone().or(e.mdns_name.take());
            if !seen.mdns_services.is_empty() {
                e.mdns_services = seen.mdns_services.iter().cloned().collect();
            }
            e.mdns_model = seen.mdns_model.clone().or(e.mdns_model.take());
            if !seen.mdns_instances.is_empty() {
                e.mdns_instances = seen.mdns_instances.iter().cloned().collect();
            }
            e.ssdp_server = seen.ssdp_server.clone().or(e.ssdp_server.take());
            e.upnp_device_type = seen.upnp_device_type.clone().or(e.upnp_device_type.take());
            e.upnp_friendly_name = seen
                .upnp_friendly_name
                .clone()
                .or(e.upnp_friendly_name.take());
            e.upnp_manufacturer = seen
                .upnp_manufacturer
                .clone()
                .or(e.upnp_manufacturer.take());
            e.upnp_model = seen.upnp_model.clone().or(e.upnp_model.take());
        }
    }
    pub fn changed(&self) {
        let _ = self.0.events.send("refresh".into());
    }
    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key=?")
            .bind(key)
            .fetch_optional(&self.0.db)
            .await?)
    }
    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(key).bind(value).execute(&self.0.db).await?;
        Ok(())
    }
    pub async fn hosts(&self) -> Result<Vec<Host>> {
        let rows = sqlx::query("SELECT body FROM hosts ORDER BY name COLLATE NOCASE")
            .fetch_all(&self.0.db)
            .await?;
        rows.into_iter()
            .map(|r| serde_json::from_str(r.get("body")).context("invalid stored host"))
            .collect()
    }
    pub async fn host(&self, id: &str) -> Result<Option<Host>> {
        let value: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE id=?")
            .bind(id)
            .fetch_optional(&self.0.db)
            .await?;
        value
            .map(|s| serde_json::from_str(&s).context("invalid stored host"))
            .transpose()
    }
    /// Callers serialize host read-modify-write operations using `writes`.
    pub async fn save_host(&self, host: &Host) -> Result<()> {
        sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,name=excluded.name,body=excluded.body")
            .bind(&host.id).bind(&host.agent_id).bind(&host.name).bind(serde_json::to_string(host)?)
            .execute(&self.0.db).await?;
        Ok(())
    }
}
