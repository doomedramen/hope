//! Typed job-kind -> handler registry for the worker role. Unregistered
//! kinds are a configuration/deploy error (a job enqueued by a newer
//! server talking to an older worker, or a typo), not something retrying
//! could ever fix — the worker fails them permanently with a clear
//! message rather than retrying forever (see `jobs::fail_permanently`).

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::str::FromStr;

use async_trait::async_trait;
use domain::discovery::ApprovedScope;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent_updates;
use crate::agents;
use crate::config::{
    DEFAULT_AUDIT_EVENT_RETENTION_DAYS, DEFAULT_CHANGE_EVENT_RETENTION_DAYS,
    DEFAULT_JOB_RETENTION_DAYS, DEFAULT_MONITOR_ROLLUP_AFTER_DAYS,
    DEFAULT_MONITOR_ROLLUP_RETENTION_DAYS, DEFAULT_RAW_MONITOR_RETENTION_DAYS,
};
use crate::dependency_graph;
use crate::discovery::service_collectors::{self, CollectorConfig, CollectorProtocol};
use crate::discovery::worker;
use crate::inventory::retention;
use crate::inventory::service_collector_evidence;
use crate::maintenance;
use crate::notifications;
use crate::session_store::PgSessionStore;
use crate::{credentials::CredentialStore, ssh_install};

pub use worker::JobOutcome;

#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn handle(
        &self,
        pool: &PgPool,
        job_id: Uuid,
        worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome>;
}

/// Delete expired session rows (spec §12.3 secure sessions). Previously a
/// bespoke hourly `tokio::spawn` loop in `main.rs`; moved to a scheduled
/// job so it goes through the same retry/lease/observability machinery as
/// everything else the worker does.
struct SessionCleanup;

#[async_trait]
impl JobHandler for SessionCleanup {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        _payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let store = PgSessionStore::new(pool.clone());
        let deleted = store.delete_expired().await?;
        if deleted > 0 {
            tracing::info!(count = deleted, "deleted expired sessions");
        }
        Ok(JobOutcome::Completed)
    }
}

/// Delete enrollment tokens that are expired or already used (ADR-0007).
struct EnrollmentTokenPurge;

#[async_trait]
impl JobHandler for EnrollmentTokenPurge {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        _payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let deleted = agents::purge_expired_tokens(pool).await?;
        if deleted > 0 {
            tracing::info!(count = deleted, "purged expired/used enrollment tokens");
        }
        Ok(JobOutcome::Completed)
    }
}

struct AgentHealthSweep;

#[async_trait]
impl JobHandler for AgentHealthSweep {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let timeout_seconds = payload
            .get("timeout_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(agents::HEARTBEAT_TIMEOUT_SECONDS);
        let opened = agents::sweep_offline(pool, timeout_seconds).await?;
        if opened > 0 {
            tracing::info!(opened, timeout_seconds, "opened offline agent incidents");
        }
        Ok(JobOutcome::Completed)
    }
}

/// No-op handler for exercising the queue/worker pipeline end to end
/// (enqueue -> claim -> dispatch -> complete) without any real side
/// effect. Logs its payload so a test/operator can confirm it actually
/// ran.
struct DiagnosticEcho;

#[async_trait]
impl JobHandler for DiagnosticEcho {
    async fn handle(
        &self,
        _pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        tracing::info!(?payload, "diagnostic.echo");
        Ok(JobOutcome::Completed)
    }
}

/// `change_events` retention (design docs/design/m1-inventory.md §5,
/// Decision 5): delete rows older than the configured window in bounded
/// batches. `retention_days` comes from the enqueuing side's payload
/// (`{"retention_days": N}`), not this handler, so the window stays
/// operator-configurable (`HOPE_CHANGE_EVENT_RETENTION_DAYS`) without a
/// worker redeploy.
struct ChangeEventRetention;

#[async_trait]
impl JobHandler for ChangeEventRetention {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let retention_days = payload
            .get("retention_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_CHANGE_EVENT_RETENTION_DAYS);
        let retention_days =
            retention::validate_retention_days(retention_days, "change event retention")?;
        let deleted = retention::purge_old_change_events(pool, retention_days).await?;
        if deleted > 0 {
            tracing::info!(count = deleted, retention_days, "purged old change_events");
        }
        Ok(JobOutcome::Completed)
    }
}

/// Compact old monitor observations into hourly rollups, then delete raw rows
/// outside the configured history window. The handler keeps the two windows in
/// the job payload so operators can change them without rebuilding workers.
struct MonitorResultRetention;

