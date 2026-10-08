use anyhow::{Context, Result};
use hope_protocol::{Capabilities, Host, InstallJob};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::HashMap,
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
    pub installs: Mutex<HashMap<String, InstallJob>>,
    pub server_url: String,
    pub secure_cookie: bool,
    pub capabilities: Capabilities,
    pub release_dir: String,
    pub enrollment_ca: Option<String>,
    pub scheduler: crate::scanner::Scheduler,
    pub login_attempts: Mutex<HashMap<String, (u32, f64)>>,
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
        mut capabilities: Capabilities,
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
        let (events, _) = broadcast::channel(256);
        let scheduler = crate::scanner::Scheduler::new();
        capabilities.arp = capabilities.arp && scheduler.arp.available();
        if !capabilities.arp && capabilities.limitations.is_empty() {
            capabilities.limitations.push(
                "Active ARP is unavailable on this interface; physical topology may be incomplete."
                    .into(),
            );
        }
        let state = Self(Arc::new(Inner {
            db,
            events,
            writes: Mutex::new(()),
            scans: Mutex::new(HashMap::new()),
            installs: Mutex::new(HashMap::new()),
            secure_cookie: server_url.starts_with("https://"),
            server_url,
            capabilities,
            release_dir,
            enrollment_ca: std::env::var("HOPE_AGENT_CA")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            scheduler,
            login_attempts: Mutex::new(HashMap::new()),
        }));
        Ok(state)
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
