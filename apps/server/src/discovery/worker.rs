//! Worker coordinator for confirmed-scope TCP discovery runs.
//!
//! This module owns scan-run policy and persistence. [`super::tcp`] owns only
//! TCP connect probes and their concurrency limits. Partial runs retain their
//! observations and positive evidence, while only a complete successful run
//! writes absence evidence and closure change events.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use domain::discovery::ApprovedScope;
use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::classification::{ClassificationResult, Classifier};
#[cfg(test)]
use super::policy::PacingConfig;
use super::policy::{
    NORMAL_CLASSIFICATION_CONCURRENCY, ResolvedScanPolicy, ScanPacer, probe_seed,
    resolve_scan_policy,
};
use super::tcp::{ConnectScanner, ConnectScannerConfig, PortObservation, PortState, Scanner};
use crate::inventory::{addresses, events::Recorder, evidence};

const TCP_PORT_COUNT: i64 = 65_535;
const SCAN_BATCH_SIZE: usize = 256;
const JOB_LEASE_SECS: i64 = 60;
const JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const JOB_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Keep classification fan-out independent from TCP scan fan-out. Each
// classifier already has its own bounded connection budget, so this cap
// prevents one batch of open ports from causing unbounded re-probing.
const CLASSIFICATION_CONCURRENCY: usize = NORMAL_CLASSIFICATION_CONCURRENCY;
const _: () = assert!(CLASSIFICATION_CONCURRENCY > 1);
const PORT_EVIDENCE_ATTRIBUTE: &str = "open_port";
const PORT_EVIDENCE_CONFIDENCE: f32 = 0.9;
const CLASSIFICATION_EVIDENCE_ATTRIBUTE: &str = "protocol_classification";

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
    handle_inner(pool, job_id, worker_id, payload, None).await
}

