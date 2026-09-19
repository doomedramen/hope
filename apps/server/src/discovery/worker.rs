//! Worker coordinator for confirmed-scope TCP discovery runs.
//!
//! This module owns scan-run policy and persistence. [`super::tcp`] owns only
//! TCP connect probes and their concurrency limits. Partial runs retain their
//! observations, but this coordinator never writes absence evidence or change
//! events, so only a complete run can later support closure reconciliation.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use domain::discovery::ApprovedScope;
use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::tcp::{ConnectScanner, ConnectScannerConfig, PortObservation, PortState, Scanner};

const TCP_PORT_COUNT: i64 = 65_535;
const SCAN_BATCH_SIZE: usize = 256;
const JOB_LEASE_SECS: i64 = 60;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Result used by the worker loop to distinguish a successful job from a
/// cooperative cancellation. A cancelled run must not be retried as failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Completed,
    Cancelled,
}

/// Execute one `discovery.full_tcp` job.
pub async fn handle(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    payload: Value,
) -> Result<JobOutcome> {
    let run = load_run(pool, job_id).await?;
    if run.status == "succeeded" && run.complete {
        return Ok(JobOutcome::Completed);
    }
    if run.status == "cancelled" || run.cancellation_requested || run.job_cancel_requested {
        mark_cancelled(pool, run.id).await?;
        return Ok(JobOutcome::Cancelled);
    }
    if let Err(error) =
        validate_payload_network(&payload, run.network_id).and_then(|_| validate_run(&run))
    {
        return async_fail_run(pool, run.id, error).await;
    }

    let scope = match validated_scope_targets(&run) {
        Ok(scope) => scope,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    let targets = {
        let targets = target_addresses(&scope);
        if targets.len() as i64 != run.targets_planned {
            let error = anyhow!(
                "scope target count changed: expected {}, got {}",
                run.targets_planned,
                targets.len()
            );
            return async_fail_run(pool, run.id, error).await;
        }
        targets
    };

    let config = match scanner_config(&run) {
        Ok(config) => config,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    tracing::debug!(
        connect_timeout_ms = config.connect_timeout().as_millis(),
        global_concurrency = config.global_concurrency(),
        network_concurrency = config.network_concurrency(),
        per_host_concurrency = config.per_host_concurrency(),
        run_id = %run.id,
        "configured TCP discovery scanner"
    );
    let scanner = match ConnectScanner::new(config).map_err(|error| anyhow!(error)) {
        Ok(scanner) => scanner,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    let device_ids = load_device_ids(pool).await?;
    let ports: Vec<u16> = (1..=u16::MAX).collect();
    let expected_ports = run
        .targets_planned
        .checked_mul(TCP_PORT_COUNT)
        .ok_or_else(|| anyhow!("planned TCP probe count overflow"))?;
    if run.ports_planned != expected_ports {
        let error = anyhow!(
            "scan run plans {} ports, expected {expected_ports}",
            run.ports_planned
        );
        return async_fail_run(pool, run.id, error).await;
    }

    execute_run(
        ExecutionContext {
            pool,
            job_id,
            worker_id,
        },
        run,
        ScanPlan {
            targets: &targets,
            ports: &ports,
            scanner: &scanner,
            device_ids: &device_ids,
        },
    )
    .await
}

#[derive(Debug, Clone)]
struct ScanRun {
    id: Uuid,
    network_id: Uuid,
    status: String,
    scope_version: i32,
    targets_planned: i64,
    targets_completed: i64,
    ports_planned: i64,
    ports_completed: i64,
    complete: bool,
    cancellation_requested: bool,
    cidr: String,
    excluded_cidrs: Option<Value>,
    scope_present: bool,
    scope_confirmed: bool,
    scope_enabled: Option<bool>,
    current_scope_version: Option<i32>,
    confirmed_target_count: Option<i64>,
    tcp_concurrency: Option<i32>,
    per_host_concurrency: Option<i32>,
    connect_timeout_ms: Option<i32>,
    job_cancel_requested: bool,
}

async fn load_run(pool: &PgPool, job_id: Uuid) -> Result<ScanRun> {
    let row = sqlx::query(
        "select sr.id, sr.network_id, sr.status, sr.scope_version, \
                sr.targets_planned, sr.targets_completed, sr.ports_planned, \
                sr.ports_completed, sr.complete, sr.cancellation_requested, \
                n.cidr::text as cidr, ds.excluded_cidrs, \
                (ds.network_id is not null) as scope_present, \
                (ds.confirmed_at is not null) as scope_confirmed, \
                ds.enabled as scope_enabled, ds.version as current_scope_version, \
                ds.confirmed_target_count, ds.tcp_concurrency, \
                ds.per_host_concurrency, ds.connect_timeout_ms, \
                j.cancel_requested as job_cancel_requested \
         from scan_runs sr \
         join jobs j on j.id = sr.job_id \
         join networks n on n.id = sr.network_id \
         left join discovery_scopes ds on ds.network_id = sr.network_id \
         where sr.job_id = $1",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow!("scan run not found for job {job_id}"))?;

    Ok(ScanRun {
        id: row.try_get("id")?,
        network_id: row.try_get("network_id")?,
        status: row.try_get("status")?,
        scope_version: row.try_get("scope_version")?,
        targets_planned: row.try_get("targets_planned")?,
        targets_completed: row.try_get("targets_completed")?,
        ports_planned: row.try_get("ports_planned")?,
        ports_completed: row.try_get("ports_completed")?,
        complete: row.try_get("complete")?,
        cancellation_requested: row.try_get("cancellation_requested")?,
        cidr: row.try_get("cidr")?,
        excluded_cidrs: row.try_get("excluded_cidrs")?,
        scope_present: row.try_get("scope_present")?,
        scope_confirmed: row.try_get("scope_confirmed")?,
        scope_enabled: row.try_get("scope_enabled")?,
        current_scope_version: row.try_get("current_scope_version")?,
        confirmed_target_count: row.try_get("confirmed_target_count")?,
        tcp_concurrency: row.try_get("tcp_concurrency")?,
        per_host_concurrency: row.try_get("per_host_concurrency")?,
        connect_timeout_ms: row.try_get("connect_timeout_ms")?,
        job_cancel_requested: row.try_get("job_cancel_requested")?,
    })
}

fn validate_payload_network(payload: &Value, network_id: Uuid) -> Result<()> {
    let Some(value) = payload.get("network_id") else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| anyhow!("discovery.full_tcp payload network_id is not a UUID string"))?;
    let payload_network_id = Uuid::parse_str(value)
        .with_context(|| "discovery.full_tcp payload network_id is invalid")?;
    if payload_network_id != network_id {
        return Err(anyhow!(
            "scan run network {network_id} does not match job payload network {payload_network_id}"
        ));
    }
    Ok(())
}

fn validate_run(run: &ScanRun) -> Result<()> {
    if run.complete && run.status != "succeeded" {
        return Err(anyhow!(
            "scan run marked complete with status {}",
            run.status
        ));
    }
    if run.targets_planned < 0
        || run.targets_completed < 0
        || run.ports_planned < 0
        || run.ports_completed < 0
    {
        return Err(anyhow!("scan run has negative progress"));
    }
    if run.targets_completed > run.targets_planned {
        return Err(anyhow!("scan run completed more targets than planned"));
    }
    if run.ports_completed > run.ports_planned {
        return Err(anyhow!("scan run completed more ports than planned"));
    }
    if run.scope_present
        && (!run.scope_confirmed
            || run.scope_enabled != Some(true)
            || run.current_scope_version != Some(run.scope_version)
            || run.confirmed_target_count != Some(run.targets_planned))
    {
        return Err(anyhow!(
            "scan run scope is no longer enabled, confirmed, or unchanged"
        ));
    }
    if !run.scope_present {
        return Err(anyhow!("scan run has no discovery scope"));
    }
    Ok(())
}

fn validated_scope_targets(run: &ScanRun) -> Result<ApprovedScope> {
    let excluded_cidrs: Vec<String> = serde_json::from_value(
        run.excluded_cidrs
            .clone()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .context("discovery scope exclusions are not a string array")?;
    let scope = ApprovedScope::parse(&run.cidr, &excluded_cidrs)
        .map_err(|error| anyhow!("invalid persisted discovery scope: {error}"))?;
    if scope.target_count() as i64 != run.targets_planned {
        return Err(anyhow!(
            "scope has {} targets, run planned {}",
            scope.target_count(),
            run.targets_planned
        ));
    }
    Ok(scope)
}

fn target_addresses(scope: &ApprovedScope) -> Vec<IpAddr> {
    let exclusions = scope.exclusions().to_vec();
    scope
        .cidr()
        .hosts()
        .filter(|address| {
            !exclusions
                .iter()
                .any(|exclusion| exclusion.contains(address))
        })
        .map(IpAddr::V4)
        .collect()
}

fn scanner_config(run: &ScanRun) -> Result<ConnectScannerConfig> {
    let tcp_concurrency = usize::try_from(
        run.tcp_concurrency
            .ok_or_else(|| anyhow!("discovery scope has no TCP concurrency"))?,
    )
    .context("discovery scope TCP concurrency is invalid")?;
    let per_host_concurrency = usize::try_from(
        run.per_host_concurrency
            .ok_or_else(|| anyhow!("discovery scope has no per-host concurrency"))?,
    )
    .context("discovery scope per-host concurrency is invalid")?;
    let connect_timeout_ms = u64::try_from(
        run.connect_timeout_ms
            .ok_or_else(|| anyhow!("discovery scope has no connect timeout"))?,
    )
    .context("discovery scope connect timeout is invalid")?;

    ConnectScannerConfig::new(
        Duration::from_millis(connect_timeout_ms),
        tcp_concurrency,
        tcp_concurrency,
        per_host_concurrency,
    )
    .map_err(|error| anyhow!(error))
}

async fn load_device_ids(pool: &PgPool) -> Result<HashMap<IpAddr, Uuid>> {
    let rows = sqlx::query(
        "select host(a.ip) as address, d.id \
         from addresses a \
         join interfaces i on i.id = a.interface_id \
         join devices d on d.id = i.device_id \
         where a.is_current and d.status != 'merged'",
    )
    .fetch_all(pool)
    .await?;
    let mut device_ids = HashMap::with_capacity(rows.len());
    for row in rows {
        let address: String = row.try_get("address")?;
        let address = address
            .parse()
            .with_context(|| format!("invalid current inventory address {address}"))?;
        device_ids.insert(address, row.try_get("id")?);
    }
    Ok(device_ids)
}

struct ExecutionContext<'a> {
    pool: &'a PgPool,
    job_id: Uuid,
    worker_id: &'a str,
}

struct ScanPlan<'a> {
    targets: &'a [IpAddr],
    ports: &'a [u16],
    scanner: &'a dyn Scanner,
    device_ids: &'a HashMap<IpAddr, Uuid>,
}

async fn execute_run(
    context: ExecutionContext<'_>,
    run: ScanRun,
    plan: ScanPlan<'_>,
) -> Result<JobOutcome> {
    let ExecutionContext {
        pool,
        job_id,
        worker_id,
    } = context;
    let ScanPlan {
        targets,
        ports,
        scanner,
        device_ids,
    } = plan;
    if ports.is_empty() {
        let error = anyhow!("TCP scan has no ports");
        mark_failed(pool, run.id, &error.to_string()).await?;
        return Err(error);
    }
    if targets.len() as i64 != run.targets_planned {
        let error = anyhow!(
            "scan target list has {}, run planned {}",
            targets.len(),
            run.targets_planned
        );
        mark_failed(pool, run.id, &error.to_string()).await?;
        return Err(error);
    }
    let ports_per_target = ports.len() as i64;
    let expected_ports = run
        .targets_planned
        .checked_mul(ports_per_target)
        .ok_or_else(|| anyhow!("planned TCP probe count overflow"))?;
    if expected_ports != run.ports_planned {
        let error = anyhow!(
            "scan port list plans {expected_ports} probes, run planned {}",
            run.ports_planned
        );
        mark_failed(pool, run.id, &error.to_string()).await?;
        return Err(error);
    }
    let completed_target_ports = run
        .targets_completed
        .checked_mul(ports_per_target)
        .ok_or_else(|| anyhow!("completed TCP probe count overflow"))?;
    let current_target_limit = if run.targets_completed < run.targets_planned {
        completed_target_ports
            .checked_add(ports_per_target)
            .ok_or_else(|| anyhow!("completed TCP probe count overflow"))?
    } else {
        completed_target_ports
    };
    if run.ports_completed < completed_target_ports || run.ports_completed > current_target_limit {
        let error = anyhow!("scan run progress does not align to target boundaries");
        mark_failed(pool, run.id, &error.to_string()).await?;
        return Err(error);
    }

    let cancellation = Arc::new(AtomicBool::new(false));
    if cancellation_requested(pool, job_id, run.id, &cancellation).await? {
        mark_cancelled(pool, run.id).await?;
        report_progress(
            pool,
            job_id,
            worker_id,
            &run,
            "cancelled",
            run.targets_completed,
            run.ports_completed,
        )
        .await?;
        return Ok(JobOutcome::Cancelled);
    }
    mark_running(pool, run.id).await?;
    report_progress(
        pool,
        job_id,
        worker_id,
        &run,
        "running",
        run.targets_completed,
        run.ports_completed,
    )
    .await?;
    let _monitor =
        CancellationMonitor::start(pool.clone(), job_id, run.id, Arc::clone(&cancellation));

    let mut ports_completed = run.ports_completed;
    let mut targets_completed = run.targets_completed;
    let first_target = usize::try_from(targets_completed)
        .context("scan run target progress does not fit in memory")?;
    let initial_port_offset = usize::try_from(ports_completed.rem_euclid(ports_per_target))
        .context("scan run port progress does not fit in memory")?;

    for (target_index, address) in targets.iter().enumerate() {
        if target_index < first_target {
            continue;
        }
        if cancellation_requested(pool, job_id, run.id, &cancellation).await? {
            return finish_cancelled(
                pool,
                job_id,
                worker_id,
                &run,
                targets_completed,
                ports_completed,
            )
            .await;
        }

        let port_offset = if target_index == first_target {
            initial_port_offset
        } else {
            0
        };
        let batches = ports[port_offset..].chunks(SCAN_BATCH_SIZE);
        let batch_count = batches.len();
        if batch_count == 0 {
            targets_completed = targets_completed
                .checked_add(1)
                .ok_or_else(|| anyhow!("scan target progress overflow"))?;
            if let Err(error) = mark_target_completed(pool, run.id, targets_completed).await {
                return async_fail_run(pool, run.id, error).await;
            }
            report_progress(
                pool,
                job_id,
                worker_id,
                &run,
                "running",
                targets_completed,
                ports_completed,
            )
            .await?;
            continue;
        }
        for (batch_index, batch) in batches.enumerate() {
            let batch_result = scan_batch(
                scanner,
                &run.network_id.to_string(),
                *address,
                batch,
                Arc::clone(&cancellation),
            )
            .await;
            let observations = batch_result.observations;
            let target_will_complete =
                batch_index + 1 == batch_count && batch_result.failure.is_none();
            let next_targets_completed = if target_will_complete {
                Some(
                    targets_completed
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("scan target progress overflow"))?,
                )
            } else {
                None
            };
            if !observations.is_empty() {
                let next_ports_completed = ports_completed
                    .checked_add(observations.len() as i64)
                    .ok_or_else(|| anyhow!("scan port progress overflow"))?;
                if let Err(error) = persist_observations(
                    pool,
                    run.id,
                    next_ports_completed,
                    next_targets_completed,
                    &observations,
                    device_ids,
                )
                .await
                {
                    return async_fail_run(pool, run.id, error).await;
                }
                ports_completed = next_ports_completed;
                if let Some(next_targets_completed) = next_targets_completed {
                    targets_completed = next_targets_completed;
                }
                report_progress(
                    pool,
                    job_id,
                    worker_id,
                    &run,
                    "running",
                    targets_completed,
                    ports_completed,
                )
                .await?;
            }

            if let Some(failure) = batch_result.failure {
                match failure {
                    ProbeFailure::Cancelled => {
                        return finish_cancelled(
                            pool,
                            job_id,
                            worker_id,
                            &run,
                            targets_completed,
                            ports_completed,
                        )
                        .await;
                    }
                    ProbeFailure::Scanner(error) => {
                        let error = anyhow!(error);
                        mark_failed(pool, run.id, &error.to_string()).await?;
                        return Err(error);
                    }
                }
            }
            if cancellation_requested(pool, job_id, run.id, &cancellation).await? {
                return finish_cancelled(
                    pool,
                    job_id,
                    worker_id,
                    &run,
                    targets_completed,
                    ports_completed,
                )
                .await;
            }
        }
    }

    if cancellation_requested(pool, job_id, run.id, &cancellation).await? {
        return finish_cancelled(
            pool,
            job_id,
            worker_id,
            &run,
            targets_completed,
            ports_completed,
        )
        .await;
    }
    if targets_completed != run.targets_planned || ports_completed != run.ports_planned {
        let error = anyhow!(
            "scan ended with {targets_completed}/{} targets and {ports_completed}/{} ports",
            run.targets_planned,
            run.ports_planned
        );
        mark_failed(pool, run.id, &error.to_string()).await?;
        return Err(error);
    }
    mark_succeeded(pool, run.id).await?;
    report_progress(
        pool,
        job_id,
        worker_id,
        &run,
        "succeeded",
        targets_completed,
        ports_completed,
    )
    .await?;
    Ok(JobOutcome::Completed)
}

