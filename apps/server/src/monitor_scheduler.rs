//! In-memory monitor scheduler and transactional result persistence.
//!
//! Monitor executions deliberately bypass the general job queue (ADR-0005).
//! PostgreSQL remains authoritative for due times, leases, results, health
//! state, and incidents; this module only holds the bounded in-memory batch
//! of checks currently executing in one worker process.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use domain::monitoring::{
    DEFAULT_STALE_AFTER, HealthConfig, HealthEventKind, HealthObservation, HealthSnapshot,
    HealthState, HealthStateMachine,
};
use jiff::Timestamp;
use serde_json::{Value, json};
use sqlx::FromRow;
use sqlx::PgPool;
use sqlx::types::time::OffsetDateTime;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::inventory::events::Recorder;
use crate::monitor_checks::{self, CheckOutcome, CheckProtocol, CheckRequest, CheckStatus};

const CLAIM_LEASE_SECONDS: f64 = 90.0;
const MAX_BATCH: usize = 32;
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const STALE_AFTER: Duration = DEFAULT_STALE_AFTER;

#[derive(Debug, FromRow)]
struct ClaimedMonitor {
    id: Uuid,
    monitor_type: String,
    config: Value,
    interval_seconds: i32,
    timeout_ms: i32,
    failure_threshold: i32,
    recovery_threshold: i32,
    state: String,
    underlying_state: String,
    consecutive_failures: i32,
    consecutive_successes: i32,
    last_result_at: Option<OffsetDateTime>,
    last_success_at: Option<OffsetDateTime>,
    last_failure_at: Option<OffsetDateTime>,
    address: Option<String>,
    port: Option<i32>,
    endpoint_type: String,
}

#[derive(Debug, FromRow)]
struct StaleMonitor {
    id: Uuid,
    state: String,
    underlying_state: String,
    failure_threshold: i32,
    recovery_threshold: i32,
    consecutive_failures: i32,
    consecutive_successes: i32,
    last_result_at: Option<OffsetDateTime>,
    last_success_at: Option<OffsetDateTime>,
}

/// Run the monitor loop until the worker's cooperative shutdown signal fires.
pub async fn run(
    pool: PgPool,
    owner: String,
    shutdown_requested: Arc<AtomicBool>,
    shutdown: Arc<Notify>,
) {
    loop {
        if shutdown_requested.load(Ordering::SeqCst) {
            break;
        }

        if let Err(error) = mark_stale_monitors(&pool).await {
            tracing::warn!(error = %error, "monitor stale sweep failed");
        }

        let mut tasks = JoinSet::new();
        for _ in 0..MAX_BATCH {
            let Some(monitor) = (match claim_due(&pool, &owner).await {
                Ok(monitor) => monitor,
                Err(error) => {
                    tracing::warn!(error = %error, "monitor claim failed");
                    break;
                }
            }) else {
                break;
            };

            let task_pool = pool.clone();
            let task_owner = owner.clone();
            tasks.spawn(async move {
                if let Err(error) = execute_one(&task_pool, &task_owner, monitor).await {
                    tracing::warn!(error = %error, "monitor execution failed");
                }
            });
        }

        if tasks.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = shutdown.notified() => break,
            }
        } else {
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    tracing::warn!(error = %error, "monitor task panicked");
                }
            }
        }
    }
}

async fn claim_due(pool: &PgPool, owner: &str) -> sqlx::Result<Option<ClaimedMonitor>> {
    sqlx::query_as(
        "with candidate as ( \
             select m.id from monitors m \
             where m.enabled \
               and (m.next_run_at is null or m.next_run_at <= now()) \
               and (m.lease_expires_at is null or m.lease_expires_at <= now()) \
             order by m.next_run_at nulls first, m.id \
             for update skip locked limit 1 \
         ), claimed as ( \
             update monitors m \
             set lease_owner = $1, \
                 lease_expires_at = now() + make_interval(secs => $2), \
                 updated_at = now(), version = version + 1 \
             from candidate c where m.id = c.id \
             returning m.id \
         ) \
         select m.id, m.monitor_type, m.config, m.interval_seconds, m.timeout_ms, \
                m.failure_threshold, m.recovery_threshold, m.state, m.underlying_state, \
                m.consecutive_failures, m.consecutive_successes, m.last_result_at, \
                m.last_success_at, m.last_failure_at, e.address::text as address, e.port, \
                e.endpoint_type \
         from claimed c \
         join monitors m on m.id = c.id \
         join endpoints e on e.id = m.endpoint_id",
    )
    .bind(owner)
    .bind(CLAIM_LEASE_SECONDS)
    .fetch_optional(pool)
    .await
}