async fn handle_inner(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    payload: Value,
    ports_override: Option<&[u16]>,
) -> Result<JobOutcome> {
    // The ignored Docker gate supplies one published port here so it can
    // exercise this production path without probing all 65,535 ports.
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

    let policy = match run_policy(&run) {
        Ok(policy) => policy,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    let config = match scanner_config(&run, policy) {
        Ok(config) => config,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    tracing::debug!(
        connect_timeout_ms = config.connect_timeout().as_millis(),
        global_concurrency = config.global_concurrency(),
        network_concurrency = config.network_concurrency(),
        per_host_concurrency = config.per_host_concurrency(),
        scan_profile = policy.profile().as_str(),
        classification_concurrency = policy.classification_concurrency(),
        tcp_probe_interval_ms = policy.tcp_pacing().interval().as_millis(),
        tcp_jitter_ms = policy.tcp_pacing().jitter().as_millis(),
        tcp_backoff_step_ms = policy.tcp_pacing().backoff_step().as_millis(),
        tcp_backoff_max_ms = policy.tcp_pacing().backoff_max().as_millis(),
        classification_interval_ms = policy.classification_pacing().interval().as_millis(),
        classification_jitter_ms = policy.classification_pacing().jitter().as_millis(),
        classification_backoff_step_ms = policy.classification_pacing().backoff_step().as_millis(),
        classification_backoff_max_ms = policy.classification_pacing().backoff_max().as_millis(),
        run_id = %run.id,
        "configured TCP discovery scanner"
    );
    let scanner = match ConnectScanner::new(config).map_err(|error| anyhow!(error)) {
        Ok(scanner) => scanner,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    let mut device_ids = load_device_ids(pool).await?;
    let expected_port_count = ports_override.map_or(TCP_PORT_COUNT, |ports| ports.len() as i64);
    let ports: Vec<u16> = ports_override
        .map(<[u16]>::to_vec)
        .unwrap_or_else(|| (1..=u16::MAX).collect());
    let expected_ports = run
        .targets_planned
        .checked_mul(expected_port_count)
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
            device_ids: &mut device_ids,
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
    scan_profile: Option<String>,
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
                ds.per_host_concurrency, ds.connect_timeout_ms, ds.scan_profile, \
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
        scan_profile: row.try_get("scan_profile")?,
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

fn run_policy(run: &ScanRun) -> Result<ResolvedScanPolicy> {
    let profile = run
        .scan_profile
        .as_deref()
        .ok_or_else(|| anyhow!("discovery scope has no scan profile"))?;
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

    resolve_scan_policy(profile, tcp_concurrency, per_host_concurrency)
        .map_err(|error| anyhow!("invalid discovery scan policy: {error}"))
}

fn scanner_config(run: &ScanRun, policy: ResolvedScanPolicy) -> Result<ConnectScannerConfig> {
    let connect_timeout_ms = u64::try_from(
        run.connect_timeout_ms
            .ok_or_else(|| anyhow!("discovery scope has no connect timeout"))?,
    )
    .context("discovery scope connect timeout is invalid")?;

    ConnectScannerConfig::new(
        Duration::from_millis(connect_timeout_ms),
        policy.tcp_global_concurrency(),
        policy.tcp_network_concurrency(),
        policy.tcp_per_host_concurrency(),
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
    device_ids: &'a mut HashMap<IpAddr, Uuid>,
}

#[derive(Debug, Default)]
struct WorkerControl {
    cancellation_requested: AtomicBool,
    lease_lost: AtomicBool,
    stop_notify: Notify,
}

impl WorkerControl {
    fn request_cancellation(&self) {
        self.cancellation_requested.store(true, Ordering::Release);
        self.stop_notify.notify_waiters();
    }

    fn mark_lease_lost(&self) {
        self.lease_lost.store(true, Ordering::Release);
        self.stop_notify.notify_waiters();
    }

    fn cancellation_requested(&self) -> bool {
        self.cancellation_requested.load(Ordering::Acquire)
    }

    fn lease_lost(&self) -> bool {
        self.lease_lost.load(Ordering::Acquire)
    }

    fn stop_requested(&self) -> bool {
        self.cancellation_requested() || self.lease_lost()
    }

    fn stop_reason(&self) -> Option<StopReason> {
        if self.lease_lost() {
            Some(StopReason::LeaseLost)
        } else if self.cancellation_requested() {
            Some(StopReason::Cancelled)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    LeaseLost,
}

#[derive(Debug)]
struct JobProgress {
    targets_completed: AtomicI64,
    ports_completed: AtomicI64,
}

impl JobProgress {
    fn new(targets_completed: i64, ports_completed: i64) -> Self {
        Self {
            targets_completed: AtomicI64::new(targets_completed),
            ports_completed: AtomicI64::new(ports_completed),
        }
    }

    fn set(&self, targets_completed: i64, ports_completed: i64) {
        self.targets_completed
            .store(targets_completed, Ordering::Release);
        self.ports_completed
            .store(ports_completed, Ordering::Release);
    }

    fn snapshot(&self) -> (i64, i64) {
        (
            self.targets_completed.load(Ordering::Acquire),
            self.ports_completed.load(Ordering::Acquire),
        )
    }
}

struct JobHeartbeatMonitor {
    task: JoinHandle<()>,
}

impl JobHeartbeatMonitor {
    fn start(
        pool: PgPool,
        job_id: Uuid,
        worker_id: String,
        run: ScanRun,
        progress: Arc<JobProgress>,
        control: Arc<WorkerControl>,
    ) -> Self {
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + JOB_HEARTBEAT_INTERVAL,
                JOB_HEARTBEAT_INTERVAL,
            );
            loop {
                interval.tick().await;
                let (targets_completed, ports_completed) = progress.snapshot();
                let heartbeat = tokio::time::timeout(
                    JOB_HEARTBEAT_TIMEOUT,
                    jobs::heartbeat(
                        &pool,
                        job_id,
                        &worker_id,
                        JOB_LEASE_SECS,
                        Some(json!({
                            "run_id": run.id,
                            "status": "running",
                            "targets_planned": run.targets_planned,
                            "targets_completed": targets_completed,
                            "ports_planned": run.ports_planned,
                            "ports_completed": ports_completed,
                        })),
                    ),
                )
                .await;
                match heartbeat {
                    Ok(Ok(true)) => {}
                    Ok(Ok(false)) => {
                        tracing::warn!(job_id = %job_id, "job lease lost during discovery");
                        control.mark_lease_lost();
                        break;
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(job_id = %job_id, %error, "discovery lease heartbeat failed");
                        control.mark_lease_lost();
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(job_id = %job_id, "discovery lease heartbeat timed out");
                        control.mark_lease_lost();
                        break;
                    }
                }
            }
        });
        Self { task }
    }
}

impl Drop for JobHeartbeatMonitor {
    fn drop(&mut self) {
        self.task.abort();
    }
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
    let policy = match run_policy(&run) {
        Ok(policy) => policy,
        Err(error) => return async_fail_run(pool, run.id, error).await,
    };
    let run_seed = u64::from_le_bytes(run.id.as_bytes()[..8].try_into().expect("UUID is 16 bytes"));
    let tcp_pacer = Arc::new(ScanPacer::production(policy.tcp_pacing(), run_seed));
    let classification_pacer = Arc::new(ScanPacer::production(
        policy.classification_pacing(),
        run_seed ^ 0x6a09_e667_f3bc_c909,
    ));
    let classifier = Classifier::new(Default::default()).map_err(|error| anyhow!(error))?;
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

    let control = Arc::new(WorkerControl::default());
    let progress = Arc::new(JobProgress::new(run.targets_completed, run.ports_completed));
    if let Some(reason) = stop_requested(pool, job_id, run.id, &control).await? {
        return finish_stop(pool, job_id, worker_id, &run, &progress, reason).await;
    }
    mark_running(pool, run.id).await?;
    report_progress(pool, job_id, worker_id, &run, "running", &progress).await?;
    let _cancellation_monitor =
        CancellationMonitor::start(pool.clone(), job_id, run.id, Arc::clone(&control));
    let _heartbeat_monitor = JobHeartbeatMonitor::start(
        pool.clone(),
        job_id,
        worker_id.to_string(),
        run.clone(),
        Arc::clone(&progress),
        Arc::clone(&control),
    );

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
        if let Some(reason) = stop_requested(pool, job_id, run.id, &control).await? {
            return finish_stop(pool, job_id, worker_id, &run, &progress, reason).await;
        }

        let mut address_has_liveness = false;

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
            progress.set(targets_completed, ports_completed);
            report_progress(pool, job_id, worker_id, &run, "running", &progress).await?;
            continue;
        }
        for (batch_index, batch) in batches.enumerate() {
            let batch_result = scan_batch(
                scanner,
                &run.network_id.to_string(),
                *address,
                batch,
                ScanBatchContext {
                    run_seed,
                    target_index,
                    batch_index,
                    pacer: &tcp_pacer,
                },
                Arc::clone(&control),
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
            if !address_has_liveness && observations.iter().any(observation_proves_liveness) {
                let device_id = addresses::ensure_scanned_address(pool, *address)
                    .await
                    .with_context(|| format!("resolve live scanned address {address}"))?;
                device_ids.insert(*address, device_id);
                attach_run_observations_to_device(pool, run.id, *address, device_id).await?;
                address_has_liveness = true;
            }
            if !observations.is_empty() {
                let classification_batch = classify_open_observations_with_policy(
                    &classifier,
                    &observations,
                    device_ids,
                    Arc::clone(&control),
                    policy.classification_concurrency(),
                    Arc::clone(&classification_pacer),
                    run_seed,
                )
                .await;
                if control.lease_lost() {
                    return Err(anyhow!("job lease lost while classifying"));
                }
                let ClassificationBatch {
                    classifications,
                    cancelled: classification_cancelled,
                } = classification_batch;
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
                    &classifications,
                )
                .await
                {
                    return async_fail_run(pool, run.id, error).await;
                }
                ports_completed = next_ports_completed;
                if let Some(next_targets_completed) = next_targets_completed {
                    targets_completed = next_targets_completed;
                }
                progress.set(targets_completed, ports_completed);
                report_progress(pool, job_id, worker_id, &run, "running", &progress).await?;
                if classification_cancelled {
                    return finish_cancelled(pool, job_id, worker_id, &run, &progress).await;
                }
            }

            if let Some(failure) = batch_result.failure {
                match failure {
                    ProbeFailure::Stopped => {
                        let reason = control.stop_reason().ok_or_else(|| {
                            anyhow!("TCP scan stopped without cancellation or lease-loss reason")
                        })?;
                        return finish_stop(pool, job_id, worker_id, &run, &progress, reason).await;
                    }
                    ProbeFailure::Scanner(error) => {
                        let error = anyhow!(error);
                        mark_failed(pool, run.id, &error.to_string()).await?;
                        return Err(error);
                    }
                }
            }
            if let Some(reason) = stop_requested(pool, job_id, run.id, &control).await? {
                return finish_stop(pool, job_id, worker_id, &run, &progress, reason).await;
            }
        }
    }

    if let Some(reason) = stop_requested(pool, job_id, run.id, &control).await? {
        return finish_stop(pool, job_id, worker_id, &run, &progress, reason).await;
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
    if let Err(error) = complete_run(pool, run.id, targets, ports).await {
        return async_fail_run(pool, run.id, error).await;
    }
    progress.set(targets_completed, ports_completed);
    report_progress(pool, job_id, worker_id, &run, "succeeded", &progress).await?;
    Ok(JobOutcome::Completed)
}

struct BatchResult {
    observations: Vec<PortObservation>,
    failure: Option<ProbeFailure>,
}

struct ScanBatchContext<'a> {
    run_seed: u64,
    target_index: usize,
    batch_index: usize,
    pacer: &'a ScanPacer,
}

struct ClassifiedOpenPort {
    address: IpAddr,
    port: u16,
    device_id: Uuid,
    result: ClassificationResult,
}

struct ClassificationBatch {
    classifications: Vec<ClassifiedOpenPort>,
    cancelled: bool,
}

#[async_trait]
trait OpenPortClassifier: Send + Sync {
    async fn classify(&self, address: IpAddr, port: u16) -> ClassificationResult;
}

#[async_trait]
impl OpenPortClassifier for Classifier {
    async fn classify(&self, address: IpAddr, port: u16) -> ClassificationResult {
        Classifier::classify(self, address, port).await
    }
}

#[cfg(test)]
async fn classify_open_observations<C: OpenPortClassifier + ?Sized>(
    classifier: &C,
    observations: &[PortObservation],
    device_ids: &HashMap<IpAddr, Uuid>,
    control: Arc<WorkerControl>,
) -> ClassificationBatch {
    classify_open_observations_with_policy(
        classifier,
        observations,
        device_ids,
        control,
        CLASSIFICATION_CONCURRENCY,
        Arc::new(ScanPacer::production(PacingConfig::disabled(), 0)),
        0,
    )
    .await
}

async fn classify_open_observations_with_policy<C: OpenPortClassifier + ?Sized>(
    classifier: &C,
    observations: &[PortObservation],
    device_ids: &HashMap<IpAddr, Uuid>,
    control: Arc<WorkerControl>,
    concurrency: usize,
    pacer: Arc<ScanPacer>,
    run_seed: u64,
) -> ClassificationBatch {
    let candidates: Vec<(usize, IpAddr, u16, Uuid)> = observations
        .iter()
        .enumerate()
        .filter_map(|(index, observation)| {
            if observation.state != PortState::Open {
                return None;
            }
            let device_id = device_ids.get(&observation.address).copied()?;
            Some((index, observation.address, observation.port, device_id))
        })
        .collect();

    let mut pending = stream::iter(candidates.into_iter().map(
        |(index, address, port, device_id)| {
            let control = Arc::clone(&control);
            let pacer = Arc::clone(&pacer);
            let pacing_attempt = u32::try_from(index / concurrency.max(1)).unwrap_or(u32::MAX);
            async move {
                let (result, stopped) = tokio::select! {
                    biased;
                    _ = wait_for_stop(Arc::clone(&control)) => {
                        let reason = if control.cancellation_requested() {
                            "cancelled"
                        } else {
                            "lease_lost"
                        };
                        (ClassificationResult::generic_fallback(address, port, Some(reason)), true)
                    }
                    _ = pacer.wait(probe_seed(run_seed, index, 0, port), pacing_attempt) => {
                        if control.stop_requested() {
                            let reason = if control.cancellation_requested() {
                                "cancelled"
                            } else {
                                "lease_lost"
                            };
                            (ClassificationResult::generic_fallback(address, port, Some(reason)), true)
                        } else {
                            tokio::select! {
                                biased;
                                _ = wait_for_stop(Arc::clone(&control)) => {
                                    let reason = if control.cancellation_requested() {
                                        "cancelled"
                                    } else {
                                        "lease_lost"
                                    };
                                    (ClassificationResult::generic_fallback(address, port, Some(reason)), true)
                                }
                                result = classifier.classify(address, port) => (result, false),
                            }
                        }
                    }
                };
                (
                    index,
                    ClassifiedOpenPort {
                        address,
                        port,
                        device_id,
                        result,
                    },
                    stopped,
                )
            }
        },
    ))
    .buffer_unordered(concurrency.max(1));

    let mut indexed = Vec::new();
    let mut cancelled = false;
    while let Some((index, classification, stopped)) = pending.next().await {
        cancelled |= stopped;
        indexed.push((index, classification));
    }
    indexed.sort_unstable_by_key(|(index, _)| *index);
    ClassificationBatch {
        classifications: indexed
            .into_iter()
            .map(|(_, classification)| classification)
            .collect(),
        cancelled,
    }
}

enum ProbeFailure {
    Stopped,
    Scanner(super::tcp::ScannerError),
}

async fn wait_for_stop(control: Arc<WorkerControl>) {
    loop {
        let notified = control.stop_notify.notified();
        tokio::pin!(notified);
        // Register before checking the flag. This closes the race where a
        // cancellation arrives between the flag check and awaiting Notify.
        notified.as_mut().enable();
        if control.stop_requested() {
            return;
        }
        notified.await;
    }
}

async fn scan_batch<S: Scanner + ?Sized>(
    scanner: &S,
    network_key: &str,
    address: IpAddr,
    ports: &[u16],
    context: ScanBatchContext<'_>,
    control: Arc<WorkerControl>,
) -> BatchResult {
    let ScanBatchContext {
        run_seed,
        target_index,
        batch_index,
        pacer,
    } = context;
    let mut observations = Vec::with_capacity(ports.len());
    let mut probes = stream::iter(ports.iter().copied().map(|port| {
        let control = Arc::clone(&control);
        let seed = probe_seed(run_seed, target_index, batch_index, port);
        async move {
            if control.stop_requested() {
                return Err(ProbeFailure::Stopped);
            }
            tokio::select! {
                biased;
                _ = wait_for_stop(Arc::clone(&control)) => Err(ProbeFailure::Stopped),
                _ = pacer.wait(seed, batch_index as u32) => {
                    if control.stop_requested() {
                        Err(ProbeFailure::Stopped)
                    } else {
                        tokio::select! {
                            biased;
                            _ = wait_for_stop(Arc::clone(&control)) => Err(ProbeFailure::Stopped),
                            result = scanner.scan_scoped(network_key, SocketAddr::new(address, port)) => {
                                result.map_err(ProbeFailure::Scanner)
                            }
                        }
                    }
                }
            }
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

fn port_evidence_value(observation: &PortObservation) -> Value {
    json!({
        "address": observation.address.to_string(),
        "port": observation.port,
        "transport": "tcp",
    })
}

fn port_snapshot(value: &Value, state: &str) -> Value {
    let mut snapshot = value.clone();
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("state".to_string(), Value::String(state.to_string()));
        snapshot
    } else {
        json!({"value": value, "state": state})
    }
}

async fn record_open_port_evidence(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    device_id: Uuid,
    observation: &PortObservation,
) -> Result<Uuid> {
    let value = port_evidence_value(observation);
    let previous: Option<(bool, Value)> = sqlx::query_as(
        "select absent, value from evidence \
         where subject_table = 'devices' and subject_id = $1 \
           and source_type = 'network_scan' and attribute = $2 and value = $3 \
         order by last_seen desc, created_at desc, id desc limit 1",
    )
    .bind(device_id)
    .bind(PORT_EVIDENCE_ATTRIBUTE)
    .bind(&value)
    .fetch_optional(&mut **tx)
    .await?;

    let source_instance = run_id.to_string();
    let evidence_id = evidence::record_automatic_tx(
        tx,
        "devices",
        device_id,
        "network_scan",
        Some(&source_instance),
        PORT_EVIDENCE_ATTRIBUTE,
        &value,
        PORT_EVIDENCE_CONFIDENCE,
        false,
    )
    .await?;

    let was_open = previous
        .as_ref()
        .map(|(absent, _)| !*absent)
        .unwrap_or(false);
    if !was_open {
        Recorder::record_change(
            tx,
            "devices",
            device_id,
            "port.opened",
            "notice",
            previous.as_ref().map(|_| port_snapshot(&value, "closed")),
            Some(port_snapshot(&value, "open")),
            Some("network_scan"),
        )
        .await?;
    }

    Ok(evidence_id)
}

async fn persist_observations(
    pool: &PgPool,
    run_id: Uuid,
    ports_completed: i64,
    targets_completed: Option<i64>,
    observations: &[PortObservation],
    device_ids: &HashMap<IpAddr, Uuid>,
    classifications: &[ClassifiedOpenPort],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let classifications_by_port: HashMap<(IpAddr, u16), &ClassifiedOpenPort> = classifications
        .iter()
        .map(|classification| {
            (
                (classification.address, classification.port),
                classification,
            )
        })
        .collect();
    for observation in observations {
        let device_id = device_ids.get(&observation.address).copied();
        let evidence_id = if observation.state == PortState::Open {
            let device_id = device_id.ok_or_else(|| {
                anyhow!(
                    "open observation has no resolved device for scanned address {}",
                    observation.address
                )
            })?;
            Some(record_open_port_evidence(&mut tx, run_id, device_id, observation).await?)
        } else {
            None
        };
        if let Some(classification) =
            classifications_by_port.get(&(observation.address, observation.port))
        {
            reconcile_service_classification(
                &mut tx,
                run_id,
                classification.device_id,
                classification.address,
                classification.port,
                &classification.result,
            )
            .await?;
        }
        let latency_ms = i32::try_from(observation.latency.as_millis()).unwrap_or(i32::MAX);
        sqlx::query(
            "insert into port_observations \
                (scan_run_id, device_id, address, port, transport, state, latency_ms, evidence_id) \
             values ($1, $2, $3::inet, $4, 'tcp', $5, $6, $7) \
             on conflict (scan_run_id, address, port, transport) do nothing",
        )
        .bind(run_id)
        .bind(device_id)
        .bind(observation.address.to_string())
        .bind(i32::from(observation.port))
        .bind(port_state_name(observation.state))
        .bind(latency_ms)
        .bind(evidence_id)
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

async fn reconcile_service_classification(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    device_id: Uuid,
    address: IpAddr,
    port: u16,
    result: &ClassificationResult,
) -> Result<()> {
    let lock_key = format!("service-classification:{device_id}:{address}:{port}");
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;

    let existing: Option<(Uuid, Uuid, Option<String>, bool, bool)> = sqlx::query_as(
        "select s.id, e.id, s.protocol, e.is_current, exists( \
             select 1 from evidence manual \
             where manual.subject_table = 'services' and manual.subject_id = s.id \
               and manual.attribute in ('protocol', 'protocol_classification') \
               and manual.source_type = 'manual' and manual.confirmed_by is not null \
               and not manual.absent) \
         from endpoints e \
         join services s on s.id = e.service_id \
         where s.owner_kind = 'device' and s.owner_id = $1 \
           and e.endpoint_type = 'socket' and e.address = $2::inet and e.port = $3 \
         order by e.is_current desc, e.last_seen desc, e.created_at, e.id \
         limit 1",
    )
    .bind(device_id)
    .bind(address.to_string())
    .bind(i32::from(port))
    .fetch_optional(&mut **tx)
    .await?;

    let (
        service_id,
        endpoint_id,
        previous_protocol,
        endpoint_current,
        manually_confirmed,
        created_service,
    ) = if let Some((service_id, endpoint_id, protocol, endpoint_current, manually_confirmed)) =
        existing
    {
        (
            service_id,
            endpoint_id,
            protocol,
            endpoint_current,
            manually_confirmed,
            false,
        )
    } else {
        let service_name = format!(
            "{} {}:{}",
            result.protocol.as_str().to_ascii_uppercase(),
            address,
            port
        );
        let (service_id,): (Uuid,) = sqlx::query_as(
            "insert into services (name, protocol, owner_kind, owner_id) \
             values ($1, $2, 'device', $3) returning id",
        )
        .bind(service_name)
        .bind(result.protocol.as_str())
        .bind(device_id)
        .fetch_one(&mut **tx)
        .await?;
        let (endpoint_id,): (Uuid,) = sqlx::query_as(
            "insert into endpoints (service_id, endpoint_type, address, port, is_current) \
             values ($1, 'socket', $2::inet, $3, true) returning id",
        )
        .bind(service_id)
        .bind(address.to_string())
        .bind(i32::from(port))
        .fetch_one(&mut **tx)
        .await?;
        (service_id, endpoint_id, None, true, false, true)
    };

    if !endpoint_current {
        sqlx::query(
            "update endpoints set is_current = true, last_seen = now(), \
                version = version + 1, updated_at = now() where id = $1",
        )
        .bind(endpoint_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query("update endpoints set last_seen = now(), updated_at = now() where id = $1")
            .bind(endpoint_id)
            .execute(&mut **tx)
            .await?;
    }

    let auto_classification_exists: (bool,) = sqlx::query_as(
        "select exists( \
             select 1 from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2)",
    )
    .bind(service_id)
    .bind(CLASSIFICATION_EVIDENCE_ATTRIBUTE)
    .fetch_one(&mut **tx)
    .await?;
    let classification_value = classification_evidence_value(result, service_id, endpoint_id);
    let source_instance = run_id.to_string();
    let existing_evidence: Option<(Uuid,)> = sqlx::query_as(
        "select id from evidence \
         where subject_table = 'services' and subject_id = $1 \
           and source_type = 'network_scan' and source_instance = $2 \
           and attribute = $3 and value = $4 and not absent \
         order by created_at desc, id desc limit 1",
    )
    .bind(service_id)
    .bind(&source_instance)
    .bind(CLASSIFICATION_EVIDENCE_ATTRIBUTE)
    .bind(&classification_value)
    .fetch_optional(&mut **tx)
    .await?;
    if existing_evidence.is_none() {
        evidence::record_automatic_tx(
            tx,
            "services",
            service_id,
            "network_scan",
            Some(&source_instance),
            CLASSIFICATION_EVIDENCE_ATTRIBUTE,
            &classification_value,
            result.confidence,
            false,
        )
        .await?;
    }

    let should_update_protocol = !created_service
        && !manually_confirmed
        && (previous_protocol.is_none() || auto_classification_exists.0)
        && previous_protocol.as_deref() != Some(result.protocol.as_str());
    if should_update_protocol {
        sqlx::query(
            "update services set protocol = $2, version = version + 1, updated_at = now() \
             where id = $1",
        )
        .bind(service_id)
        .bind(result.protocol.as_str())
        .execute(&mut **tx)
        .await?;
    }

    if created_service {
        Recorder::record_change(
            tx,
            "services",
            service_id,
            "service.classified",
            "notice",
            None,
            Some(json!({
                "protocol": result.protocol.as_str(),
                "endpoint_id": endpoint_id,
                "address": address.to_string(),
                "port": port,
            })),
            Some("network_scan"),
        )
        .await?;
    } else if should_update_protocol {
        Recorder::record_change(
            tx,
            "services",
            service_id,
            "service.protocol_changed",
            "notice",
            Some(json!({
                "protocol": previous_protocol,
                "endpoint_id": endpoint_id,
                "address": address.to_string(),
                "port": port,
            })),
            Some(json!({
                "protocol": result.protocol.as_str(),
                "endpoint_id": endpoint_id,
                "address": address.to_string(),
                "port": port,
            })),
            Some("network_scan"),
        )
        .await?;
    }
    Ok(())
}

fn classification_evidence_value(
    result: &ClassificationResult,
    service_id: Uuid,
    endpoint_id: Uuid,
) -> Value {
    let mut value = result.evidence.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("service_id".to_string(), json!(service_id));
        object.insert("endpoint_id".to_string(), json!(endpoint_id));
    }
    value
}

fn observation_proves_liveness(observation: &PortObservation) -> bool {
    matches!(observation.state, PortState::Open | PortState::Closed)
}

async fn attach_run_observations_to_device(
    pool: &PgPool,
    run_id: Uuid,
    address: IpAddr,
    device_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "update port_observations set device_id = $3 \
         where scan_run_id = $1 and address = $2::inet and device_id is null",
    )
    .bind(run_id)
    .bind(address.to_string())
    .bind(device_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn complete_run(
    pool: &PgPool,
    run_id: Uuid,
    targets: &[IpAddr],
    ports: &[u16],
) -> Result<()> {
    let target_addresses: Vec<String> = targets.iter().map(ToString::to_string).collect();
    let target_address_set: HashSet<String> = target_addresses.iter().cloned().collect();
    let mut tx = pool.begin().await?;

    let open_observations: Vec<(String, i32, String)> = sqlx::query_as(
        "select host(address), port, transport from port_observations \
         where scan_run_id = $1 and state = 'open'",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    let current_open: HashSet<(String, i32, String)> = open_observations.into_iter().collect();

    let scanned_device_rows: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "select distinct host(address), device_id from port_observations \
         where scan_run_id = $1 and transport = 'tcp' \
           and state in ('open', 'closed')",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut scanned_device_ids = HashMap::new();
    let mut ambiguous_addresses = HashSet::new();
    for (address, device_id) in scanned_device_rows {
        let Some(device_id) = device_id else {
            ambiguous_addresses.insert(address);
            continue;
        };
        if let Some(previous) = scanned_device_ids.insert(address.clone(), device_id)
            && previous != device_id
        {
            ambiguous_addresses.insert(address);
        }
    }

    let previous_open: Vec<(Uuid, Value, bool)> = sqlx::query_as(
        "select distinct on (subject_id, value) subject_id, value, absent \
         from evidence \
         where subject_table = 'devices' and source_type = 'network_scan' \
           and attribute = $1 \
           and value->>'address' = any($2::text[]) \
         order by subject_id, value, last_seen desc, created_at desc, id desc",
    )
    .bind(PORT_EVIDENCE_ATTRIBUTE)
    .bind(&target_addresses)
    .fetch_all(&mut *tx)
    .await?;

    let source_instance = run_id.to_string();
    let mut canonical_ids = HashMap::new();
    for (device_id, value, absent) in previous_open {
        if absent {
            continue;
        }
        let Some(address) = value.get("address").and_then(Value::as_str) else {
            continue;
        };
        let Some(port) = value
            .get("port")
            .and_then(Value::as_i64)
            .and_then(|port| i32::try_from(port).ok())
        else {
            continue;
        };
        let Some(transport) = value.get("transport").and_then(Value::as_str) else {
            continue;
        };
        if !target_address_set.contains(address)
            || !ports.iter().any(|candidate| i32::from(*candidate) == port)
        {
            continue;
        }
        if ambiguous_addresses.contains(address) {
            continue;
        }
        let Some(scanned_device_id) = scanned_device_ids.get(address).copied() else {
            continue;
        };
        let scanned_device_id =
            canonical_device_id_in_tx(&mut tx, scanned_device_id, &mut canonical_ids).await?;
        let evidence_device_id =
            canonical_device_id_in_tx(&mut tx, device_id, &mut canonical_ids).await?;
        if scanned_device_id != evidence_device_id {
            continue;
        }
        let key = (address.to_string(), port, transport.to_string());
        if current_open.contains(&key) {
            continue;
        }

        let evidence_id = evidence::record_automatic_tx(
            &mut tx,
            "devices",
            device_id,
            "network_scan",
            Some(&source_instance),
            PORT_EVIDENCE_ATTRIBUTE,
            &value,
            PORT_EVIDENCE_CONFIDENCE,
            true,
        )
        .await?;
        Recorder::record_change(
            &mut tx,
            "devices",
            device_id,
            "port.closed",
            "notice",
            Some(port_snapshot(&value, "open")),
            Some(port_snapshot(&value, "closed")),
            Some("network_scan"),
        )
        .await?;
        sqlx::query(
            "update port_observations set evidence_id = $5 \
             where scan_run_id = $1 and address = $2::inet and port = $3 and transport = $4",
        )
        .bind(run_id)
        .bind(address)
        .bind(port)
        .bind(transport)
        .bind(evidence_id)
        .execute(&mut *tx)
        .await?;
        if transport == "tcp" {
            close_service_endpoints(&mut tx, device_id, address, port).await?;
        }
    }

    let updated = sqlx::query(
        "update scan_runs set status = 'succeeded', complete = true, finished_at = now(), \
            error = null, updated_at = now() \
         where id = $1 and status = 'running' and not complete \
           and not cancellation_requested \
           and targets_completed = targets_planned and ports_completed = ports_planned",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(anyhow!(
            "scan run completion rejected before all probes completed"
        ));
    }
    tx.commit().await?;
    Ok(())
}

async fn close_service_endpoints(
    tx: &mut Transaction<'_, Postgres>,
    device_id: Uuid,
    address: &str,
    port: i32,
) -> Result<()> {
    sqlx::query(
        "update endpoints e set is_current = false, last_seen = now(), \
            version = version + 1, updated_at = now() \
         from services s \
         where e.service_id = s.id and s.owner_kind = 'device' and s.owner_id = $1 \
           and e.endpoint_type = 'socket' and e.address = $2::inet and e.port = $3 \
           and e.is_current",
    )
    .bind(device_id)
    .bind(address)
    .bind(port)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn canonical_device_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    cache: &mut HashMap<Uuid, Uuid>,
) -> Result<Uuid> {
    if let Some(canonical_id) = cache.get(&id).copied() {
        return Ok(canonical_id);
    }

    let mut current = id;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return Err(anyhow!(
                "device canonical redirect cycle includes {current}"
            ));
        }
        let next: Option<(Option<Uuid>,)> =
            sqlx::query_as("select canonical_of from devices where id = $1 for key share")
                .bind(current)
                .fetch_optional(&mut **tx)
                .await?;
        match next {
            Some((Some(canonical_of),)) if canonical_of != current => current = canonical_of,
            _ => break,
        }
    }
    cache.insert(id, current);
    Ok(current)
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

async fn stop_requested(
    pool: &PgPool,
    job_id: Uuid,
    run_id: Uuid,
    control: &WorkerControl,
) -> Result<Option<StopReason>> {
    if let Some(reason) = control.stop_reason() {
        return Ok(Some(reason));
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
        control.request_cancellation();
        return Ok(Some(StopReason::Cancelled));
    }
    Ok(control.stop_reason())
}

async fn report_progress(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    run: &ScanRun,
    status: &str,
    progress: &JobProgress,
) -> Result<()> {
    let (targets_completed, ports_completed) = progress.snapshot();
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
    progress: &JobProgress,
) -> Result<JobOutcome> {
    mark_cancelled(pool, run.id).await?;
    report_progress(pool, job_id, worker_id, run, "cancelled", progress).await?;
    Ok(JobOutcome::Cancelled)
}

async fn finish_stop(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: &str,
    run: &ScanRun,
    progress: &JobProgress,
    reason: StopReason,
) -> Result<JobOutcome> {
    match reason {
        StopReason::Cancelled => finish_cancelled(pool, job_id, worker_id, run, progress).await,
        StopReason::LeaseLost => Err(anyhow!("job lease lost while scanning")),
    }
}

async fn async_fail_run<T>(pool: &PgPool, run_id: Uuid, error: anyhow::Error) -> Result<T> {
    mark_failed(pool, run_id, &error.to_string()).await?;
    Err(error)
}

struct CancellationMonitor {
    task: JoinHandle<()>,
}

impl CancellationMonitor {
    fn start(pool: PgPool, job_id: Uuid, run_id: Uuid, control: Arc<WorkerControl>) -> Self {
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(CANCELLATION_POLL_INTERVAL).await;
                if control.stop_requested() {
                    break;
                }
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
                            control.request_cancellation();
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
    use std::net::{Ipv4Addr, UdpSocket};
    use std::process::Command;
    use std::sync::atomic::AtomicUsize;

    use super::super::classification::ServiceProtocol;
    use tokio::net::TcpStream;
    use tokio::time::{sleep, timeout};

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

    #[test]
    fn classification_heartbeat_budget_stays_well_inside_job_lease() {
        assert!(
            JOB_HEARTBEAT_INTERVAL + JOB_HEARTBEAT_TIMEOUT
                < Duration::from_secs(JOB_LEASE_SECS as u64)
        );
    }

    #[derive(Clone)]
    struct BoundedTestClassifier {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
        first_wave: Arc<tokio::sync::Barrier>,
    }

    #[async_trait]
    impl OpenPortClassifier for BoundedTestClassifier {
        async fn classify(&self, address: IpAddr, port: u16) -> ClassificationResult {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            let wave = self.started.fetch_add(1, Ordering::AcqRel) + 1;
            if wave <= CLASSIFICATION_CONCURRENCY {
                self.first_wave.wait().await;
            }
            let delay = if port.is_multiple_of(2) { 20 } else { 1 };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            self.active.fetch_sub(1, Ordering::AcqRel);
            ClassificationResult::generic_fallback(address, port, None)
        }
    }

    #[tokio::test]
    async fn classifications_run_concurrently_with_fixed_bound_and_input_order() {
        let address: IpAddr = "192.0.2.20".parse().expect("test address");
        let device_id = Uuid::new_v4();
        let port_count = CLASSIFICATION_CONCURRENCY * 2;
        let observations: Vec<_> = (0..port_count)
            .map(|index| PortObservation {
                address,
                port: 10_000 + index as u16,
                state: PortState::Open,
                latency: Duration::from_millis(1),
            })
            .collect();
        let device_ids = HashMap::from([(address, device_id)]);
        let classifier = BoundedTestClassifier {
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(AtomicUsize::new(0)),
            first_wave: Arc::new(tokio::sync::Barrier::new(CLASSIFICATION_CONCURRENCY)),
        };
        let max_active = Arc::clone(&classifier.max_active);
        let batch = tokio::time::timeout(
            Duration::from_secs(1),
            classify_open_observations(
                &classifier,
                &observations,
                &device_ids,
                Arc::new(WorkerControl::default()),
            ),
        )
        .await
        .expect("bounded classifications finish");

        assert!(!batch.cancelled);
        assert_eq!(
            max_active.load(Ordering::Acquire),
            CLASSIFICATION_CONCURRENCY
        );
        assert_eq!(batch.classifications.len(), port_count);
        let ports: Vec<_> = batch
            .classifications
            .iter()
            .map(|classification| classification.port)
            .collect();
        assert_eq!(
            ports,
            observations
                .iter()
                .map(|observation| observation.port)
                .collect::<Vec<_>>()
        );
    }

    #[derive(Clone, Default)]
    struct BlockingTestClassifier {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl OpenPortClassifier for BlockingTestClassifier {
        async fn classify(&self, _address: IpAddr, _port: u16) -> ClassificationResult {
            self.calls.fetch_add(1, Ordering::AcqRel);
            std::future::pending::<ClassificationResult>().await
        }
    }

    #[tokio::test]
    async fn cancellation_aborts_in_flight_classification_and_keeps_generic_fallbacks() {
        let address: IpAddr = "192.0.2.21".parse().expect("test address");
        let device_id = Uuid::new_v4();
        let port_count = CLASSIFICATION_CONCURRENCY + 2;
        let observations: Vec<_> = (0..port_count)
            .map(|index| PortObservation {
                address,
                port: 11_000 + index as u16,
                state: PortState::Open,
                latency: Duration::from_millis(1),
            })
            .collect();
        let device_ids = HashMap::from([(address, device_id)]);
        let classifier = BlockingTestClassifier::default();
        let control = Arc::new(WorkerControl::default());
        let calls = Arc::clone(&classifier.calls);
        let classify_task = tokio::spawn({
            let classifier = classifier.clone();
            let control = Arc::clone(&control);
            async move {
                classify_open_observations(&classifier, &observations, &device_ids, control).await
            }
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::Acquire) < CLASSIFICATION_CONCURRENCY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("classification fan-out starts bounded first wave");
        control.request_cancellation();

        let batch = tokio::time::timeout(Duration::from_millis(100), classify_task)
            .await
            .expect("cancellation stops classification promptly")
            .expect("classification task joins");
        assert!(batch.cancelled);
        assert_eq!(batch.classifications.len(), port_count);
        assert!(batch
            .classifications
            .iter()
            .all(|classification| classification.result.protocol == ServiceProtocol::GenericTcp));
        assert_eq!(calls.load(Ordering::Acquire), CLASSIFICATION_CONCURRENCY);
    }

    #[tokio::test]
    async fn db_open_classification_reuses_canonical_service_and_evidence() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let octets = Uuid::new_v4().into_bytes();
        let address = format!("10.{}.{}.{}", octets[0], octets[1], octets[2]);
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .expect("create classification device");
        let (interface_id,): (Uuid,) =
            sqlx::query_as("insert into interfaces (device_id) values ($1) returning id")
                .bind(device_id)
                .fetch_one(&pool)
                .await
                .expect("create classification interface");
        sqlx::query(
            "insert into addresses (interface_id, ip, is_current) values ($1, $2::inet, true)",
        )
        .bind(interface_id)
        .bind(&address)
        .execute(&pool)
        .await
        .expect("create classification address");

        let result = ClassificationResult {
            protocol: ServiceProtocol::Http,
            confidence: 0.98,
            evidence: json!({
                "protocol": "http",
                "transport": "tcp",
                "address": address,
                "port": 18080,
                "probe": "http_get",
                "status": 200,
            }),
        };
        let run_id = Uuid::new_v4();
        let ip = address.parse().expect("classification IP");
        let mut tx = pool.begin().await.expect("begin classification tx");
        reconcile_service_classification(&mut tx, run_id, device_id, ip, 18080, &result)
            .await
            .expect("persist first classification");
        reconcile_service_classification(&mut tx, run_id, device_id, ip, 18080, &result)
            .await
            .expect("persist repeated classification");
        tx.commit().await.expect("commit classifications");

        let services: (i64,) = sqlx::query_as(
            "select count(*) from services where owner_kind = 'device' and owner_id = $1",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("count classification services");
        assert_eq!(services.0, 1);
        let endpoints: (i64,) = sqlx::query_as(
            "select count(*) from endpoints e join services s on s.id = e.service_id \
             where s.owner_kind = 'device' and s.owner_id = $1 and e.address = $2::inet \
               and e.port = 18080 and e.is_current",
        )
        .bind(device_id)
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("count classification endpoints");
        assert_eq!(endpoints.0, 1);
        let evidence_rows: (i64,) = sqlx::query_as(
            "select count(*) from evidence where subject_table = 'services' \
             and source_type = 'network_scan' and source_instance = $1 \
             and attribute = $2",
        )
        .bind(run_id.to_string())
        .bind(CLASSIFICATION_EVIDENCE_ATTRIBUTE)
        .fetch_one(&pool)
        .await
        .expect("count classification evidence");
        assert_eq!(evidence_rows.0, 1);
        let events: (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_kind = 'services' \
             and category = 'service.classified'",
        )
        .fetch_one(&pool)
        .await
        .expect("count classification events");
        assert_eq!(events.0, 1);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL and a local Docker daemon"]
    async fn m2_docker_high_port_scan_creates_canonical_inventory_records() {
        if !docker_daemon_available() {
            eprintln!("skipping: Docker daemon unavailable");
            return;
        }

        let database_url =
            std::env::var("DATABASE_URL").expect("M2 Docker acceptance gate requires DATABASE_URL");
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let host_ip = private_host_address()
            .expect("M2 Docker acceptance gate needs a private host interface address");
        let target = host_ip.to_string();
        let container = DockerHttpContainer::start(host_ip)
            .unwrap_or_else(|error| panic!("start Docker HTTP fixture: {error}"));
        container.wait_until_ready().await;
        assert!(
            container.host_port >= 1024,
            "Docker assigned privileged host port {}",
            container.host_port
        );
        let ports = [container.host_port];
        let port_value = json!({
            "address": &target,
            "port": container.host_port,
            "transport": "tcp",
        });

        let (network_id, job_id, run_id) = test_run_for_address(&pool, &target, 1).await;
        let outcome = handle_inner(
            &pool,
            job_id,
            "test-worker",
            json!({"network_id": network_id}),
            Some(&ports),
        )
        .await
        .expect("scan Docker-published HTTP port");
        assert_eq!(outcome, JobOutcome::Completed);

        let run_state: (String, bool, i64, i64) = sqlx::query_as(
            "select status, complete, targets_completed, ports_completed \
             from scan_runs where id = $1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read first acceptance scan");
        assert_eq!(run_state, ("succeeded".to_string(), true, 1, 1));

        let (device_id, state): (Uuid, String) = sqlx::query_as(
            "select device_id, state from port_observations \
             where scan_run_id = $1 and address = $2::inet and port = $3",
        )
        .bind(run_id)
        .bind(&target)
        .bind(i32::from(container.host_port))
        .fetch_one(&pool)
        .await
        .expect("read first port observation");
        assert_eq!(state, "open");

        let (open_evidence,): (i64,) = sqlx::query_as(
            "select count(*) from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and source_instance = $2 \
               and attribute = 'open_port' and value = $3 and not absent",
        )
        .bind(device_id)
        .bind(run_id.to_string())
        .bind(&port_value)
        .fetch_one(&pool)
        .await
        .expect("read first open-port evidence");
        assert_eq!(open_evidence, 1);

        let (service_count,): (i64,) = sqlx::query_as(
            "select count(distinct s.id) from services s \
             join endpoints e on e.service_id = s.id \
             where s.owner_kind = 'device' and s.owner_id = $1 \
               and e.address = $2::inet and e.port = $3",
        )
        .bind(device_id)
        .bind(&target)
        .bind(i32::from(container.host_port))
        .fetch_one(&pool)
        .await
        .expect("count first canonical services");
        assert_eq!(service_count, 1);

        let (endpoint_count,): (i64,) = sqlx::query_as(
            "select count(*) from endpoints e \
             join services s on s.id = e.service_id \
             where s.owner_kind = 'device' and s.owner_id = $1 \
               and e.endpoint_type = 'socket' and e.address = $2::inet \
               and e.port = $3 and e.is_current",
        )
        .bind(device_id)
        .bind(&target)
        .bind(i32::from(container.host_port))
        .fetch_one(&pool)
        .await
        .expect("count first canonical endpoint");
        assert_eq!(endpoint_count, 1);

        let (service_id, protocol, endpoint_id, endpoint_type, endpoint_address, endpoint_port): (
            Uuid,
            String,
            Uuid,
            String,
            String,
            i32,
        ) = sqlx::query_as(
            "select s.id, s.protocol, e.id, e.endpoint_type, host(e.address), e.port \
             from services s join endpoints e on e.service_id = s.id \
             where s.owner_kind = 'device' and s.owner_id = $1 \
               and e.address = $2::inet and e.port = $3 and e.is_current",
        )
        .bind(device_id)
        .bind(&target)
        .bind(i32::from(container.host_port))
        .fetch_one(&pool)
        .await
        .expect("read first canonical service endpoint");
        assert_eq!(protocol, "http");
        assert_eq!(endpoint_type, "socket");
        assert_eq!(endpoint_address, target);
        assert_eq!(endpoint_port, i32::from(container.host_port));

        let (classification_evidence, evidence_protocol): (i64, Option<String>) = sqlx::query_as(
            "select count(*), max(value->>'protocol') from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'network_scan' and source_instance = $2 \
               and attribute = 'protocol_classification' and not absent",
        )
        .bind(service_id)
        .bind(run_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("read first protocol evidence");
        assert_eq!(classification_evidence, 1);
        assert_eq!(evidence_protocol.as_deref(), Some("http"));

        let (classified_events,): (i64,) = sqlx::query_as(
            "select count(*) from change_events \
             where entity_kind = 'services' and entity_id = $1 \
               and category = 'service.classified'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("count first service classification events");
        assert_eq!(classified_events, 1);

        let (repeat_network_id, repeat_job_id, repeat_run_id) =
            test_run_for_address(&pool, &target, 1).await;
        let repeat_outcome = handle_inner(
            &pool,
            repeat_job_id,
            "test-worker",
            json!({"network_id": repeat_network_id}),
            Some(&ports),
        )
        .await
        .expect("repeat scan Docker-published HTTP port");
        assert_eq!(repeat_outcome, JobOutcome::Completed);

        let (device_count,): (i64,) = sqlx::query_as(
            "select count(distinct d.id) from devices d \
             join interfaces i on i.device_id = d.id \
             join addresses a on a.interface_id = i.id \
             where a.ip = $1::inet",
        )
        .bind(&target)
        .fetch_one(&pool)
        .await
        .expect("count repeat-scan devices");
        assert_eq!(device_count, 1);

        let (service_count, endpoint_count, current_endpoint_count): (i64, i64, i64) =
            sqlx::query_as(
                "select count(distinct s.id), count(distinct e.id), \
                        count(distinct e.id) filter (where e.is_current) \
                 from services s join endpoints e on e.service_id = s.id \
                 where s.owner_kind = 'device' and s.owner_id = $1 \
                   and e.address = $2::inet and e.port = $3",
            )
            .bind(device_id)
            .bind(&target)
            .bind(i32::from(container.host_port))
            .fetch_one(&pool)
            .await
            .expect("count repeat-scan canonical records");
        assert_eq!(
            (service_count, endpoint_count, current_endpoint_count),
            (1, 1, 1)
        );

        let (repeat_service_id, repeat_protocol, repeat_endpoint_id): (Uuid, String, Uuid) =
            sqlx::query_as(
                "select s.id, s.protocol, e.id from services s \
                 join endpoints e on e.service_id = s.id \
                 where s.owner_kind = 'device' and s.owner_id = $1 \
                   and e.address = $2::inet and e.port = $3 and e.is_current",
            )
            .bind(device_id)
            .bind(&target)
            .bind(i32::from(container.host_port))
            .fetch_one(&pool)
            .await
            .expect("read repeat-scan canonical records");
        assert_eq!(repeat_service_id, service_id);
        assert_eq!(repeat_endpoint_id, endpoint_id);
        assert_eq!(repeat_protocol, "http");

        let (first_scan_evidence, repeat_scan_evidence): (i64, i64) = sqlx::query_as(
            "select \
                (select count(*) from evidence \
                 where subject_table = 'services' and subject_id = $1 \
                   and source_type = 'network_scan' and source_instance = $2 \
                   and attribute = 'protocol_classification' and not absent), \
                (select count(*) from evidence \
                 where subject_table = 'services' and subject_id = $1 \
                   and source_type = 'network_scan' and source_instance = $3 \
                   and attribute = 'protocol_classification' and not absent)",
        )
        .bind(service_id)
        .bind(run_id.to_string())
        .bind(repeat_run_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("count repeat-scan protocol evidence");
        assert_eq!((first_scan_evidence, repeat_scan_evidence), (1, 1));

        let (classified_events,): (i64,) = sqlx::query_as(
            "select count(*) from change_events \
             where entity_kind = 'services' and entity_id = $1 \
               and category = 'service.classified'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("count repeat-scan service classification events");
        assert_eq!(classified_events, 1);

        for scan_run_id in [run_id, repeat_run_id] {
            let (observations, open_evidence): (i64, i64) = sqlx::query_as(
                "select \
                    (select count(*) from port_observations \
                     where scan_run_id = $1 and address = $2::inet and port = $3 \
                       and state = 'open'), \
                    (select count(*) from evidence \
                     where subject_table = 'devices' and subject_id = $4 \
                       and source_type = 'network_scan' and source_instance = $5 \
                       and attribute = 'open_port' and value = $6 and not absent)",
            )
            .bind(scan_run_id)
            .bind(&target)
            .bind(i32::from(container.host_port))
            .bind(device_id)
            .bind(scan_run_id.to_string())
            .bind(&port_value)
            .fetch_one(&pool)
            .await
            .expect("read repeat-scan open-port records");
            assert_eq!((observations, open_evidence), (1, 1));
        }
    }

    fn docker_daemon_available() -> bool {
        Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn private_host_address() -> Option<Ipv4Addr> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 80)).ok()?;
        let address = socket.local_addr().ok()?.ip();
        match address {
            IpAddr::V4(address) if is_rfc1918(address) => Some(address),
            _ => None,
        }
    }

    fn is_rfc1918(address: Ipv4Addr) -> bool {
        let octets = address.octets();
        (octets[0] == 10)
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168)
    }

    struct DockerHttpContainer {
        id: String,
        host_ip: Ipv4Addr,
        host_port: u16,
    }

    impl DockerHttpContainer {
        fn start(host_ip: Ipv4Addr) -> std::result::Result<Self, String> {
            let name = format!("hope-m2-{}", Uuid::new_v4());
            let publish = format!("{host_ip}::80");
            let output = Command::new("docker")
                .args([
                    "run",
                    "--detach",
                    "--rm",
                    "--pull=missing",
                    "--name",
                    &name,
                    "--publish",
                    &publish,
                    "nginx:1.27-alpine",
                ])
                .output()
                .map_err(|error| format!("run Docker: {error}"))?;
            if !output.status.success() {
                remove_docker_container(&name);
                return Err(format!(
                    "docker run failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }

            let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if id.is_empty() {
                remove_docker_container(&name);
                return Err("docker run returned no container ID".to_string());
            }
            let mut container = Self {
                id,
                host_ip,
                host_port: 0,
            };
            let host_port = container.mapped_port()?;
            container.host_port = host_port;
            Ok(container)
        }

        fn mapped_port(&self) -> std::result::Result<u16, String> {
            let output = Command::new("docker")
                .args(["port", &self.id, "80/tcp"])
                .output()
                .map_err(|error| format!("inspect Docker port: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "docker port failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find_map(|line| line.rsplit(':').next()?.trim().parse().ok())
                .ok_or_else(|| {
                    format!(
                        "docker port returned no host port: {}",
                        String::from_utf8_lossy(&output.stdout).trim()
                    )
                })
        }

        async fn wait_until_ready(&self) {
            let address = SocketAddr::from((self.host_ip, self.host_port));
            for _ in 0..100 {
                let connected = timeout(Duration::from_millis(250), TcpStream::connect(address))
                    .await
                    .is_ok_and(|result| result.is_ok());
                if connected {
                    return;
                }
                sleep(Duration::from_millis(100)).await;
            }
            panic!(
                "Docker HTTP fixture did not become reachable on {}:{}",
                self.host_ip, self.host_port
            );
        }
    }

    impl Drop for DockerHttpContainer {
        fn drop(&mut self) {
            remove_docker_container(&self.id);
        }
    }

    fn remove_docker_container(id_or_name: &str) {
        let _ = Command::new("docker")
            .args(["rm", "--force", "--volumes", id_or_name])
            .status();
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
        let mut device_ids = HashMap::new();
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
                device_ids: &mut device_ids,
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
    async fn db_filtered_only_run_keeps_observations_without_discovering_address() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let octets = Uuid::new_v4().into_bytes();
        let address = format!("10.{}.{}.{}", octets[0], octets[1], octets[2]);
        let target: IpAddr = address.parse().expect("test target");
        let (_, job_id, run_id) = test_run_for_address(&pool, &address, 2).await;
        let run = load_run(&pool, job_id)
            .await
            .expect("load filtered test run");
        let scanner = FakeScanner {
            filtered_ports: [80, 443].into_iter().collect(),
            ..FakeScanner::default()
        };
        let mut device_ids = HashMap::new();

        execute_run(
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
                device_ids: &mut device_ids,
            },
        )
        .await
        .expect("complete filtered test run");

        let run_state: (String, bool, i64, i64) = sqlx::query_as(
            "select status, complete, targets_completed, ports_completed from scan_runs where id = $1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read filtered run");
        assert_eq!(run_state, ("succeeded".to_string(), true, 1, 2));

        let observations: (i64, i64) = sqlx::query_as(
            "select count(*), count(*) filter (where device_id is null) \
             from port_observations where scan_run_id = $1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read filtered observations");
        assert_eq!(observations, (2, 2));
        assert!(device_ids.is_empty());

        let addresses: (i64,) =
            sqlx::query_as("select count(*) from addresses where ip = $1::inet")
                .bind(&address)
                .fetch_one(&pool)
                .await
                .expect("count filtered addresses");
        assert_eq!(addresses.0, 0);
        let evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence \
             where source_type = 'network_scan' and value->>'address' = $1",
        )
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("count filtered evidence");
        assert_eq!(evidence.0, 0);
    }

    #[tokio::test]
    async fn db_closed_then_open_discovery_refreshes_one_device() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let octets = Uuid::new_v4().into_bytes();
        let address = format!("10.{}.{}.{}", octets[0], octets[1], octets[2]);
        let target: IpAddr = address.parse().expect("test target");

        let (_, first_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        let first_run = load_run(&pool, first_job_id)
            .await
            .expect("load closed test run");
        let first_scanner = FakeScanner::default();
        let mut first_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: first_job_id,
                worker_id: "test-worker",
            },
            first_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &first_scanner,
                device_ids: &mut first_device_ids,
            },
        )
        .await
        .expect("complete closed test run");

        let (device_id,): (Uuid,) = sqlx::query_as(
            "select d.id from devices d \
             join interfaces i on i.device_id = d.id \
             join addresses a on a.interface_id = i.id \
             where a.ip = $1::inet and a.is_current",
        )
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("resolve closed device");

        let (_, second_job_id, second_run_id) = test_run_for_address(&pool, &address, 2).await;
        let second_run = load_run(&pool, second_job_id)
            .await
            .expect("load open refresh test run");
        let second_scanner = FakeScanner {
            open_port: Some(443),
            ..FakeScanner::default()
        };
        let mut second_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: second_job_id,
                worker_id: "test-worker",
            },
            second_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &second_scanner,
                device_ids: &mut second_device_ids,
            },
        )
        .await
        .expect("complete open refresh test run");

        assert_eq!(second_device_ids.get(&target), Some(&device_id));
        let device_count: (i64,) = sqlx::query_as(
            "select count(distinct d.id) from devices d \
             join interfaces i on i.device_id = d.id \
             join addresses a on a.interface_id = i.id \
             where a.ip = $1::inet",
        )
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("count refreshed devices");
        assert_eq!(device_count.0, 1);

        let open_evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2 \
               and value->>'address' = $3 and not absent",
        )
        .bind(device_id)
        .bind(PORT_EVIDENCE_ATTRIBUTE)
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("count refreshed open evidence");
        assert_eq!(open_evidence.0, 1);
        let observations: (i64, i64) = sqlx::query_as(
            "select count(*), count(*) filter (where device_id = $2) \
             from port_observations where scan_run_id = $1",
        )
        .bind(second_run_id)
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("read refreshed observations");
        assert_eq!(observations, (2, 2));
    }

    #[tokio::test]
    async fn db_scan_application_emits_deduplicated_open_and_complete_close_events() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let octets = Uuid::new_v4().into_bytes();
        let address = format!("10.{}.{}.{}", octets[0], octets[1], octets[2]);
        let target: IpAddr = address.parse().expect("test target");
        let port_value = json!({
            "address": address,
            "port": 80,
            "transport": "tcp",
        });

        let (_, job_id, run_id) = test_run_for_address(&pool, &address, 2).await;
        let run = load_run(&pool, job_id).await.expect("load open test run");
        let scanner = FakeScanner {
            open_port: Some(80),
            ..FakeScanner::default()
        };
        let mut device_ids = HashMap::new();
        execute_run(
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
                device_ids: &mut device_ids,
            },
        )
        .await
        .expect("complete open test run");

        let (device_id,): (Uuid,) = sqlx::query_as(
            "select d.id from devices d \
             join interfaces i on i.device_id = d.id \
             join addresses a on a.interface_id = i.id \
             where a.ip = $1::inet and a.is_current",
        )
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("resolve scanned device");
        let open_events: (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_id = $1 and category = 'port.opened'",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("count open events");
        assert_eq!(open_events.0, 1, "new open port emits one event");
        let open_evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2 and value = $3 \
               and not absent",
        )
        .bind(device_id)
        .bind(PORT_EVIDENCE_ATTRIBUTE)
        .bind(&port_value)
        .fetch_one(&pool)
        .await
        .expect("count open evidence");
        assert_eq!(open_evidence.0, 1);
        let linked_observation: (bool,) = sqlx::query_as(
            "select evidence_id is not null from port_observations where scan_run_id = $1 and port = 80",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read linked open observation");
        assert!(linked_observation.0);

        let (_, repeat_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        let repeat_run = load_run(&pool, repeat_job_id)
            .await
            .expect("load repeated open test run");
        let repeat_scanner = FakeScanner {
            open_port: Some(80),
            ..FakeScanner::default()
        };
        let mut repeat_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: repeat_job_id,
                worker_id: "test-worker",
            },
            repeat_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &repeat_scanner,
                device_ids: &mut repeat_device_ids,
            },
        )
        .await
        .expect("complete repeated open test run");
        let open_events_after_repeat: (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_id = $1 and category = 'port.opened'",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("count repeated open events");
        assert_eq!(
            open_events_after_repeat.0, 1,
            "refresh does not duplicate event"
        );

        let (_, failed_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        let failed_run = load_run(&pool, failed_job_id)
            .await
            .expect("load partial test run");
        let failed_scanner = FakeScanner {
            open_port: Some(80),
            fail_port: Some(443),
            ..FakeScanner::default()
        };
        let mut failed_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: failed_job_id,
                worker_id: "test-worker",
            },
            failed_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &failed_scanner,
                device_ids: &mut failed_device_ids,
            },
        )
        .await
        .expect_err("partial scanner run must fail");
        let open_after_failure: (bool,) = sqlx::query_as(
            "select absent from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2 and value = $3 \
             order by last_seen desc, created_at desc, id desc limit 1",
        )
        .bind(device_id)
        .bind(PORT_EVIDENCE_ATTRIBUTE)
        .bind(&port_value)
        .fetch_one(&pool)
        .await
        .expect("read evidence after failed run");
        assert!(!open_after_failure.0, "failed run cannot close port");

        let (_, cancelled_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        jobs::request_cancel(&pool, cancelled_job_id)
            .await
            .expect("request cancellation");
        let cancelled_run = load_run(&pool, cancelled_job_id)
            .await
            .expect("load cancelled test run");
        let cancelled_scanner = FakeScanner::default();
        let mut cancelled_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: cancelled_job_id,
                worker_id: "test-worker",
            },
            cancelled_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &cancelled_scanner,
                device_ids: &mut cancelled_device_ids,
            },
        )
        .await
        .expect("cancelled test run");
        let closed_events_before_complete: (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_id = $1 and category = 'port.closed'",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("count closed events before complete run");
        assert_eq!(closed_events_before_complete.0, 0);

        let (_, closing_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        let closing_run = load_run(&pool, closing_job_id)
            .await
            .expect("load closing test run");
        let closing_scanner = FakeScanner::default();
        let mut closing_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: closing_job_id,
                worker_id: "test-worker",
            },
            closing_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &closing_scanner,
                device_ids: &mut closing_device_ids,
            },
        )
        .await
        .expect("complete closing test run");
        let closed_events: (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_id = $1 and category = 'port.closed'",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("count closed events");
        assert_eq!(closed_events.0, 1, "complete run emits one close event");
        let closed_evidence: (bool,) = sqlx::query_as(
            "select absent from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2 and value = $3 \
             order by last_seen desc, created_at desc, id desc limit 1",
        )
        .bind(device_id)
        .bind(PORT_EVIDENCE_ATTRIBUTE)
        .bind(&port_value)
        .fetch_one(&pool)
        .await
        .expect("read closed evidence");
        assert!(closed_evidence.0);
    }

    #[tokio::test]
    async fn db_complete_scan_does_not_close_port_for_former_ip_owner() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let octets = Uuid::new_v4().into_bytes();
        let address = format!("10.{}.{}.{}", octets[0], octets[1], octets[2]);
        let replacement_address = format!(
            "10.{}.{}.{}",
            octets[0],
            octets[1],
            octets[2].wrapping_add(1)
        );
        let target: IpAddr = address.parse().expect("test target");
        let port_value = json!({
            "address": address,
            "port": 80,
            "transport": "tcp",
        });

        let (_, first_job_id, _) = test_run_for_address(&pool, &address, 2).await;
        let first_run = load_run(&pool, first_job_id)
            .await
            .expect("load first test run");
        let first_scanner = FakeScanner {
            open_port: Some(80),
            ..FakeScanner::default()
        };
        let mut first_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: first_job_id,
                worker_id: "test-worker",
            },
            first_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &first_scanner,
                device_ids: &mut first_device_ids,
            },
        )
        .await
        .expect("complete first test run");

        let (former_device_id, former_interface_id): (Uuid, Uuid) = sqlx::query_as(
            "select d.id, i.id from devices d \
             join interfaces i on i.device_id = d.id \
             join addresses a on a.interface_id = i.id \
             where a.ip = $1::inet and a.is_current",
        )
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("resolve former scanned device");
        addresses::assign_address_tx(
            &pool,
            former_interface_id,
            &replacement_address,
            Some("dhcp"),
        )
        .await
        .expect("move former device to replacement address");

        let (new_device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .expect("create replacement device");
        let (new_interface_id,): (Uuid,) =
            sqlx::query_as("insert into interfaces (device_id) values ($1) returning id")
                .bind(new_device_id)
                .fetch_one(&pool)
                .await
                .expect("create replacement interface");
        addresses::assign_address_tx(&pool, new_interface_id, &address, Some("dhcp"))
            .await
            .expect("assign address to replacement device");

        let (_, closing_job_id, closing_run_id) = test_run_for_address(&pool, &address, 2).await;
        let closing_run = load_run(&pool, closing_job_id)
            .await
            .expect("load replacement-owner test run");
        let closing_scanner = FakeScanner::default();
        let mut closing_device_ids = HashMap::new();
        execute_run(
            ExecutionContext {
                pool: &pool,
                job_id: closing_job_id,
                worker_id: "test-worker",
            },
            closing_run,
            ScanPlan {
                targets: &[target],
                ports: &[80, 443],
                scanner: &closing_scanner,
                device_ids: &mut closing_device_ids,
            },
        )
        .await
        .expect("complete replacement-owner test run");

        let (observed_device_id, observation_evidence_id): (Uuid, Option<Uuid>) = sqlx::query_as(
            "select device_id, evidence_id from port_observations \
                 where scan_run_id = $1 and address = $2::inet and port = 80",
        )
        .bind(closing_run_id)
        .bind(&address)
        .fetch_one(&pool)
        .await
        .expect("read replacement-owner observation");
        assert_eq!(observed_device_id, new_device_id);
        assert!(
            observation_evidence_id.is_none(),
            "replacement-owner closed observation must not link former-owner absence evidence"
        );

        let former_close_events: (i64,) = sqlx::query_as(
            "select count(*) from change_events \
             where entity_id = $1 and category = 'port.closed'",
        )
        .bind(former_device_id)
        .fetch_one(&pool)
        .await
        .expect("count former-owner close events");
        assert_eq!(former_close_events.0, 0);
        let former_absence_evidence: (i64,) = sqlx::query_as(
            "select count(*) from evidence \
             where subject_table = 'devices' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = $2 \
               and value = $3 and absent",
        )
        .bind(former_device_id)
        .bind(PORT_EVIDENCE_ATTRIBUTE)
        .bind(&port_value)
        .fetch_one(&pool)
        .await
        .expect("count former-owner absence evidence");
        assert_eq!(former_absence_evidence.0, 0);
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
        let mut device_ids = HashMap::new();
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
                device_ids: &mut device_ids,
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
        let mut device_ids = HashMap::new();
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
                device_ids: &mut device_ids,
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
        test_run_for_address(pool, "192.168.30.10", ports_planned).await
    }

    async fn test_run_for_address(
        pool: &PgPool,
        address: &str,
        ports_planned: i64,
    ) -> (Uuid, Uuid, Uuid) {
        let (network_id,): (Uuid,) =
            sqlx::query_as("insert into networks (cidr, name) values ($1::cidr, $2) returning id")
                .bind(format!("{address}/32"))
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
        open_port: Option<u16>,
        filtered_ports: HashSet<u16>,
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
                state: if self.filtered_ports.contains(&target.port()) {
                    PortState::Filtered
                } else if self.open_port == Some(target.port()) {
                    PortState::Open
                } else {
                    PortState::Closed
                },
                latency: Duration::from_millis(1),
            })
        }
    }
}