struct BatchResult {
    observations: Vec<PortObservation>,
    failure: Option<ProbeFailure>,
}

enum ProbeFailure {
    Cancelled,
    Scanner(super::tcp::ScannerError),
}

async fn scan_batch<S: Scanner + ?Sized>(
    scanner: &S,
    network_key: &str,
    address: IpAddr,
    ports: &[u16],
    cancellation: Arc<AtomicBool>,
) -> BatchResult {
    let mut observations = Vec::with_capacity(ports.len());
    let mut probes = stream::iter(ports.iter().copied().map(|port| {
        let cancellation = Arc::clone(&cancellation);
        async move {
            if cancellation.load(Ordering::Acquire) {
                return Err(ProbeFailure::Cancelled);
            }
            scanner
                .scan_scoped(network_key, SocketAddr::new(address, port))
                .await
                .map_err(ProbeFailure::Scanner)
        }
    }))
    .buffered(SCAN_BATCH_SIZE);

    while let Some(result) = probes.next().await {
        match result {
            Ok(observation) => observations.push(observation),
            Err(failure) => {
                return BatchResult {
                    observations,
                    failure: Some(failure),
                };
            }
        }
    }
    BatchResult {
        observations,
        failure: None,
    }
}

async fn persist_observations(
    pool: &PgPool,
    run_id: Uuid,
    ports_completed: i64,
    targets_completed: Option<i64>,
    observations: &[PortObservation],
    device_ids: &HashMap<IpAddr, Uuid>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for observation in observations {
        let latency_ms = i32::try_from(observation.latency.as_millis()).unwrap_or(i32::MAX);
        sqlx::query(
            "insert into port_observations \
                (scan_run_id, device_id, address, port, transport, state, latency_ms) \
             values ($1, $2, $3::inet, $4, 'tcp', $5, $6) \
             on conflict (scan_run_id, address, port, transport) do nothing",
        )
        .bind(run_id)
        .bind(device_ids.get(&observation.address).copied())
        .bind(observation.address.to_string())
        .bind(i32::from(observation.port))
        .bind(port_state_name(observation.state))
        .bind(latency_ms)
        .execute(&mut *tx)
        .await?;
    }
    let updated = sqlx::query(
        "update scan_runs set ports_completed = $2, \
            targets_completed = coalesce($3, targets_completed), updated_at = now() \
         where id = $1 and status = 'running' and not complete",
    )
    .bind(run_id)
    .bind(ports_completed)
    .bind(targets_completed)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(anyhow!("scan run is no longer writable"));
    }
    tx.commit().await?;
    Ok(())
}