#[async_trait]
impl JobHandler for MonitorResultRetention {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let rollup_after_days = payload
            .get("rollup_after_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_MONITOR_ROLLUP_AFTER_DAYS);
        let rollup_after_days =
            retention::validate_retention_days(rollup_after_days, "monitor result rollup age")?;
        let retention_days = payload
            .get("retention_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_RAW_MONITOR_RETENTION_DAYS);
        let retention_days =
            retention::validate_retention_days(retention_days, "monitor result retention")?;
        if retention_days <= rollup_after_days {
            anyhow::bail!("monitor result retention must exceed monitor result rollup age");
        }
        let rollup_retention_days = payload
            .get("rollup_retention_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_MONITOR_ROLLUP_RETENTION_DAYS);
        let rollup_retention_days = retention::validate_retention_days(
            rollup_retention_days,
            "monitor result rollup retention",
        )?;
        let rolled_up = retention::rollup_monitor_results(pool, rollup_after_days).await?;
        let deleted = retention::purge_old_monitor_results(pool, retention_days).await?;
        let rollups_deleted =
            retention::purge_old_monitor_rollups(pool, rollup_retention_days).await?;
        if rolled_up > 0 || deleted > 0 || rollups_deleted > 0 {
            tracing::info!(
                rolled_up,
                deleted,
                rollups_deleted,
                rollup_after_days,
                retention_days,
                rollup_retention_days,
                "compacted monitor results"
            );
        }
        Ok(JobOutcome::Completed)
    }
}

/// Delete old audit events while retaining records needed by open incidents
/// and active maintenance operations.
struct AuditEventRetention;

#[async_trait]
impl JobHandler for AuditEventRetention {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let retention_days = payload
            .get("retention_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_AUDIT_EVENT_RETENTION_DAYS);
        let retention_days =
            retention::validate_retention_days(retention_days, "audit event retention")?;
        let deleted = retention::purge_old_audit_events(pool, retention_days).await?;
        if deleted > 0 {
            tracing::info!(count = deleted, retention_days, "purged old audit_events");
        }
        Ok(JobOutcome::Completed)
    }
}

/// Delete old terminal job records. The retention query excludes pending and
/// running work and protects rows still needed by active operations.
struct JobRetention;

#[async_trait]
impl JobHandler for JobRetention {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let retention_days = payload
            .get("retention_days")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_JOB_RETENTION_DAYS);
        let retention_days = retention::validate_retention_days(retention_days, "job retention")?;
        let deleted = retention::purge_old_jobs(pool, retention_days).await?;
        if deleted > 0 {
            tracing::info!(count = deleted, retention_days, "purged old terminal jobs");
        }
        Ok(JobOutcome::Completed)
    }
}

struct NotificationDelivery;

#[async_trait]
impl JobHandler for NotificationDelivery {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let delivery_id = payload
            .get("delivery_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("notification payload has no delivery_id"))
            .and_then(|value| {
                Uuid::parse_str(value)
                    .map_err(|error| anyhow::anyhow!("invalid notification delivery_id: {error}"))
            })?;
        notifications::deliver(pool, delivery_id).await?;
        Ok(JobOutcome::Completed)
    }
}

struct MaintenanceNotificationDelivery;

#[async_trait]
impl JobHandler for MaintenanceNotificationDelivery {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let delivery_id = payload
            .get("delivery_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("maintenance notification payload has no delivery_id"))
            .and_then(|value| {
                Uuid::parse_str(value)
                    .map_err(|error| anyhow::anyhow!("invalid maintenance delivery_id: {error}"))
            })?;
        notifications::deliver_maintenance(pool, delivery_id).await?;
        Ok(JobOutcome::Completed)
    }
}

struct FullTcpDiscovery;

#[async_trait]
impl JobHandler for FullTcpDiscovery {
    async fn handle(
        &self,
        pool: &PgPool,
        job_id: Uuid,
        worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        worker::handle(pool, job_id, worker_id, payload).await
    }
}

struct ServiceCollector;

#[async_trait]
impl JobHandler for ServiceCollector {
    async fn handle(
        &self,
        pool: &PgPool,
        job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let network_id = payload
            .get("network_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("service collector payload has no network_id"))
            .and_then(|value| {
                Uuid::parse_str(value).map_err(|error| {
                    anyhow::anyhow!("invalid service collector network_id: {error}")
                })
            })?;
        let protocol = payload
            .get("protocol")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("service collector payload has no protocol"))?;
        let protocol = match protocol {
            "mdns" => CollectorProtocol::Mdns,
            "ssdp" => CollectorProtocol::Ssdp,
            other => anyhow::bail!("unsupported service collector protocol `{other}`"),
        };
        let interface = payload
            .get("interface")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("service collector payload has no interface"))
            .and_then(|value| {
                Ipv4Addr::from_str(value).map_err(|error| {
                    anyhow::anyhow!("invalid service collector interface: {error}")
                })
            })?;

