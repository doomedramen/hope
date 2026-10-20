use crate::inventory::{SourceSettings, StoredHost};
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
    pub notification_delivery: Mutex<()>,
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
    pub background: crate::health::BackgroundHealth,
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
        capabilities: Capabilities,
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
            notification_delivery: Mutex::new(()),
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
            background: crate::health::BackgroundHealth::default(),
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
    pub async fn publish_host(&self, id: &str) {
        match self.host(id).await {
            Ok(Some(view)) => self.host_changed(&view),
            _ => self.changed(),
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
    pub async fn stored_hosts(&self) -> Result<Vec<StoredHost>> {
        let bodies: Vec<String> =
            sqlx::query_scalar("SELECT body FROM hosts ORDER BY name COLLATE NOCASE")
                .fetch_all(&self.0.db)
                .await?;
        bodies.iter().map(|body| StoredHost::decode(body)).collect()
    }
    pub async fn resolve_id(&self, id: &str) -> Result<String> {
        let mut current = id.to_owned();
        let mut seen = HashSet::new();
        while seen.insert(current.clone()) {
            let next: Option<String> = sqlx::query_scalar("SELECT survivor_id FROM device_id_aliases WHERE alias_id=? UNION ALL SELECT survivor_id FROM source_device_aliases WHERE alias_id=? LIMIT 1")
                .bind(&current).bind(&current).fetch_optional(&self.0.db).await?;
            match next {
                Some(next) => current = next,
                None => return Ok(current),
            }
        }
        anyhow::bail!("cyclic device alias")
    }
    pub async fn stored_host(&self, id: &str) -> Result<Option<StoredHost>> {
        let id = self.resolve_id(id).await?;
        let body: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE id=?")
            .bind(&id)
            .fetch_optional(&self.0.db)
            .await?;
        body.map(|body| StoredHost::decode(&body)).transpose()
    }
    /// Persist only user-editable fields, never the observation fields of a device view.
    pub async fn save_settings(&self, view: &Host) -> Result<()> {
        let settings = SourceSettings::from_view(view);
        if let Some(mut record) = self.stored_host(&view.id).await? {
            settings.apply(&mut record);
            self.save_host(&record).await?;
        } else {
            sqlx::query("INSERT INTO source_device_settings(source_device_id,body) VALUES(?,?) ON CONFLICT(source_device_id) DO UPDATE SET body=excluded.body")
                .bind(&view.id).bind(serde_json::to_string(&settings)?).execute(&self.0.db).await?;
        }
        Ok(())
    }
    pub async fn hosts(&self) -> Result<Vec<Host>> {
        self.device_views(None).await
    }
    async fn device_views(&self, id: Option<&str>) -> Result<Vec<Host>> {
        // All pieces of a view come from one SQLite snapshot during concurrent writes.
        let mut tx = self.0.db.begin().await?;
        let rows = sqlx::query("SELECT body FROM hosts WHERE ? IS NULL OR id=? OR id=(SELECT host_id FROM source_devices WHERE id=?) ORDER BY name COLLATE NOCASE")
            .bind(id).bind(id).bind(id).fetch_all(&mut *tx)
            .await?;
        let mut hosts: Vec<Host> = rows
            .into_iter()
            .map(|r| StoredHost::decode(r.get("body")).map(StoredHost::into_view))
            .collect::<Result<_>>()?;
        let retired_ids: HashSet<String> =
            sqlx::query_scalar("SELECT device_id FROM retired_devices")
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect();
        for host in &mut hosts {
            host.retired = retired_ids.contains(&host.id);
        }
        let networks =
            sqlx::query("SELECT id,name,cidr,enabled,interval_minutes FROM networks ORDER BY name")
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(|row| Network {
                    id: row.get("id"),
                    name: row.get("name"),
                    cidr: row.get("cidr"),
                    enabled: row.get("enabled"),
                    interval_minutes: row.get::<i64, _>("interval_minutes") as u32,
                })
                .collect::<Vec<_>>();
        let association_rows = sqlx::query("SELECT a.source_device_id,a.host_id,h.name,a.status,a.reason,a.evidence FROM device_associations a JOIN hosts h ON h.id=a.host_id WHERE ? IS NULL OR a.source_device_id=? OR a.host_id=? OR a.host_id=(SELECT host_id FROM source_devices WHERE id=?) ORDER BY CASE a.status WHEN 'confirmed' THEN 0 WHEN 'suggested' THEN 1 ELSE 2 END,h.name COLLATE NOCASE")
            .bind(id).bind(id).bind(id).bind(id).fetch_all(&mut *tx).await?;
        let mut associations = HashMap::<String, Vec<DeviceAssociation>>::new();
        for row in association_rows {
            associations
                .entry(row.get("source_device_id"))
                .or_default()
                .push(source_association(&row)?);
        }
        let children = sqlx::query("SELECT d.id,d.source,d.source_scope,d.source_key,d.incarnation,d.lifecycle,d.name,d.observed_at,d.body,d.host_id,(SELECT r.to_device_id FROM device_relationships r WHERE r.source_device_id=d.id AND r.kind='runs_on' ORDER BY r.observed_at DESC LIMIT 1) AS to_device_id FROM source_devices d WHERE NOT EXISTS(SELECT 1 FROM source_device_aliases a WHERE a.alias_id=d.id) AND (? IS NULL OR d.id=? OR d.host_id=? OR d.host_id=(SELECT host_id FROM source_devices WHERE id=?))")
            .bind(id).bind(id).bind(id).bind(id).fetch_all(&mut *tx).await?;
        let mut service_relationships = source_service_relationships(&mut tx, id).await?;
        let mut settings: HashMap<String, String> =
            sqlx::query_as("SELECT source_device_id,body FROM source_device_settings WHERE ? IS NULL OR source_device_id=? OR source_device_id IN (SELECT id FROM source_devices WHERE host_id=? OR host_id=(SELECT host_id FROM source_devices WHERE id=?))")
                .bind(id).bind(id).bind(id).bind(id).fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect();
        for row in children {
            let mut source = source_device_host(&row)?;
            let source_parent = row.try_get::<Option<String>, _>("host_id").ok().flatten();
            source.retired = retired_ids.contains(&source.id)
                || source_parent
                    .as_ref()
                    .is_some_and(|parent| retired_ids.contains(parent));
            if let Some(id) = &source.source_device_id {
                source
                    .relationships
                    .extend(service_relationships.remove(id).unwrap_or_default());
            }
            if let Some(settings) = settings.remove(&source.id) {
                serde_json::from_str::<SourceSettings>(&settings)?.apply(&mut source);
            }
            source.network_ids = source_networks(&networks, &source.addresses);
            source.associations = source
                .source_device_id
                .as_ref()
                .and_then(|id| associations.remove(id))
                .unwrap_or_default();
            let linked_target = source_parent
                .clone()
                .and_then(|target_id| hosts.iter().position(|host| host.id == target_id));
            if let Some(target_index) = linked_target {
                let target = &mut hosts[target_index];
                merge_source_host(target, source);
            } else {
                hosts.push(source);
            }
        }
        for host in &mut hosts {
            host.primary_ip = crate::addresses::primary_ip(&host.addresses, &networks);
        }
        hosts.sort_by_key(|host| host.name.to_lowercase());
        tx.commit().await?;
        Ok(hosts)
    }
    pub async fn host(&self, id: &str) -> Result<Option<Host>> {
        let id = self.resolve_id(id).await?;
        Ok(self
            .device_views(Some(&id))
            .await?
            .into_iter()
            .find(|host| host.id == id || host.source_device_id.as_deref() == Some(id.as_str())))
    }
    /// Callers serialize host read-modify-write operations using `writes`.
    /// Reserve SQLite's writer before reading state that will be changed in this transaction.
    /// Some independent single-statement writers (for example SSH progress) do not use `writes`.
    /// A deferred read transaction cannot safely upgrade after one of those writers commits.
    pub(crate) async fn write_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
        Ok(self.0.db.begin_with("BEGIN IMMEDIATE").await?)
    }

    /// A fresh scan observation revives the physical device and any child identities linked to it.
    pub(crate) async fn clear_retired_host(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
    ) -> Result<()> {
        sqlx::query("DELETE FROM retired_devices WHERE device_id=? OR device_id IN (SELECT id FROM source_devices WHERE host_id=?)")
            .bind(id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// A fresh child observation revives its source identity and its linked physical device.
    pub(crate) async fn clear_retired_source(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
    ) -> Result<()> {
        sqlx::query("DELETE FROM retired_devices WHERE device_id=? OR device_id=(SELECT host_id FROM source_devices WHERE id=?)")
            .bind(id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    pub async fn save_host(&self, host: &StoredHost) -> Result<()> {
        host.validate()?;
        let mut tx = self.write_transaction().await?;
        let old: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE id=?")
            .bind(&host.id)
            .fetch_optional(&mut *tx)
            .await?;
        let old = old.map(|body| StoredHost::decode(&body)).transpose()?;
        let observed_again = old
            .as_ref()
            .is_some_and(|previous| host.last_seen > previous.last_seen);
        let identity_changed = old.as_ref().is_none_or(|old| {
            old.addresses != host.addresses
                || old.macs != host.macs
                || old.name != host.name
                || (old.last_seen < now() - 7.0 * 86400.0
                    && host.last_seen >= now() - 7.0 * 86400.0)
        });
        sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,name=excluded.name,body=excluded.body")
            .bind(&host.id).bind(&host.agent_id).bind(&host.name).bind(serde_json::to_string(host)?)
            .execute(&mut *tx).await?;
        if observed_again {
            Self::clear_retired_host(&mut tx, &host.id).await?;
        }
        if identity_changed {
            crate::identity::reconcile(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn source_service_relationships(
    db: &mut sqlx::SqliteConnection,
    id: Option<&str>,
) -> Result<HashMap<String, Vec<DeviceRelationship>>> {
    let rows = sqlx::query("SELECT from_device_id,to_device_id,detail FROM device_relationships WHERE kind='exposes_service' AND (? IS NULL OR source_device_id=? OR source_device_id IN (SELECT id FROM source_devices WHERE host_id=? OR host_id=(SELECT host_id FROM source_devices WHERE id=?)))").bind(id).bind(id).bind(id).bind(id).fetch_all(&mut *db).await?;
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
    host.primary_ip = None;
    host.macs = child
        .interfaces
        .iter()
        .filter_map(|interface| interface.mac.clone())
        .collect();
    for mac in child
        .network_configuration
        .iter()
        .filter_map(|network| network.mac.clone())
    {
        if !host
            .macs
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&mac))
        {
            host.macs.push(mac);
        }
    }
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
    target.retired |= source.retired;
    if target.name_override.is_none() {
        target.name_override = source.name_override.clone();
    }
    if target.icon_override.is_none() {
        target.icon_override = source.icon_override.clone();
    }
    if target.device.is_none() {
        target.device = source.device.clone();
    }
    if !target.uptime.enabled {
        target.uptime = source.uptime.clone();
    }
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