fn port_state_name(state: PortState) -> &'static str {
    match state {
        PortState::Open => "open",
        PortState::Closed => "closed",
        PortState::Filtered => "filtered",
    }
}

async fn mark_running(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let result = sqlx::query(
        "update scan_runs set status = 'running', started_at = coalesce(started_at, now()), \
            finished_at = null, error = null, updated_at = now() \
         where id = $1 and status in ('pending', 'running', 'failed') and not complete",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow!("scan run cannot enter running state"));
    }
    Ok(())
}

async fn mark_target_completed(pool: &PgPool, run_id: Uuid, targets_completed: i64) -> Result<()> {
    let result = sqlx::query(
        "update scan_runs set targets_completed = $2, updated_at = now() \
         where id = $1 and status = 'running' and not complete and not cancellation_requested",
    )
    .bind(run_id)
    .bind(targets_completed)
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow!("scan run target progress update was rejected"));
    }
    Ok(())
}

async fn mark_succeeded(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let result = sqlx::query(
        "update scan_runs set status = 'succeeded', complete = true, finished_at = now(), \
            error = null, updated_at = now() \
         where id = $1 and status = 'running' and not cancellation_requested \
           and targets_completed = targets_planned and ports_completed = ports_planned",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "scan run completion rejected before all probes completed"
        ));
    }
    Ok(())
}