        validate_collector_scope(pool, network_id, interface).await?;
        let observations =
            service_collectors::collect_multicast(protocol, interface, CollectorConfig::default())
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
        let outcome = service_collector_evidence::persist_collected_observations(
            pool,
            &job_id.to_string(),
            &observations,
        )
        .await?;
        tracing::info!(
            %job_id,
            %network_id,
            observations = outcome.observations,
            evidence_rows_added = outcome.evidence_rows_added,
            unmatched = outcome.unmatched,
            "service collector job completed"
        );
        Ok(JobOutcome::Completed)
    }
}

struct AgentDeployment;

#[async_trait]
impl JobHandler for AgentDeployment {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let host = payload
            .get("host")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("agent deployment payload has no host"))?;
        let port = payload
            .get("port")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("agent deployment payload has no port"))?;
        let credential_id = payload
            .get("credential_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("agent deployment payload has no credential_id"))
            .and_then(|value| {
                Uuid::parse_str(value).map_err(|error| {
                    anyhow::anyhow!("invalid agent deployment credential_id: {error}")
                })
            })?;
        let device_id = payload
            .get("device_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("agent deployment payload has no device_id"))
            .and_then(|value| {
                Uuid::parse_str(value)
                    .map_err(|error| anyhow::anyhow!("invalid agent deployment device_id: {error}"))
            })?;
        let actor_user_id = payload
            .get("actor_user_id")
            .and_then(Value::as_str)
            .map(Uuid::parse_str)
            .transpose()?;
        let repair = payload
            .get("repair")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let disassociate_after_enrollment = payload
            .get("disassociate_after_enrollment")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let store = CredentialStore::from_environment(pool.clone())?;
        let result = ssh_install::install_or_repair(
            pool,
            &store,
            ssh_install::DeploymentRequest {
                device_id,
                host: host.to_string(),
                port: i32::try_from(port).map_err(|_| anyhow::anyhow!("invalid SSH port"))?,
                credential_id,
                actor_user_id,
                repair,
                disassociate_after_enrollment,
            },
        )
        .await?;
        tracing::info!(
            host = %result.host,
            port = result.port,
            platform = %result.platform,
            architecture = %result.architecture,
            enrolled = result.enrolled,
            repaired = result.repaired,
            "agent deployment completed"
        );
        Ok(JobOutcome::Completed)
    }
}

struct AgentUpdate;

#[async_trait]
impl JobHandler for AgentUpdate {
    async fn handle(
        &self,
        pool: &PgPool,
        job_id: Uuid,
        worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        agent_updates::handle_update(pool, job_id, worker_id, payload).await
    }
}

struct AgentUpdateReconcile;

#[async_trait]
impl JobHandler for AgentUpdateReconcile {
    async fn handle(
        &self,
        pool: &PgPool,
        job_id: Uuid,
        worker_id: &str,
        _payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        agent_updates::reconcile(pool, job_id, worker_id).await
    }
}

struct DependencyGraphReconcile;

#[async_trait]
impl JobHandler for DependencyGraphReconcile {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        _payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        dependency_graph::reconcile_deterministic_edges(pool).await?;
        Ok(JobOutcome::Completed)
    }
}

struct MaintenanceReconcile;

#[async_trait]
impl JobHandler for MaintenanceReconcile {
    async fn handle(
        &self,
        pool: &PgPool,
        _job_id: Uuid,
        _worker_id: &str,
        _payload: Value,
    ) -> anyhow::Result<JobOutcome> {
        let changed = maintenance::reconcile(pool).await?;
        if changed > 0 {
            tracing::info!(changed, "reconciled maintenance events");
        }
        Ok(JobOutcome::Completed)
    }
}

async fn validate_collector_scope(
    pool: &PgPool,
    network_id: Uuid,
    interface: Ipv4Addr,
) -> anyhow::Result<()> {
    let scope: Option<(String, bool, bool, Value)> = sqlx::query_as(
        "select n.cidr::text, coalesce(ds.enabled, false), \
                (ds.confirmed_at is not null), coalesce(ds.excluded_cidrs, '[]'::jsonb) \
         from networks n \
         left join discovery_scopes ds on ds.network_id = n.id \
         where n.id = $1",
    )
    .bind(network_id)
    .fetch_optional(pool)
    .await?;
    let Some((cidr, enabled, confirmed, excluded_cidrs)) = scope else {
        anyhow::bail!("network {network_id} not found");
    };
    if !enabled || !confirmed {
        anyhow::bail!("network {network_id} has no enabled, confirmed discovery scope");
    }
    let excluded_cidrs: Vec<String> = serde_json::from_value(excluded_cidrs)?;
    let scope = ApprovedScope::parse(&cidr, &excluded_cidrs)?;
    if !scope.cidr().contains(&interface)
        || scope
            .exclusions()
            .iter()
            .any(|excluded| excluded.contains(&interface))
    {
        anyhow::bail!("collector interface is outside the confirmed discovery scope");
    }
    Ok(())
}

