use anyhow::{Context, Result};
use hope_protocol::{
    AssociationStatus, Capabilities, ChildDeviceKind, ChildDeviceReport, DeviceAssociation,
    DeviceLifecycle, DeviceRelationship, Host, Network, SourceIdentity,
};
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

/// A change pushed to open dashboards over server-sent events.
#[derive(Clone, Debug)]
pub enum Change {
    /// Something changed; clients reload everything.
    Refresh,
    /// One host's latest state, serialized once for every subscriber.
    Host(Arc<str>),
}

pub struct Inner {
    pub db: SqlitePool,
    pub events: broadcast::Sender<Change>,
    /// Seconds between samples that agents are asked to report at.
    pub report_interval: std::sync::atomic::AtomicU32,
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
        let report_interval = sqlx::query_scalar::<_, String>(
            "SELECT value FROM meta WHERE key='report_interval_seconds'",
        )
        .fetch_optional(&db)
        .await?
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|seconds| hope_protocol::REPORT_INTERVALS_SECONDS.contains(seconds))
        .unwrap_or(hope_protocol::DEFAULT_REPORT_INTERVAL_SECONDS);
        let state = Self(Arc::new(Inner {
            db,
            events,
            report_interval: std::sync::atomic::AtomicU32::new(report_interval),
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
        for address in host.addresses.clone() {
            let Some(seen) = address.parse().ok().and_then(|ip| sightings.get(&ip)) else {
                continue;
            };
            let e = &mut host.evidence;
            e.mdns_name = seen.mdns_name.clone().or(e.mdns_name.take());
            // An address heard over mDNS this sweep replaces what it announced before, so services
            // once credited to it by mistake (a router reflecting other devices) fall away.
            if seen.mdns_seen {
                e.mdns_services = seen.mdns_services.iter().cloned().collect();
                e.mdns_model = seen.mdns_model.clone();
                let instances: Vec<String> = seen.mdns_instances.iter().cloned().collect();
                if (host.agent_id.is_none() || host.agent_uninstalled)
                    && e.mdns_instances.contains(&host.name)
                    && !instances.contains(&host.name)
                {
                    // The name came from an instance it no longer announces; let it be re-derived.
                    host.name = address.clone();
                }
                e.mdns_instances = instances;
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
        let _ = self.0.events.send(Change::Refresh);
    }
    /// Push one host's new state, so dashboards update it without reloading everything.
    pub fn host_changed(&self, host: &Host) {
        match serde_json::to_string(host) {
            Ok(body) => {
                let _ = self.0.events.send(Change::Host(body.into()));
            }
            Err(_) => self.changed(),
        }
    }
    pub fn report_interval(&self) -> u32 {
        self.0
            .report_interval
            .load(std::sync::atomic::Ordering::Relaxed)
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
        let mut hosts: Vec<Host> = rows
            .into_iter()
            .map(|r| serde_json::from_str(r.get("body")).context("invalid stored host"))
            .collect::<Result<_>>()?;
        let networks = crate::api::networks(self)
            .await
            .map_err(|error| anyhow::anyhow!(error.1))?;
        let association_rows = sqlx::query("SELECT a.source_device_id,a.host_id,h.name,a.status,a.reason,a.evidence FROM device_associations a JOIN hosts h ON h.id=a.host_id ORDER BY CASE a.status WHEN 'confirmed' THEN 0 WHEN 'suggested' THEN 1 ELSE 2 END,h.name COLLATE NOCASE")
            .fetch_all(&self.0.db).await?;
        let mut associations = HashMap::<String, Vec<DeviceAssociation>>::new();
        for row in association_rows {
            associations
                .entry(row.get("source_device_id"))
                .or_default()
                .push(source_association(&row)?);
        }
        let children = sqlx::query("SELECT d.id,d.source,d.source_scope,d.source_key,d.incarnation,d.lifecycle,d.name,d.observed_at,d.body,d.host_id,r.to_device_id FROM source_devices d LEFT JOIN device_relationships r ON r.source_device_id=d.id AND r.kind='runs_on'")
            .fetch_all(&self.0.db).await?;
        let mut service_relationships = source_service_relationships(&self.0.db, None).await?;
        for row in children {
            let mut source = source_device_host(&row)?;
            if let Some(id) = &source.source_device_id {
                source
                    .relationships
                    .extend(service_relationships.remove(id).unwrap_or_default());
            }
            source.network_ids = source_networks(&networks, &source.addresses);
            source.associations = source
                .source_device_id
                .as_ref()
                .and_then(|id| associations.remove(id))
                .unwrap_or_default();
            if let Some(target_id) = row.try_get::<Option<String>, _>("host_id").ok().flatten()
                && let Some(target) = hosts.iter_mut().find(|host| host.id == target_id)
            {
                merge_source_host(target, source);
            } else {
                hosts.push(source);
            }
        }
        hosts.sort_by_key(|host| host.name.to_lowercase());
        Ok(hosts)
    }
    pub async fn host(&self, id: &str) -> Result<Option<Host>> {
        let value: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE id=?")
            .bind(id)
            .fetch_optional(&self.0.db)
            .await?;
        if let Some(body) = value {
            return serde_json::from_str(&body)
                .context("invalid stored host")
                .map(Some);
        }
        let row = sqlx::query("SELECT d.id,d.source,d.source_scope,d.source_key,d.incarnation,d.lifecycle,d.name,d.observed_at,d.body,d.host_id,r.to_device_id FROM source_devices d LEFT JOIN device_relationships r ON r.source_device_id=d.id AND r.kind='runs_on' WHERE d.id=?")
            .bind(id).fetch_optional(&self.0.db).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut source = source_device_host(&row)?;
        source.relationships.extend(
            source_service_relationships(&self.0.db, Some(id))
                .await?
                .remove(id)
                .unwrap_or_default(),
        );
        let networks = crate::api::networks(self)
            .await
            .map_err(|error| anyhow::anyhow!(error.1))?;
        source.network_ids = source_networks(&networks, &source.addresses);
        source.associations = self.source_associations(&source.source_device_id).await?;
        Ok(Some(source))
    }
    async fn source_associations(
        &self,
        source_device_id: &Option<String>,
    ) -> Result<Vec<DeviceAssociation>> {
        let Some(source_device_id) = source_device_id else {
            return Ok(vec![]);
        };
        let rows = sqlx::query("SELECT a.host_id,h.name,a.status,a.reason,a.evidence FROM device_associations a JOIN hosts h ON h.id=a.host_id WHERE a.source_device_id=? ORDER BY CASE a.status WHEN 'confirmed' THEN 0 WHEN 'suggested' THEN 1 ELSE 2 END,h.name COLLATE NOCASE")
            .bind(source_device_id).fetch_all(&self.0.db).await?;
        rows.iter().map(source_association).collect()
    }
    /// Callers serialize host read-modify-write operations using `writes`.
    pub async fn save_host(&self, host: &Host) -> Result<()> {
        sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,name=excluded.name,body=excluded.body")
            .bind(&host.id).bind(&host.agent_id).bind(&host.name).bind(serde_json::to_string(host)?)
            .execute(&self.0.db).await?;
        Ok(())
    }
}

async fn source_service_relationships(
    db: &SqlitePool,
    source_device_id: Option<&str>,
) -> Result<HashMap<String, Vec<DeviceRelationship>>> {
    let rows = if let Some(id) = source_device_id {
        sqlx::query("SELECT from_device_id,to_device_id,detail FROM device_relationships WHERE kind='exposes_service' AND source_device_id=?")
            .bind(id).fetch_all(db).await?
    } else {
        sqlx::query("SELECT from_device_id,to_device_id,detail FROM device_relationships WHERE kind='exposes_service'")
            .fetch_all(db).await?
    };
    let mut relationships = HashMap::<String, Vec<DeviceRelationship>>::new();
    for row in rows {
        let from_device_id: String = row.get("from_device_id");
        relationships
            .entry(from_device_id.clone())
            .or_default()
            .push(DeviceRelationship {
                from_device_id,
                to_device_id: row.get("to_device_id"),
                kind: hope_protocol::RelationshipKind::ExposesService,
                detail: row.get("detail"),
            });
    }
    Ok(relationships)
}

fn source_device_host(row: &sqlx::sqlite::SqliteRow) -> Result<Host> {
    let id: String = row.get("id");
    let source: String = row.get("source");
    let scope: String = row.get("source_scope");
    let key: String = row.get("source_key");
    let incarnation: String = row.get("incarnation");
    let observed_at: f64 = row.get("observed_at");
    let mut child: ChildDeviceReport =
        serde_json::from_str(row.get("body")).context("invalid stored child device")?;
    let kind: ChildDeviceKind = child.identity.kind;
    let lifecycle = match row.get::<String, _>("lifecycle").as_str() {
        "running" => DeviceLifecycle::Running,
        "stopped" => DeviceLifecycle::Stopped,
        "archived" => DeviceLifecycle::Archived,
        _ => DeviceLifecycle::Stale,
    };
    child.lifecycle = lifecycle;
    let mut host = Host::new(id.clone(), row.get("name"), &source);
    host.last_seen = observed_at;
    host.last_collected_at = Some(observed_at);
    host.source_observed_at = Some(observed_at);
    host.source = Some(SourceIdentity {
        kind,
        scope,
        key,
        incarnation,
    });
    host.source_device_id = Some(id.clone());
    host.lifecycle = Some(lifecycle);
    host.discovery_method = source;
    host.addresses = child
        .interfaces
        .iter()
        .flat_map(|interface| interface.addresses.iter().cloned())
        .collect();
    host.macs = child
        .interfaces
        .iter()
        .filter_map(|interface| interface.mac.clone())
        .collect();
    host.network_ids.clear();
    host.parent_id = row.try_get("to_device_id").ok().flatten();
    host.relationships = host
        .parent_id
        .iter()
        .map(|parent| DeviceRelationship {
            from_device_id: id.clone(),
            to_device_id: parent.clone(),
            kind: hope_protocol::RelationshipKind::RunsOn,
            detail: None,
        })
        .collect();
    host.child = Some(child);
    Ok(host)
}

fn source_association(row: &sqlx::sqlite::SqliteRow) -> Result<DeviceAssociation> {
    let evidence: Vec<String> = serde_json::from_str(row.get("evidence"))?;
    Ok(DeviceAssociation {
        host_id: row.get("host_id"),
        host_name: row.get("name"),
        status: match row.get::<String, _>("status").as_str() {
            "confirmed" => AssociationStatus::Confirmed,
            "rejected" => AssociationStatus::Rejected,
            _ => AssociationStatus::Suggested,
        },
        reason: row.get("reason"),
        evidence,
    })
}

fn source_networks(networks: &[Network], addresses: &[String]) -> Vec<String> {
    networks
        .iter()
        .filter(|network| {
            network.cidr.parse::<ipnet::IpNet>().is_ok_and(|cidr| {
                addresses.iter().any(|address| {
                    address
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| cidr.contains(&address))
                })
            })
        })
        .map(|network| network.id.clone())
        .collect()
}

fn merge_source_host(target: &mut Host, source: Host) {
    for address in source.addresses {
        if !target.addresses.contains(&address) {
            target.addresses.push(address);
        }
    }
    for mac in source.macs {
        if !target
            .macs
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&mac))
        {
            target.macs.push(mac);
        }
    }
    for network in source.network_ids {
        if !target.network_ids.contains(&network) {
            target.network_ids.push(network);
        }
    }
    target.source = source.source;
    target.source_device_id = source.source_device_id;
    target.source_observed_at = source.source_observed_at;
    if target.last_collected_at.is_none() {
        target.last_collected_at = source.last_collected_at;
    }
    target.last_seen = target.last_seen.max(source.last_seen);
    target.lifecycle = source.lifecycle;
    target.child = source.child;
    target.associations = source.associations;
    for relationship in source.relationships {
        if !target.relationships.iter().any(|existing| {
            existing.from_device_id == relationship.from_device_id
                && existing.to_device_id == relationship.to_device_id
                && existing.kind == relationship.kind
        }) {
            target.relationships.push(relationship);
        }
    }
}