async fn mark_failed(pool: &PgPool, run_id: Uuid, error: &str) -> Result<()> {
    sqlx::query(
        "update scan_runs set status = 'failed', complete = false, finished_at = now(), \
            error = $2, updated_at = now() where id = $1 and not complete",
    )
    .bind(run_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_cancelled(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        "update scan_runs set status = 'cancelled', complete = false, \
            cancellation_requested = true, finished_at = now(), \
            error = coalesce(error, 'cancellation requested'), updated_at = now() \
         where id = $1 and not complete",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn cancellation_requested(
    pool: &PgPool,
    job_id: Uuid,
    run_id: Uuid,
    cancellation: &AtomicBool,
) -> Result<bool> {
    if cancellation.load(Ordering::Acquire) {
        return Ok(true);
    }
    let row = sqlx::query(
        "select j.cancel_requested or sr.cancellation_requested as requested \
         from jobs j join scan_runs sr on sr.job_id = j.id \
         where j.id = $1 and sr.id = $2",
    )
    .bind(job_id)
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let requested: bool = row.try_get("requested")?;
    if requested {
        cancellation.store(true, Ordering::Release);
    }
    Ok(requested)
}

async fn report_progress(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    run: &ScanRun,
    status: &str,
    targets_completed: i64,
    ports_completed: i64,
) -> Result<()> {
    let alive = jobs::heartbeat(
        pool,
        job_id,
        worker_id,
        JOB_LEASE_SECS,
        Some(json!({
            "run_id": run.id,
            "status": status,
            "targets_planned": run.targets_planned,
            "targets_completed": targets_completed,
            "ports_planned": run.ports_planned,
            "ports_completed": ports_completed,
        })),
    )
    .await?;
    if !alive {
        return Err(anyhow!("job lease lost while scanning"));
    }
    Ok(())
}

async fn finish_cancelled(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    run: &ScanRun,
    targets_completed: i64,
    ports_completed: i64,
) -> Result<JobOutcome> {
    mark_cancelled(pool, run.id).await?;
    report_progress(
        pool,
        job_id,
        worker_id,
        run,
        "cancelled",
        targets_completed,
        ports_completed,
    )
    .await?;
    Ok(JobOutcome::Cancelled)
}

async fn async_fail_run<T>(pool: &PgPool, run_id: Uuid, error: anyhow::Error) -> Result<T> {
    mark_failed(pool, run_id, &error.to_string()).await?;
    Err(error)
}

struct CancellationMonitor {
    task: JoinHandle<()>,
}

impl CancellationMonitor {
    fn start(pool: PgPool, job_id: Uuid, run_id: Uuid, cancellation: Arc<AtomicBool>) -> Self {
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(CANCELLATION_POLL_INTERVAL).await;
                let requested = sqlx::query(
                    "select j.cancel_requested or sr.cancellation_requested as requested \
                     from jobs j join scan_runs sr on sr.job_id = j.id \
                     where j.id = $1 and sr.id = $2",
                )
                .bind(job_id)
                .bind(run_id)
                .fetch_optional(&pool)
                .await;
                match requested {
                    Ok(Some(row)) => match row.try_get::<bool, _>("requested") {
                        Ok(true) => {
                            cancellation.store(true, Ordering::Release);
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(%error, "discovery cancellation poll failed");
                        }
                    },
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "discovery cancellation poll failed");
                    }
                }
            }
        });
        Self { task }
    }
}