pub struct Registry(HashMap<&'static str, Box<dyn JobHandler>>);

impl Registry {
    pub fn new() -> Self {
        let mut handlers: HashMap<&'static str, Box<dyn JobHandler>> = HashMap::new();
        handlers.insert("session.cleanup", Box::new(SessionCleanup));
        handlers.insert("enrollment_token.purge", Box::new(EnrollmentTokenPurge));
        handlers.insert("agent_health.sweep", Box::new(AgentHealthSweep));
        handlers.insert("agent.update", Box::new(AgentUpdate));
        handlers.insert("agent_updates.reconcile", Box::new(AgentUpdateReconcile));
        handlers.insert(
            "dependency_graph.reconcile",
            Box::new(DependencyGraphReconcile),
        );
        handlers.insert(
            maintenance::MAINTENANCE_RECONCILE_JOB_TYPE,
            Box::new(MaintenanceReconcile),
        );
        handlers.insert("diagnostic.echo", Box::new(DiagnosticEcho));
        handlers.insert("change_events.retention", Box::new(ChangeEventRetention));
        handlers.insert(
            "monitor_results.retention",
            Box::new(MonitorResultRetention),
        );
        handlers.insert("audit_events.retention", Box::new(AuditEventRetention));
        handlers.insert("jobs.retention", Box::new(JobRetention));
        handlers.insert("notifications.deliver", Box::new(NotificationDelivery));
        handlers.insert(
            "notifications.deliver_maintenance",
            Box::new(MaintenanceNotificationDelivery),
        );
        handlers.insert("discovery.full_tcp", Box::new(FullTcpDiscovery));
        handlers.insert("discovery.service_collectors", Box::new(ServiceCollector));
        handlers.insert("agent.install", Box::new(AgentDeployment));
        handlers.insert("agent.repair", Box::new(AgentDeployment));
        Self(handlers)
    }

    pub fn get(&self, job_type: &str) -> Option<&dyn JobHandler> {
        self.0.get(job_type).map(|handler| handler.as_ref())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn registry_dispatches_known_kinds_and_rejects_unknown() {
        let registry = Registry::new();
        assert!(registry.get("session.cleanup").is_some());
        assert!(registry.get("enrollment_token.purge").is_some());
        assert!(registry.get("agent_health.sweep").is_some());
        assert!(registry.get("diagnostic.echo").is_some());
        assert!(registry.get("change_events.retention").is_some());
        assert!(registry.get("monitor_results.retention").is_some());
        assert!(registry.get("audit_events.retention").is_some());
        assert!(registry.get("jobs.retention").is_some());
        assert!(registry.get("notifications.deliver").is_some());
        assert!(registry.get("notifications.deliver_maintenance").is_some());
        assert!(registry.get("discovery.full_tcp").is_some());
        assert!(registry.get("discovery.service_collectors").is_some());
        assert!(registry.get("agent.install").is_some());
        assert!(registry.get("agent.repair").is_some());
        assert!(registry.get("no.such.kind").is_none());
    }

    #[tokio::test]
    async fn diagnostic_echo_always_succeeds() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let registry = Registry::new();
        let handler = registry.get("diagnostic.echo").unwrap();
        handler
            .handle(
                &pool,
                Uuid::new_v4(),
                "test-worker",
                serde_json::json!({"hello": "world"}),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn session_cleanup_handler_deletes_expired_sessions() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        sqlx::query(
            "insert into sessions (id, data, expiry_date) values ($1, '{}'::jsonb, now() - interval '1 hour')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let registry = Registry::new();
        let handler = registry.get("session.cleanup").unwrap();
        handler
            .handle(&pool, Uuid::new_v4(), "test-worker", serde_json::json!({}))
            .await
            .unwrap();

        let remaining: (i64,) =
            sqlx::query_as("select count(*) from sessions where expiry_date < now()")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }

    #[tokio::test]
    async fn enrollment_token_purge_handler_deletes_expired_and_used_tokens() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        sqlx::query(
            "insert into enrollment_tokens (token_hash, expires_at) values ($1, now() - interval '1 hour')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let registry = Registry::new();
        let handler = registry.get("enrollment_token.purge").unwrap();
        handler
            .handle(&pool, Uuid::new_v4(), "test-worker", serde_json::json!({}))
            .await
            .unwrap();

        let remaining: (i64,) =
            sqlx::query_as("select count(*) from enrollment_tokens where expires_at < now()")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining.0, 0);
    }
}