async fn execute_one(pool: &PgPool, owner: &str, monitor: ClaimedMonitor) -> Result<()> {
    let outcome = match build_request(&monitor) {
        Ok(request) => monitor_checks::run(request).await,
        Err(error) => CheckOutcome {
            status: CheckStatus::Error,
            latency_ms: 0,
            error: Some(error.to_string()),
            details: json!({
                "protocol": monitor.monitor_type.clone(),
                "endpoint_type": monitor.endpoint_type,
            }),
        },
    };
    persist_outcome(pool, owner, &monitor, outcome).await
}

fn build_request(monitor: &ClaimedMonitor) -> Result<CheckRequest> {
    let address = monitor
        .address
        .as_deref()
        .ok_or_else(|| anyhow!("monitor endpoint has no address"))?
        .split('/')
        .next()
        .unwrap_or_default()
        .parse::<IpAddr>()
        .context("monitor endpoint address is invalid")?;
    let port = monitor
        .port
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| anyhow!("monitor endpoint port is invalid"))?;
    let protocol = CheckProtocol::parse(&monitor.monitor_type)?;
    let object = monitor.config.as_object();
    let method = object
        .and_then(|config| config.get("method"))
        .and_then(Value::as_str)
        .unwrap_or("GET");
    if method != "GET" {
        anyhow::bail!("monitor HTTP method `{method}` is not supported");
    }
    let path = object
        .and_then(|config| config.get("path"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let host = object
        .and_then(|config| config.get("host"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let expected_status = object
        .and_then(|config| config.get("expected_status"))
        .and_then(Value::as_u64)
        .map(|status| u16::try_from(status).context("expected status is out of range"))
        .transpose()?;
    let body_contains = object
        .and_then(|config| config.get("body_contains"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let timeout_ms = u64::try_from(monitor.timeout_ms).context("monitor timeout is invalid")?;

    Ok(CheckRequest {
        protocol,
        address,
        port,
        path,
        host,
        expected_status,
        body_contains,
        timeout: Duration::from_millis(timeout_ms),
    })
}

async fn persist_outcome(
    pool: &PgPool,
    owner: &str,
    monitor: &ClaimedMonitor,
    outcome: CheckOutcome,
) -> Result<()> {
    let observed_at = OffsetDateTime::now_utc();
    let observed_timestamp = to_jiff(Some(observed_at))?.expect("present observed timestamp");
    let config = HealthConfig::new(
        u32::try_from(monitor.failure_threshold).context("failure threshold is invalid")?,
        u32::try_from(monitor.recovery_threshold).context("recovery threshold is invalid")?,
        STALE_AFTER,
    )?;
    let snapshot = HealthSnapshot {
        underlying_state: parse_underlying_state(&monitor.underlying_state)?,
        consecutive_failures: u32::try_from(monitor.consecutive_failures)
            .context("failure counter is invalid")?,
        consecutive_successes: u32::try_from(monitor.consecutive_successes)
            .context("success counter is invalid")?,
        last_observed_at: to_jiff(monitor.last_result_at)?,
        last_success_at: to_jiff(monitor.last_success_at)?,
    };
    let mut machine = HealthStateMachine::restore(config, snapshot)?;
    let observation = if outcome.status == CheckStatus::Success {
        HealthObservation::Success
    } else {
        HealthObservation::Failure
    };
    let update = machine.observe(observed_timestamp, observation);
    let last_success_at = from_jiff(update.last_success_at)?;
    let last_failure_at = if observation == HealthObservation::Failure {
        Some(observed_at)
    } else {
        monitor.last_failure_at
    };
    let next_delay = next_delay_seconds(monitor.interval_seconds, monitor.id);

    let mut tx = pool.begin().await?;
    let result_id: Uuid = sqlx::query_scalar(
        "insert into monitor_results \
            (monitor_id, status, observed_at, latency_ms, error, details) \
         values ($1, $2, $3, $4, $5, $6) returning id",
    )
    .bind(monitor.id)
    .bind(outcome.status.as_str())
    .bind(observed_at)
    .bind(outcome.latency_ms)
    .bind(outcome.error.as_deref())
    .bind(&outcome.details)
    .fetch_one(&mut *tx)
    .await?;

    let updated = sqlx::query(
        "update monitors set state = $3, underlying_state = $4, \
            consecutive_failures = $5, consecutive_successes = $6, \
            last_result_at = $7, last_success_at = $8, last_failure_at = $9, \
            next_run_at = now() + make_interval(secs => $10), \
            lease_owner = null, lease_expires_at = null, updated_at = now(), \
            version = version + 1 \
         where id = $1 and lease_owner = $2",
    )
    .bind(monitor.id)
    .bind(owner)
    .bind(update.state.as_str())
    .bind(update.underlying_state.as_str())
    .bind(i32::try_from(update.consecutive_failures).context("failure count overflow")?)
    .bind(i32::try_from(update.consecutive_successes).context("success count overflow")?)
    .bind(observed_at)
    .bind(last_success_at)
    .bind(last_failure_at)
    .bind(next_delay)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        anyhow::bail!("monitor lease was lost before result persistence");
    }

    let open_incident: Option<(Uuid, i32)> = sqlx::query_as(
        "select id, failure_count from incidents \
         where monitor_id = $1 and state = 'open' for update",
    )
    .bind(monitor.id)
    .fetch_optional(&mut *tx)
    .await?;
    if update.underlying_state == HealthState::Down {
        let failure_count = i32::try_from(update.consecutive_failures).unwrap_or(i32::MAX);
        if let Some((incident_id, _)) = open_incident {
            sqlx::query(
                "update incidents set last_event_at = $2, failure_count = $3, \
                    last_result_id = $4, updated_at = now() where id = $1",
            )
            .bind(incident_id)
            .bind(observed_at)
            .bind(failure_count.max(1))
            .bind(result_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "insert into incidents \
                    (monitor_id, state, severity, opened_at, last_event_at, \
                     failure_count, last_result_id, summary) \
                 values ($1, 'open', 'critical', $2, $2, $3, $4, $5)",
            )
            .bind(monitor.id)
            .bind(observed_at)
            .bind(failure_count.max(1))
            .bind(result_id)
            .bind(outcome.error.as_deref().unwrap_or("monitor check failed"))
            .execute(&mut *tx)
            .await?;
        }
    } else if let Some((incident_id, _)) = open_incident {
        sqlx::query(
            "update incidents set state = 'recovered', recovered_at = $2, \
                last_event_at = $2, last_result_id = $3, updated_at = now() \
             where id = $1",
        )
        .bind(incident_id)
        .bind(observed_at)
        .bind(result_id)
        .execute(&mut *tx)
        .await?;
    }

    for event in &update.events {
        let (category, severity) = match event.kind {
            HealthEventKind::IncidentOpened => ("incident.opened", "critical"),
            HealthEventKind::IncidentRecovered => ("incident.recovered", "notice"),
            HealthEventKind::StaleStarted => ("monitor.stale", "warning"),
            HealthEventKind::StaleCleared => ("monitor.stale_cleared", "notice"),
            HealthEventKind::StateChanged => ("monitor.state_changed", "notice"),
        };
        Recorder::record_change(
            &mut tx,
            "monitors",
            monitor.id,
            category,
            severity,
            Some(json!({
                "state": monitor.state,
                "underlying_state": monitor.underlying_state,
                "consecutive_failures": monitor.consecutive_failures,
                "consecutive_successes": monitor.consecutive_successes,
            })),
            Some(json!({
                "state": update.state.as_str(),
                "underlying_state": update.underlying_state.as_str(),
                "consecutive_failures": update.consecutive_failures,
                "consecutive_successes": update.consecutive_successes,
                "result_id": result_id,
            })),
            Some("monitor"),
        )
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn mark_stale_monitors(pool: &PgPool) -> Result<()> {
    let candidates: Vec<StaleMonitor> = sqlx::query_as(
        "select id, state, underlying_state, failure_threshold, recovery_threshold, \
                consecutive_failures, consecutive_successes, last_result_at, last_success_at \
         from monitors \
         where enabled and state <> 'stale' and last_result_at is not null \
           and last_result_at < now() - make_interval(secs => $1) \
           and (lease_expires_at is null or lease_expires_at <= now()) \
         order by last_result_at limit $2",
    )
    .bind(STALE_AFTER.as_secs() as f64)
    .bind(MAX_BATCH as i64)
    .fetch_all(pool)
    .await?;

    for candidate in candidates {
        let monitor_id = candidate.id;
        if let Err(error) = mark_one_stale(pool, candidate).await {
            tracing::warn!(%monitor_id, error = %error, "failed to mark monitor stale");
        }
    }
    Ok(())
}

async fn mark_one_stale(pool: &PgPool, candidate: StaleMonitor) -> Result<()> {
    let config = HealthConfig::new(
        u32::try_from(candidate.failure_threshold)?,
        u32::try_from(candidate.recovery_threshold)?,
        STALE_AFTER,
    )?;
    let snapshot = HealthSnapshot {
        underlying_state: parse_underlying_state(&candidate.underlying_state)?,
        consecutive_failures: u32::try_from(candidate.consecutive_failures)?,
        consecutive_successes: u32::try_from(candidate.consecutive_successes)?,
        last_observed_at: to_jiff(candidate.last_result_at)?,
        last_success_at: to_jiff(candidate.last_success_at)?,
    };
    let mut machine = HealthStateMachine::restore(config, snapshot)?;
    let now = OffsetDateTime::now_utc();
    let update = machine.evaluate(to_jiff(Some(now))?.expect("present current timestamp"));
    if update.state != HealthState::Stale {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "update monitors set state = 'stale', version = version + 1, updated_at = now() \
         where id = $1 and state <> 'stale' \
           and (lease_expires_at is null or lease_expires_at <= now())",
    )
    .bind(candidate.id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 1 {
        Recorder::record_change(
            &mut tx,
            "monitors",
            candidate.id,
            "monitor.stale",
            "warning",
            Some(json!({
                "state": candidate.state,
                "underlying_state": candidate.underlying_state,
            })),
            Some(json!({"state": "stale", "underlying_state": candidate.underlying_state})),
            Some("monitor"),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn parse_underlying_state(value: &str) -> Result<HealthState> {
    match value {
        "unknown" => Ok(HealthState::Unknown),
        "up" => Ok(HealthState::Up),
        "degraded" => Ok(HealthState::Degraded),
        "down" => Ok(HealthState::Down),
        other => Err(anyhow!("invalid persisted monitor state `{other}`")),
    }
}

fn to_jiff(value: Option<OffsetDateTime>) -> Result<Option<Timestamp>> {
    value
        .map(|value| {
            Timestamp::from_nanosecond(value.unix_timestamp_nanos())
                .map_err(|error| anyhow!("invalid persisted monitor timestamp: {error}"))
        })
        .transpose()
}

fn from_jiff(value: Option<Timestamp>) -> Result<Option<OffsetDateTime>> {
    value
        .map(|value| {
            OffsetDateTime::from_unix_timestamp_nanos(value.as_nanosecond())
                .map_err(|error| anyhow!("invalid monitor timestamp: {error}"))
        })
        .transpose()
}

fn next_delay_seconds(interval_seconds: i32, monitor_id: Uuid) -> f64 {
    let bytes = monitor_id.as_bytes();
    let sample = u16::from_be_bytes([bytes[0], bytes[1]]) as f64 / u16::MAX as f64;
    let jitter = (sample * 2.0 - 1.0) * (f64::from(interval_seconds) * 0.1);
    (f64::from(interval_seconds) + jitter).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn jitter_is_bounded_and_deterministic() {
        let id = Uuid::from_bytes([0; 16]);
        let first = next_delay_seconds(30, id);
        assert_eq!(first, next_delay_seconds(30, id));
        assert!((27.0..=33.0).contains(&first));
    }

    #[test]
    fn timestamp_conversion_round_trips() {
        let value = OffsetDateTime::now_utc();
        let round_trip = from_jiff(to_jiff(Some(value)).unwrap()).unwrap().unwrap();
        assert_eq!(
            round_trip.unix_timestamp_nanos(),
            value.unix_timestamp_nanos()
        );
    }

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to test database");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    async fn create_monitor(pool: &PgPool, port: u16, failure_threshold: i32) -> Uuid {
        let device_id: Uuid =
            sqlx::query_scalar("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(pool)
                .await
                .expect("create monitor device");
        let service_id: Uuid = sqlx::query_scalar(
            "insert into services (protocol, owner_kind, owner_id) \
             values ('http', 'device', $1) returning id",
        )
        .bind(device_id)
        .fetch_one(pool)
        .await
        .expect("create monitor service");
        let endpoint_id: Uuid = sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '127.0.0.1'::inet, $2) returning id",
        )
        .bind(service_id)
        .bind(i32::from(port))
        .fetch_one(pool)
        .await
        .expect("create monitor endpoint");
        sqlx::query_scalar(
            "insert into monitors \
                (service_id, endpoint_id, monitor_type, config, interval_seconds, timeout_ms, \
                 failure_threshold, recovery_threshold, next_run_at) \
             values ($1, $2, 'http', $3, 30, 500, $4, 2, now()) returning id",
        )
        .bind(service_id)
        .bind(endpoint_id)
        .bind(json!({
            "method": "GET",
            "path": "/health",
            "expected_status": 200,
            "body_contains": "ok",
        }))
        .bind(failure_threshold)
        .fetch_one(pool)
        .await
        .expect("create monitor")
    }

    async fn isolate_due_monitors(pool: &PgPool) {
        sqlx::query(
            "update monitors set next_run_at = now() + interval '1 hour', \
             lease_owner = null, lease_expires_at = null",
        )
        .execute(pool)
        .await
        .expect("isolate due monitors");
    }

    #[tokio::test]
    async fn claimed_http_monitor_persists_success() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        isolate_due_monitors(&pool).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let monitor_id = create_monitor(&pool, port, 2).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 512];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let claimed =
            tokio::time::timeout(Duration::from_secs(2), claim_due(&pool, "monitor-test"))
                .await
                .expect("claim timed out")
                .unwrap()
                .expect("monitor is due");
        assert_eq!(claimed.id, monitor_id);
        tokio::time::timeout(
            Duration::from_secs(2),
            execute_one(&pool, "monitor-test", claimed),
        )
        .await
        .expect("execute timed out")
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("test server timed out")
            .unwrap();

        let row: (String, String, i32, i32, i64) = sqlx::query_as(
            "select state, underlying_state, consecutive_failures, consecutive_successes, \
                    (select count(*) from monitor_results where monitor_id = $1) \
             from monitors where id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "up");
        assert_eq!(row.1, "up");
        assert_eq!(row.2, 0);
        assert_eq!(row.3, 1);
        assert_eq!(row.4, 1);
    }

    #[tokio::test]
    async fn failures_below_threshold_do_not_open_then_open_one_incident() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        isolate_due_monitors(&pool).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let monitor_id = create_monitor(&pool, port, 2).await;

        for attempt in 0..2 {
            let claimed = claim_due(&pool, "monitor-test")
                .await
                .unwrap()
                .expect("monitor is due");
            execute_one(&pool, "monitor-test", claimed).await.unwrap();
            if attempt == 0 {
                sqlx::query("update monitors set next_run_at = now() where id = $1")
                    .bind(monitor_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }

        let row: (String, String, i32, i64, i64) = sqlx::query_as(
            "select state, underlying_state, consecutive_failures, \
                    (select count(*) from incidents where monitor_id = $1 and state = 'open'), \
                    (select count(*) from monitor_results where monitor_id = $1) \
             from monitors where id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "down");
        assert_eq!(row.1, "down");
        assert_eq!(row.2, 2);
        assert_eq!(row.3, 1);
        assert_eq!(row.4, 2);
    }

    #[tokio::test]
    async fn stale_sweep_preserves_underlying_state() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        isolate_due_monitors(&pool).await;
        let monitor_id = create_monitor(&pool, 1, 2).await;
        sqlx::query(
            "update monitors set state = 'up', underlying_state = 'up', \
                consecutive_failures = 0, consecutive_successes = 1, \
                last_result_at = now() - interval '10 minutes', \
                last_success_at = now() - interval '10 minutes', next_run_at = now()",
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .unwrap();

        mark_stale_monitors(&pool).await.unwrap();

        let row: (String, String, i64) = sqlx::query_as(
            "select state, underlying_state, \
                    (select count(*) from change_events where entity_kind = 'monitors' \
                     and entity_id = $1 and category = 'monitor.stale') \
             from monitors where id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "stale");
        assert_eq!(row.1, "up");
        assert_eq!(row.2, 1);
    }
}