impl Drop for CancellationMonitor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn target_addresses_match_approved_scope_exclusions() {
        let scope = ApprovedScope::parse(
            "192.168.20.0/29",
            &["192.168.20.2/31".to_string(), "192.168.20.6/32".to_string()],
        )
        .expect("valid scope");

        let targets = target_addresses(&scope);

        assert_eq!(
            targets,
            vec![
                "192.168.20.1".parse::<IpAddr>().unwrap(),
                "192.168.20.4".parse::<IpAddr>().unwrap(),
                "192.168.20.5".parse::<IpAddr>().unwrap(),
            ]
        );
    }

    #[test]
    fn port_state_persistence_names_are_closed_set() {
        assert_eq!(port_state_name(PortState::Open), "open");
        assert_eq!(port_state_name(PortState::Closed), "closed");
        assert_eq!(port_state_name(PortState::Filtered), "filtered");
    }

    #[tokio::test]
    async fn db_run_persists_observations_and_marks_complete_only_after_plan() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let network_id: (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('192.168.30.10/32', $1) returning id",
        )
        .bind(format!("scan-test-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create test network");
        let network_id = network_id.0;
        sqlx::query(
            "insert into discovery_scopes \
                (network_id, target_count, confirmed_target_count, confirmed_at) \
             values ($1, 1, 1, now())",
        )
        .bind(network_id)
        .execute(&pool)
        .await
        .expect("create confirmed scope");
        let job_id: (Uuid,) = sqlx::query_as(
            "insert into jobs (job_type, idempotency_key, payload, status, locked_by, lease_expires_at) \
             values ('discovery.full_tcp', $1, $2, 'running', 'test-worker', now() + interval '1 minute') \
             returning id",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(json!({"network_id": network_id}))
        .fetch_one(&pool)
        .await
        .expect("create test job");
        let job_id = job_id.0;
        let run_id: (Uuid,) = sqlx::query_as(
            "insert into scan_runs \
                (network_id, job_id, kind, scope_version, targets_planned, ports_planned) \
             values ($1, $2, 'full_tcp', 1, 1, 2) returning id",
        )
        .bind(network_id)
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("create test run");
        let run = load_run(&pool, job_id).await.expect("load test run");
        let scanner = FakeScanner::default();
        let target = "192.168.30.10".parse().expect("test target");
        let device_ids = HashMap::new();
        let outcome = execute_run(
            ExecutionContext {
                pool: &pool,
                job_id,
                worker_id: "test-worker",
            },
            run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &scanner,
                device_ids: &device_ids,
            },
        )
        .await
        .expect("complete test run");

        assert_eq!(outcome, JobOutcome::Completed);
        let state: (String, bool, i64, i64) = sqlx::query_as(
            "select status, complete, targets_completed, ports_completed from scan_runs where id = $1",
        )
        .bind(run_id.0)
        .fetch_one(&pool)
        .await
        .expect("read completed run");
        assert_eq!(state, ("succeeded".to_string(), true, 1, 2));
        let observations: (i64,) =
            sqlx::query_as("select count(*) from port_observations where scan_run_id = $1")
                .bind(run_id.0)
                .fetch_one(&pool)
                .await
                .expect("count observations");
        assert_eq!(observations.0, 2);
        let progress: (Value,) = sqlx::query_as("select progress from jobs where id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .expect("read job progress");
        assert_eq!(progress.0["status"], "succeeded");
        assert_eq!(progress.0["ports_completed"], 2);
    }

    #[tokio::test]
    async fn db_failed_run_stays_partial_and_writes_no_closure_evidence() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (network_id, job_id, run_id) = test_run(&pool, 2).await;
        let run = load_run(&pool, job_id).await.expect("load test run");
        let scanner = FakeScanner {
            fail_port: Some(443),
            ..FakeScanner::default()
        };
        let device_ids = HashMap::new();
        let error = execute_run(
            ExecutionContext {
                pool: &pool,
                job_id,
                worker_id: "test-worker",
            },
            run,
            ScanPlan {
                targets: &["192.168.30.10".parse().unwrap()],
                ports: &[80, 443],
                scanner: &scanner,
                device_ids: &device_ids,
            },
        )
        .await
        .expect_err("scanner failure must fail run");
        assert!(
            error
                .to_string()
                .contains("scanner concurrency limiter closed")
        );

        let state: (String, bool, i64, i64) = sqlx::query_as(
            "select status, complete, targets_completed, ports_completed from scan_runs where id = $1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read failed run");
        assert_eq!(state, ("failed".to_string(), false, 0, 1));
        let evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence where source_type = 'network_scan' and subject_id = $1",
        )
        .bind(network_id)
        .fetch_one(&pool)
        .await
        .expect("count closure evidence");
        assert_eq!(evidence.0, 0);
    }

    #[tokio::test]
    async fn db_cancelled_run_stays_incomplete_and_adds_no_closure_evidence() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (network_id, job_id, run_id) = test_run(&pool, 2).await;
        jobs::request_cancel(&pool, job_id)
            .await
            .expect("request test cancellation");
        let run = load_run(&pool, job_id).await.expect("load test run");
        let scanner = FakeScanner::default();
        let device_ids = HashMap::new();
        let outcome = execute_run(
            ExecutionContext {
                pool: &pool,
                job_id,
                worker_id: "test-worker",
            },
            run,
            ScanPlan {
                targets: &["192.168.30.10".parse().unwrap()],
                ports: &[80, 443],
                scanner: &scanner,
                device_ids: &device_ids,
            },
        )
        .await
        .expect("cancel test run");

        assert_eq!(outcome, JobOutcome::Cancelled);
        let state: (String, bool, i64) =
            sqlx::query_as("select status, complete, ports_completed from scan_runs where id = $1")
                .bind(run_id)
                .fetch_one(&pool)
                .await
                .expect("read cancelled run");
        assert_eq!(state, ("cancelled".to_string(), false, 0));
        assert_eq!(scanner.calls.load(Ordering::Relaxed), 0);
        let evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence where source_type = 'network_scan' and subject_id = $1",
        )
        .bind(network_id)
        .fetch_one(&pool)
        .await
        .expect("count closure evidence");
        assert_eq!(evidence.0, 0);
    }

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    async fn test_run(pool: &PgPool, ports_planned: i64) -> (Uuid, Uuid, Uuid) {
        let (network_id,): (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('192.168.30.10/32', $1) returning id",
        )
        .bind(format!("scan-test-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("create test network");
        sqlx::query(
            "insert into discovery_scopes \
                (network_id, target_count, confirmed_target_count, confirmed_at) \
             values ($1, 1, 1, now())",
        )
        .bind(network_id)
        .execute(pool)
        .await
        .expect("create confirmed scope");
        let (job_id,): (Uuid,) = sqlx::query_as(
            "insert into jobs (job_type, idempotency_key, payload, status, locked_by, lease_expires_at) \
             values ('discovery.full_tcp', $1, $2, 'running', 'test-worker', now() + interval '1 minute') \
             returning id",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(json!({"network_id": network_id}))
        .fetch_one(pool)
        .await
        .expect("create test job");
        let (run_id,): (Uuid,) = sqlx::query_as(
            "insert into scan_runs \
                (network_id, job_id, kind, scope_version, targets_planned, ports_planned) \
             values ($1, $2, 'full_tcp', 1, 1, $3) returning id",
        )
        .bind(network_id)
        .bind(job_id)
        .bind(ports_planned)
        .fetch_one(pool)
        .await
        .expect("create test run");
        (network_id, job_id, run_id)
    }

    #[derive(Default)]
    struct FakeScanner {
        fail_port: Option<u16>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Scanner for FakeScanner {
        async fn scan(
            &self,
            target: SocketAddr,
        ) -> std::result::Result<PortObservation, super::super::tcp::ScannerError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_port == Some(target.port()) {
                return Err(super::super::tcp::ScannerError::LimiterClosed);
            }
            Ok(PortObservation {
                address: target.ip(),
                port: target.port(),
                state: PortState::Closed,
                latency: Duration::from_millis(1),
            })
        }
    }
}
