use crate::{
    child_delivery::{self, Outcome as ChildOutcome},
    collect,
    config::{self, Config},
    delivery::{self, Outcome, Spool},
    now,
};
use anyhow::{Context, Result};
use hope_protocol::{
    AgentReport, ChildReportBatch, CollectorStatus, Inventory, MAX_AGENT_REPORT_BYTES, Metrics,
    SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path, pin::Pin, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until},
};
use uuid::Uuid;

pub type Sample = (Inventory, Metrics, Vec<CollectorStatus>);
pub type Sampling<'a> = Pin<Box<dyn Future<Output = Sample> + 'a>>;
pub trait Sampler {
    fn sample(&mut self) -> Sampling<'_>;
    fn child_snapshots(&self) -> Vec<collect::ChildSnapshot> {
        vec![]
    }
}
impl Sampler for collect::Collector {
    fn sample(&mut self) -> Sampling<'_> {
        Box::pin(self.sample())
    }
    fn child_snapshots(&self) -> Vec<collect::ChildSnapshot> {
        collect::Collector::child_snapshots(self)
    }
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Acceptance {
    pub(crate) agent_id: String,
    pub(crate) report_id: String,
    pub(crate) run_id: String,
    pub(crate) process_id: u32,
    pub(crate) operation: String,
    pub(crate) collected_at: f64,
    pub(crate) accepted_at: f64,
    #[serde(default)]
    pub(crate) activation_id: Option<String>,
}
#[derive(Default)]
struct InventoryDelivery {
    fingerprint: String,
    accepted_at: f64,
    accepted_sequence: Option<u64>,
    refreshed_at: Option<Instant>,
}
impl InventoryDelivery {
    fn changed(&self, inventory: &Inventory) -> Result<bool> {
        Ok(self.fingerprint
            != serde_json::to_string(&collect::stable_inventory(inventory.clone()))?
            || self
                .refreshed_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(3600)))
    }
    fn accept(&mut self, report: &AgentReport) -> Result<()> {
        let newer = match (report.sequence, self.accepted_sequence) {
            (Some(sequence), Some(previous)) => sequence >= previous,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => report.collected_at >= self.accepted_at,
        };
        if newer {
            self.accepted_sequence = report.sequence;
            self.accepted_at = report.collected_at;
        }
        if newer && let Some(inventory) = &report.inventory {
            self.fingerprint =
                serde_json::to_string(&collect::stable_inventory(inventory.clone()))?;
            self.refreshed_at = Some(Instant::now());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ChildCounters {
    sequence: u64,
    generation: u64,
    #[serde(default)]
    last_inventory_at: f64,
    #[serde(default)]
    last_report_at: f64,
}
impl Default for ChildCounters {
    fn default() -> Self {
        Self {
            sequence: 0,
            generation: 0,
            last_inventory_at: 0.0,
            last_report_at: 0.0,
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ChildSchedule {
    counters: std::collections::HashMap<String, ChildCounters>,
    disabled: bool,
}
impl ChildSchedule {
    async fn load(dir: &Path) -> Self {
        let path = dir.join("child-sequence.json");
        match tokio::fs::try_exists(&path).await {
            Ok(false) => Self::default(),
            Ok(true) => match config::read_bounded(&path, 4096).await {
                Ok(bytes) => {
                    let valid = |counters: &ChildCounters| {
                        counters.last_inventory_at.is_finite()
                            && counters.last_report_at.is_finite()
                            && counters.sequence <= i64::MAX as u64
                            && counters.generation <= i64::MAX as u64
                    };
                    if let Ok(schedule) = serde_json::from_slice::<Self>(&bytes)
                        && schedule.counters.values().all(valid)
                    {
                        schedule
                    } else if let Ok(legacy) = serde_json::from_slice::<ChildCounters>(&bytes)
                        && valid(&legacy)
                    {
                        Self {
                            counters: [("*".into(), legacy)].into(),
                            disabled: false,
                        }
                    } else {
                        eprintln!(
                            "Child reporting disabled: its sequence state is unreadable; core reporting continues"
                        );
                        Self {
                            disabled: true,
                            ..Self::default()
                        }
                    }
                }
                Err(error) => {
                    eprintln!(
                        "Child reporting disabled: cannot inspect its sequence state: {error}"
                    );
                    Self {
                        disabled: true,
                        ..Self::default()
                    }
                }
            },
            Err(error) => {
                eprintln!("Child reporting disabled: cannot inspect its sequence state: {error}");
                Self {
                    disabled: true,
                    ..Self::default()
                }
            }
        }
    }
    fn batch(
        &mut self,
        snapshot: collect::ChildSnapshot,
        agent_id: &str,
        at: f64,
    ) -> Option<Vec<ChildReportBatch>> {
        let source_observed_at = snapshot.observed_at;
        if self.disabled {
            return None;
        }
        let key = format!("{:?}:{}", snapshot.source, snapshot.source_scope);
        let mut next = self
            .counters
            .get(&key)
            .or_else(|| self.counters.get("*"))
            .cloned()
            .unwrap_or_default();
        if next.last_report_at > 0.0 && at - next.last_report_at < 30.0 {
            return None;
        }
        let first_sequence = next.sequence.checked_add(1)?;
        let inventory_due = next.last_inventory_at == 0.0 || at - next.last_inventory_at >= 60.0;
        if inventory_due {
            next.generation = next.generation.checked_add(1)?;
            next.last_inventory_at = at;
        }
        let complete = inventory_due && snapshot.inventory_complete;
        next.last_report_at = at;
        let inventory_id = inventory_due.then(|| Uuid::new_v4().to_string());
        let batch = ChildReportBatch {
            schema_version: SCHEMA_VERSION,
            batch_id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            source: snapshot.source,
            source_scope: snapshot.source_scope,
            source_token: snapshot.source_token.clone(),
            legacy_source_scope: snapshot.legacy_source_scope.clone(),
            legacy_identities: snapshot.legacy_identities.clone(),
            proxmox_guest_presence: snapshot.proxmox_guest_presence.clone(),
            sequence: first_sequence,
            inventory_generation: next.generation,
            inventory_id: inventory_id.clone(),
            chunk_index: 0,
            chunk_count: 512,
            complete,
            observed_at: source_observed_at,
            devices: Vec::new(),
        };
        // Use the protocol's strict body limit when splitting. Reserving the maximum chunk-count
        // width keeps the sizing valid after the actual count and index are known.
        let mut sizing = batch.clone();
        sizing.sequence = i64::MAX as u64;
        sizing.inventory_generation = i64::MAX as u64;
        sizing.chunk_index = 511;
        sizing.chunk_count = 512;
        let base_size = serde_json::to_vec(&sizing).ok()?.len();
        let mut chunks = vec![batch.clone()];
        let mut current_size = base_size;
        for device in snapshot.devices {
            let device_size = serde_json::to_vec(&device).ok()?.len();
            let current = chunks.last_mut()?;
            let added_size = device_size + usize::from(!current.devices.is_empty());
            if current_size + added_size > MAX_AGENT_REPORT_BYTES {
                if current.devices.is_empty() || base_size + device_size > MAX_AGENT_REPORT_BYTES {
                    return None;
                }
                chunks.push(batch.clone());
                chunks.last_mut()?.batch_id = Uuid::new_v4().to_string();
                current_size = base_size;
            }
            chunks.last_mut()?.devices.push(device);
            current_size += device_size + usize::from(chunks.last()?.devices.len() > 1);
        }
        let group_id =
            inventory_id.or_else(|| (chunks.len() > 1).then(|| Uuid::new_v4().to_string()));
        let chunk_count = chunks.len() as u32;
        for (index, part) in chunks.iter_mut().enumerate() {
            part.sequence = first_sequence.checked_add(index as u64)?;
            part.chunk_index = index as u32;
            part.chunk_count = chunk_count;
            part.inventory_id = group_id.clone();
            part.complete = complete && index + 1 == chunk_count as usize;
            if serde_json::to_vec(part).ok()?.len() > MAX_AGENT_REPORT_BYTES {
                return None;
            }
        }
        next.sequence = first_sequence.checked_add(chunks.len() as u64 - 1)?;
        self.counters.remove("*");
        self.counters.insert(key, next);
        Some(chunks)
    }
}
type Request = Pin<Box<dyn Future<Output = (AgentReport, Outcome)> + Send>>;
type ChildRequest = Pin<Box<dyn Future<Output = (ChildReportBatch, ChildOutcome)> + Send>>;
/// Collection is polled independently of delivery. A bounded channel allows only
/// one completed sample to wait, and this actor remains the sole spool writer.
const MIN_INTERVAL_SECONDS: u32 = 5;
const MAX_INTERVAL_SECONDS: u32 = 300;

/// Spread agents that started together by up to a fifteenth of the interval either side.
fn jittered(period: Duration) -> Duration {
    let spread = period.as_millis() as u64 / 15;
    if period < Duration::from_secs(MIN_INTERVAL_SECONDS.into()) || spread == 0 {
        return period;
    }
    period - Duration::from_millis(spread)
        + Duration::from_millis(rand::random::<u64>() % (2 * spread + 1))
}

async fn sample_loop<C: Sampler>(
    collector: &mut C,
    sender: mpsc::Sender<(Sample, f64, Vec<collect::ChildSnapshot>)>,
    mut period: watch::Receiver<Duration>,
) {
    let mut next_sample = Instant::now();
    loop {
        tokio::select! {
            _ = sleep_until(next_sample) => {}
            changed = period.changed() => {
                if changed.is_err() { return; }
                next_sample = Instant::now() + jittered(*period.borrow_and_update());
                continue;
            }
        }
        let tick = Instant::now();
        let sample = collector.sample().await;
        let collected_at = now();
        let child_snapshots = collector.child_snapshots();
        if sender
            .send((sample, collected_at, child_snapshots))
            .await
            .is_err()
        {
            return;
        }
        let interval = jittered(*period.borrow_and_update());
        next_sample = tick + interval;
        // Skip an overrun instead of repeatedly collecting against an expired deadline.
        if next_sample <= Instant::now() {
            next_sample = Instant::now() + interval;
        }
    }
}

pub async fn run<C: Sampler, S: Future<Output = ()>>(
    collector: &mut C,
    configuration: &Config,
    mut spool: Spool,
    operation: &str,
    dir: &Path,
    period: Duration,
    shutdown: S,
) -> Result<()> {
    if configuration.disabled {
        return Err(delivery::Revoked.into());
    }
    let client = config::client(configuration)?;
    let credential = configuration
        .credential
        .clone()
        .context("Missing agent credential")?;
    let child_queue_path = dir.join("child-spool.json");
    let child_state_path = dir.join("child-sequence.json");
    let child_bytes = config::read_bounded(&child_queue_path, child_delivery::MAX_BYTES)
        .await
        .unwrap_or_default();
    let mut child_spool =
        child_delivery::Spool::decode(&child_bytes, &configuration.agent_id, now());
    let mut child_schedule = ChildSchedule::load(dir).await;
    let run_id = Uuid::new_v4().to_string();
    let activation_id = std::env::var("HOPE_ACTIVATION_ID").ok();
    if let Some(id) = &activation_id {
        Uuid::parse_str(id).context("Invalid service activation identifier")?;
    }
    let (sample_tx, mut samples) = mpsc::channel(1);
    let (period_tx, period_rx) = watch::channel(period);
    let sampling = sample_loop(collector, sample_tx, period_rx);
    tokio::pin!(sampling);
    let mut current_reports = std::collections::HashSet::new();
    let mut retry_at = Instant::now();
    let mut inflight: Option<Request> = None;
    let mut child_inflight: Option<ChildRequest> = None;
    let mut child_retry_at = Instant::now();
    let mut child_failures = 0u32;
    let mut inventory_delivery = InventoryDelivery::default();
    let mut sequence =
        crate::sequence::Sequence::load(dir, &configuration.agent_id, &spool).await?;
    let mut failures = 0u32;
    let mut update_requests = crate::updates::Requests;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _=&mut shutdown => {
                config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
                if let Ok(bytes) = child_spool.encode()
                    && let Err(error) = config::write_private(&child_queue_path, &bytes).await
                {
                    eprintln!("Could not persist child reports during shutdown: {error}");
                }
                return Ok(());
            }
            _=&mut sampling => { anyhow::bail!("Collector stopped unexpectedly"); }
            result=async { inflight.as_mut().expect("guarded request").await }, if inflight.is_some() => {
                inflight=None;
                let (report,outcome)=result;
                match outcome {
                    Outcome::Accepted(response) => {
                        debug_assert!(response.accepted);
                        if let Some(watermark) = response.report_sequence { sequence.advance(dir, watermark).await?; }
                        spool.remove(&report.report_id); failures=0; retry_at=Instant::now();
                        if let Some(version)=&response.update_to { update_requests.handle_now(dir,version).await; }
                        if let Some(seconds)=response.report_interval_seconds { period_tx.send_if_modified(|period| { let next=Duration::from_secs(seconds.clamp(MIN_INTERVAL_SECONDS,MAX_INTERVAL_SECONDS).into()); if *period==next { false } else { *period=next; true } }); }
                        let applied = response.live_applied != Some(false);
                        if applied { inventory_delivery.accept(&report)?; }
                        let current = current_reports.remove(&report.report_id);
                        if current && applied {
                            let accepted=now();
                            config::write_private(&dir.join("last-report"),accepted.to_string().as_bytes()).await?;
                            config::write_private(&dir.join("last-acceptance.json"),&serde_json::to_vec(&Acceptance { agent_id:configuration.agent_id.clone(),report_id:report.report_id.clone(),run_id:run_id.clone(),process_id:std::process::id(),operation:operation.into(),activation_id:activation_id.clone(),collected_at:report.collected_at,accepted_at:accepted })?).await?;
                        }
                        config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
                        if !applied && current {
                            // Refresh inventory with the next new sample after a restored counter.
                            inventory_delivery = InventoryDelivery::default();
                        }
                        if operation!="run" && current && applied { println!("Agent report accepted"); return Ok(()); }
                    }
                    Outcome::Revoked => {
                        let mut disabled = configuration.clone();
                        disabled.disabled = true;
                        config::save(dir, &disabled).await?;
                        // Queued telemetry is no longer deliverable. Do not retain it on disable.
                        config::write_private(&dir.join("spool.json"), b"[]").await?;
                        let _ = config::write_private(&child_queue_path, b"[]").await;
                        return Err(delivery::Revoked.into());
                    }
                    Outcome::Invalid(status) => {
                        current_reports.remove(&report.report_id);
                        spool.remove(&report.report_id);
                        config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
                        if operation!="run" { anyhow::bail!("First report rejected ({status}); no success was confirmed"); }
                        eprintln!("Report rejected ({status}); dropping invalid sample");
                    }
                    Outcome::Retry(after) => {
                        failures=failures.saturating_add(1); retry_at=Instant::now()+after.unwrap_or_else(||delivery::backoff(failures,rand::random()));
                        if operation!="run" { anyhow::bail!("First report was not accepted; check connectivity and retry"); }
                    }
                }
            }
            result=async { child_inflight.as_mut().expect("guarded child request").await }, if child_inflight.is_some() => {
                child_inflight = None;
                let (batch, outcome) = result;
                match outcome {
                    ChildOutcome::Accepted => {
                        child_spool.pop(&batch.batch_id);
                        child_failures = 0;
                        child_retry_at = Instant::now();
                    }
                    ChildOutcome::Revoked => {
                        let mut disabled = configuration.clone();
                        disabled.disabled = true;
                        config::save(dir, &disabled).await?;
                        config::write_private(&dir.join("spool.json"), b"[]").await?;
                        let _ = config::write_private(&child_queue_path, b"[]").await;
                        return Err(delivery::Revoked.into());
                    }
                    ChildOutcome::Invalid(status) => {
                        child_spool.pop(&batch.batch_id);
                        child_failures = 0;
                        child_retry_at = Instant::now();
                        eprintln!("Child report rejected ({status}); dropping invalid inventory batch");
                    }
                    ChildOutcome::Retry(after) => {
                        child_failures = child_failures.saturating_add(1);
                        child_retry_at = Instant::now() + after.unwrap_or_else(|| child_delivery::backoff(child_failures, rand::random()));
                    }
                }
                if let Ok(bytes) = child_spool.encode()
                    && let Err(error) = config::write_private(&child_queue_path, &bytes).await
                {
                    eprintln!("Could not persist child reports: {error}");
                }
            }
            _=sleep_until(child_retry_at), if child_inflight.is_none() && !child_spool.is_empty() => {
                child_spool.trim(now());
                if let Some(batch) = child_spool.front().cloned() {
                    child_inflight = Some(Box::pin(child_delivery::send(client.clone(), configuration.server.clone(), credential.clone(), batch)));
                }
            }
            _=sleep_until(retry_at), if inflight.is_none() && !spool.is_empty() => {
                spool.trim(now());
                if let Some(report)=spool.latest().cloned() {
                    inflight=Some(Box::pin(delivery::send(client.clone(),configuration.server.clone(),credential.clone(),report)));
                }
            }
            Some(((inventory,metrics,mut collectors),collected_at,child_snapshots))=samples.recv() => {
                collectors.push(crate::updates::status(dir));
                let changed=inventory_delivery.changed(&inventory)?;
                let mut report=AgentReport { schema_version:SCHEMA_VERSION,report_id:Uuid::new_v4().to_string(),agent_id:configuration.agent_id.clone(),sequence:Some(sequence.next(dir).await?),collected_at,inventory:changed.then_some(inventory),metrics,collectors };
                crate::telemetry::bound_report(&mut report)?;
                for snapshot in child_snapshots {
                    let Some(batches) = child_schedule.batch(snapshot, &configuration.agent_id, collected_at) else { continue; };
                    if batches.iter().all(|batch| hope_protocol::validation::child_batch(batch).is_ok()) {
                        match config::write_private(&child_state_path, &serde_json::to_vec(&child_schedule)?).await {
                            Ok(()) => {
                                let mut queued = true;
                                for batch in batches {
                                    if let Err(error) = child_spool.push(batch, now()) {
                                        eprintln!("Could not queue child report: {error:#}");
                                        queued = false;
                                        break;
                                    }
                                }
                                if queued && let Ok(bytes) = child_spool.encode()
                                    && let Err(error) = config::write_private(&child_queue_path, &bytes).await {
                                    eprintln!("Could not persist child reports: {error}");
                                }
                            }
                            Err(error) => {
                                child_schedule.disabled = true;
                                eprintln!("Child reporting disabled: could not persist its sequence: {error}");
                            }
                        }
                    } else {
                        eprintln!("Dropped invalid local child inventory batch; core reporting continues");
                    }
                }
                current_reports.insert(report.report_id.clone());
                spool.push(report,now())?;
                current_reports.retain(|id| spool.iter().any(|report| &report.report_id == id));
                config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::{
        ChildDeviceKind, ChildDeviceReport, ChildMetrics, ChildSourceKind, DeviceLifecycle,
        SourceIdentity,
    };
    #[test]
    fn child_batches_keep_monotonic_sequences_and_separate_inventory_generations() {
        let mut schedule = ChildSchedule {
            counters: Default::default(),
            disabled: false,
        };
        let snapshot = || collect::ChildSnapshot {
            source: ChildSourceKind::Docker,
            source_scope: "engine-id".into(),
            source_token: None,
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            observed_at: 100.0,
            devices: vec![],
            inventory_complete: true,
        };
        let agent_id = Uuid::new_v4().to_string();
        let first = schedule
            .batch(snapshot(), &agent_id, 100.0)
            .unwrap()
            .remove(0);
        assert!(schedule.batch(snapshot(), &agent_id, 115.0).is_none());
        let second = schedule
            .batch(snapshot(), &agent_id, 130.0)
            .unwrap()
            .remove(0);
        let third = schedule
            .batch(snapshot(), &agent_id, 160.0)
            .unwrap()
            .remove(0);
        assert_eq!(
            (first.sequence, first.inventory_generation, first.complete),
            (1, 1, true)
        );
        assert_eq!(
            (
                second.sequence,
                second.inventory_generation,
                second.complete
            ),
            (2, 1, false)
        );
        assert_eq!(
            (third.sequence, third.inventory_generation, third.complete),
            (3, 2, true)
        );
        for batch in [first, second, third] {
            hope_protocol::validation::child_batch(&batch).expect("valid batch");
        }
    }
    #[test]
    fn incomplete_collections_never_claim_authoritative_absence() {
        let mut schedule = ChildSchedule {
            counters: Default::default(),
            disabled: false,
        };
        let snapshot = |inventory_complete| collect::ChildSnapshot {
            source: ChildSourceKind::Docker,
            source_scope: "engine-id".into(),
            source_token: None,
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            observed_at: 100.0,
            devices: vec![],
            inventory_complete,
        };
        let first = schedule
            .batch(snapshot(false), "agent", 100.0)
            .unwrap()
            .remove(0);
        let next = schedule
            .batch(snapshot(false), "agent", 160.0)
            .unwrap()
            .remove(0);
        let complete = schedule
            .batch(snapshot(true), "agent", 220.0)
            .unwrap()
            .remove(0);
        assert!(!first.complete && !next.complete && complete.complete);
        assert_eq!(
            (
                first.inventory_generation,
                next.inventory_generation,
                complete.inventory_generation
            ),
            (1, 2, 3)
        );
    }

    #[test]
    fn each_child_source_has_independent_sequences_and_complete_inventory_clock() {
        let mut schedule = ChildSchedule::default();
        let docker = collect::ChildSnapshot {
            source: ChildSourceKind::Docker,
            source_scope: "engine".into(),
            source_token: None,
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            observed_at: 100.0,
            devices: vec![],
            inventory_complete: true,
        };
        let proxmox = collect::ChildSnapshot {
            source: ChildSourceKind::Proxmox,
            source_scope: Uuid::new_v4().to_string(),
            source_token: Some("a".repeat(64)),
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: Some(vec![]),
            observed_at: 100.0,
            devices: vec![],
            inventory_complete: true,
        };
        let docker_first = schedule
            .batch(docker.clone(), "agent", 100.0)
            .unwrap()
            .remove(0);
        let pve_first = schedule
            .batch(proxmox.clone(), "agent", 100.0)
            .unwrap()
            .remove(0);
        let docker_second = schedule.batch(docker, "agent", 160.0).unwrap().remove(0);
        let pve_second = schedule.batch(proxmox, "agent", 160.0).unwrap().remove(0);
        assert_eq!(
            (
                docker_first.sequence,
                docker_first.inventory_generation,
                docker_first.complete
            ),
            (1, 1, true)
        );
        assert_eq!(
            (
                pve_first.sequence,
                pve_first.inventory_generation,
                pve_first.complete
            ),
            (1, 1, true)
        );
        assert_eq!(
            (docker_second.sequence, docker_second.inventory_generation),
            (2, 2)
        );
        assert_eq!(
            (pve_second.sequence, pve_second.inventory_generation),
            (2, 2)
        );
    }

    #[test]
    fn oversized_child_inventory_is_partial_and_fits_the_server_body_limit() {
        let source_scope = "engine-id".to_owned();
        let devices: Vec<ChildDeviceReport> = (0..512)
            .map(|index| ChildDeviceReport {
                identity: SourceIdentity {
                    kind: ChildDeviceKind::DockerContainer,
                    scope: source_scope.clone(),
                    key: format!("container-{index}"),
                    incarnation: "first".into(),
                },
                ownership_generation: None,
                legacy_incarnation: None,
                name: "n".repeat(256),
                lifecycle: DeviceLifecycle::Running,
                status: None,
                health: None,
                restart_count: None,
                parent_key: None,
                node: None,
                tags: vec!["t".repeat(128); 64],
                interfaces: vec![],
                cpu_cores: None,
                memory_bytes: None,
                image: None,
                compose_project: None,
                compose_service: None,
                published_ports: vec![],
                guest_agent_status: None,
                guest_os: None,
                guest_filesystems: vec![],
                network_configuration: vec![],
                disk_configuration: vec![],
                metrics: ChildMetrics::default(),
            })
            .collect();
        let mut schedule = ChildSchedule::default();
        let agent_id = Uuid::new_v4().to_string();
        let mut observed = std::collections::HashSet::new();
        let mut final_chunks = 0;
        for report in 0..32 {
            let batches = schedule
                .batch(
                    collect::ChildSnapshot {
                        source: ChildSourceKind::Docker,
                        source_scope: source_scope.clone(),
                        source_token: None,
                        legacy_source_scope: None,
                        legacy_identities: vec![],
                        proxmox_guest_presence: None,
                        observed_at: 100.0 + f64::from(report) * 30.0,
                        devices: devices.clone(),
                        inventory_complete: true,
                    },
                    &agent_id,
                    100.0 + f64::from(report) * 30.0,
                )
                .unwrap();
            for batch in batches {
                assert!(serde_json::to_vec(&batch).unwrap().len() <= MAX_AGENT_REPORT_BYTES);
                hope_protocol::validation::child_batch(&batch).expect("bounded batch is valid");
                final_chunks += usize::from(batch.complete);
                observed.extend(batch.devices.into_iter().map(|device| device.identity.key));
            }
        }
        assert_eq!(
            observed.len(),
            512,
            "multipart inventories report every device in each generation"
        );
        assert!(
            final_chunks >= 16,
            "each full snapshot ends with an authoritative chunk"
        );
    }
    #[test]
    fn jitter_stays_within_a_fifteenth_of_the_interval() {
        for _ in 0..200 {
            let thirty = jittered(Duration::from_secs(30));
            assert!(thirty >= Duration::from_secs(28) && thirty <= Duration::from_secs(32));
            let ten = jittered(Duration::from_secs(10));
            assert!(ten >= Duration::from_millis(9334) && ten <= Duration::from_millis(10666));
        }
        assert_eq!(
            jittered(Duration::from_millis(50)),
            Duration::from_millis(50)
        );
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct FakeSampler {
        count: usize,
        stop_after_samples: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Sampler for FakeSampler {
        fn sample(&mut self) -> Sampling<'_> {
            self.count += 1;
            if self.count >= 6
                && let Some(sender) = self.stop_after_samples.take()
            {
                let _ = sender.send(());
            }
            Box::pin(async {
                (
                    Inventory {
                        hostname: "fixture".into(),
                        os: "test".into(),
                        architecture: "test".into(),
                        cpu_model: "test".into(),
                        cpu_cores: 1,
                        memory_total_bytes: 1.0,
                        interfaces: vec![],
                        disks: vec![],
                        gpus: vec![],
                        collectors: vec![],
                    },
                    Metrics::default(),
                    vec![],
                )
            })
        }
    }
    struct OvertakingSampler {
        inner: FakeSampler,
        second_sample: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Sampler for OvertakingSampler {
        fn sample(&mut self) -> Sampling<'_> {
            let second_sample = (self.inner.count == 1)
                .then(|| self.second_sample.take())
                .flatten();
            let sample = self.inner.sample();
            Box::pin(async move {
                let sample = sample.await;
                if let Some(second_sample) = second_sample {
                    let _ = second_sample.send(());
                }
                sample
            })
        }
    }
    struct ChildFakeSampler {
        count: usize,
        stop_after_samples: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Sampler for ChildFakeSampler {
        fn sample(&mut self) -> Sampling<'_> {
            self.count += 1;
            if self.count >= 6
                && let Some(sender) = self.stop_after_samples.take()
            {
                let _ = sender.send(());
            }
            Box::pin(async {
                (
                    Inventory {
                        hostname: "fixture".into(),
                        os: "test".into(),
                        architecture: "test".into(),
                        cpu_model: "test".into(),
                        cpu_cores: 1,
                        memory_total_bytes: 1.0,
                        interfaces: vec![],
                        disks: vec![],
                        gpus: vec![],
                        collectors: vec![],
                    },
                    Metrics::default(),
                    vec![],
                )
            })
        }
        fn child_snapshots(&self) -> Vec<collect::ChildSnapshot> {
            vec![collect::ChildSnapshot {
                source: ChildSourceKind::Docker,
                source_scope: "docker-engine".into(),
                source_token: None,
                legacy_source_scope: None,
                legacy_identities: vec![],
                proxmox_guest_presence: None,
                observed_at: now(),
                devices: vec![],
                inventory_complete: true,
            }]
        }
    }
    #[tokio::test]
    async fn inventory_suppression_starts_only_after_acceptance_and_ignores_older_backlog() {
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        let (inventory, metrics, collectors) = sampler.sample().await;
        let mut delivery = InventoryDelivery::default();
        assert!(delivery.changed(&inventory).expect("changed"));
        assert!(delivery.changed(&inventory).expect("still unaccepted"));
        let mut report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: Uuid::new_v4().to_string(),
            sequence: None,
            collected_at: 200.0,
            inventory: Some(inventory.clone()),
            metrics,
            collectors,
        };
        delivery.accept(&report).expect("accept");
        assert!(!delivery.changed(&inventory).expect("unchanged"));
        delivery.refreshed_at = Some(Instant::now() - Duration::from_secs(3600));
        assert!(delivery.changed(&inventory).expect("refresh"));
        delivery.refreshed_at = Some(Instant::now());
        let mut changed = inventory.clone();
        changed.hostname = "changed".into();
        assert!(delivery.changed(&changed).expect("new inventory"));
        report.collected_at = 100.0;
        report.inventory = Some(changed);
        delivery.accept(&report).expect("old accepted backlog");
        assert!(!delivery.changed(&inventory).expect("latest remains"));
        report.sequence = Some(1);
        report.collected_at = 500.0;
        report.inventory = Some(inventory.clone());
        delivery.accept(&report).unwrap();
        let mut corrected = inventory.clone();
        corrected.hostname = "clock-corrected".into();
        report.sequence = Some(2);
        report.collected_at = 50.0;
        report.inventory = Some(corrected.clone());
        delivery.accept(&report).unwrap();
        assert!(!delivery.changed(&corrected).unwrap());
        report.sequence = Some(1);
        report.collected_at = 500.0;
        report.inventory = Some(inventory);
        delivery.accept(&report).unwrap();
        assert!(!delivery.changed(&corrected).unwrap());
    }
    async fn server(
        status: &str,
        body: &str,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{Router, http::StatusCode, routing::any};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let status = StatusCode::from_u16(
            status
                .split_whitespace()
                .next()
                .expect("status code")
                .parse()
                .expect("numeric status code"),
        )
        .expect("valid status code");
        let body = body.to_string();
        let app = Router::new().fallback(any(move || {
            let body = body.clone();
            async move {
                tokio::time::sleep(delay).await;
                (
                    status,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
            }
        }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn restored_counter_waits_for_a_new_live_sample_before_confirming_installation() {
        use axum::{Json, Router, extract::State, routing::post};
        use std::sync::{Arc, Mutex};
        let reports = Arc::new(Mutex::new(Vec::<AgentReport>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/api/v1/agent/report",
                post(
                    |State(reports): State<Arc<Mutex<Vec<AgentReport>>>>,
                     Json(report): Json<AgentReport>| async move {
                        let applied = report.sequence.unwrap() > 100;
                        let watermark = report.sequence.unwrap().max(100);
                        reports.lock().unwrap().push(report);
                        Json(hope_protocol::ReportResponse {
                            accepted: true,
                            duplicate: false,
                            live_applied: Some(applied),
                            report_sequence: Some(watermark),
                            report_interval_seconds: None,
                            update_to: None,
                        })
                    },
                ),
            )
            .with_state(reports.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let config = configuration(format!("http://{address}"));
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        tokio::time::timeout(
            Duration::from_secs(10),
            run(
                &mut sampler,
                &config,
                Spool::default(),
                "once",
                dir.path(),
                Duration::from_millis(50),
                std::future::pending(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let accepted = {
            let attempts = reports.lock().unwrap();
            assert!(attempts.len() >= 2);
            assert!(attempts[0].sequence.unwrap() <= 100);
            attempts.last().unwrap().clone()
        };
        assert!(accepted.sequence.unwrap() > 100);
        assert!(accepted.inventory.is_some());
        let marker: Acceptance = serde_json::from_slice(
            &tokio::fs::read(dir.path().join("last-acceptance.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(marker.report_id, accepted.report_id);
        task.abort();
    }

    #[tokio::test]
    async fn stalled_child_uploads_do_not_delay_parent_heartbeat_acceptance() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let core_requests = Arc::new(AtomicUsize::new(0));
        let child_requests = Arc::new(AtomicUsize::new(0));
        let core_count = core_requests.clone();
        let child_count = child_requests.clone();
        let server_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let core_count = core_count.clone();
                let child_count = child_count.clone();
                tokio::spawn(async move {
                    let mut request = [0; 8192];
                    let size = socket.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..size]);
                    if request.starts_with("POST /api/v1/agent/children ") {
                        child_count.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        return;
                    }
                    if request.starts_with("POST /api/v1/agent/report ") {
                        core_count.fetch_add(1, Ordering::SeqCst);
                        let body = r#"{"accepted":true,"duplicate":false,"report_interval_seconds":null,"update_to":null}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                });
            }
        });

        let directory = tempfile::tempdir().expect("directory");
        let last_report = directory.path().join("last-report");
        let config = configuration(format!("http://{address}"));
        let mut sampler = ChildFakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        let child_count = child_requests.clone();
        tokio::time::timeout(
            Duration::from_secs(3),
            run(
                &mut sampler,
                &config,
                Spool::default(),
                "run",
                directory.path(),
                Duration::from_millis(80),
                async move {
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                    while (child_count.load(Ordering::SeqCst) == 0 || !last_report.exists())
                        && tokio::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                },
            ),
        )
        .await
        .expect("sampling and parent reports continue while child upload is stalled")
        .expect("agent run");
        assert!(child_requests.load(Ordering::SeqCst) >= 1);
        assert!(core_requests.load(Ordering::SeqCst) >= 1);
        assert!(directory.path().join("last-report").exists());
        let queued = config::read_bounded(
            &directory.path().join("child-spool.json"),
            child_delivery::MAX_BYTES,
        )
        .await
        .expect("child queue persisted");
        assert!(!child_delivery::Spool::decode(&queued, &config.agent_id, now()).is_empty());
        server_task.abort();
    }
    fn configuration(server: String) -> Config {
        Config {
            server,
            agent_id: Uuid::new_v4().to_string(),
            credential: Some("fixture-credential".into()),
            pending_credential: None,
            ca_pem: None,
            disabled: false,
        }
    }
    struct SlowSampler {
        inner: FakeSampler,
        delay: Duration,
    }

    #[tokio::test]
    async fn only_explicit_revocation_is_terminal() {
        for (status, body, revoked) in [
            ("403 Forbidden", "access policy denied", false),
            (
                "401 Unauthorized",
                r#"{"code":"authentication_required","error":"Authentication required"}"#,
                false,
            ),
            (
                "401 Unauthorized",
                r#"{"code":"agent_revoked","error":"Agent credential revoked"}"#,
                true,
            ),
        ] {
            let (endpoint, task) = server(status, body, Duration::ZERO).await;
            let config = configuration(endpoint.clone());
            let client = config::client(&config).expect("client");
            let report = AgentReport {
                schema_version: SCHEMA_VERSION,
                report_id: Uuid::new_v4().to_string(),
                agent_id: config.agent_id,
                sequence: None,
                collected_at: now(),
                inventory: None,
                metrics: Metrics::default(),
                collectors: vec![],
            };
            let (_, outcome) =
                delivery::send(client, endpoint, config.credential.unwrap(), report).await;
            if revoked {
                assert!(matches!(outcome, Outcome::Revoked));
            } else {
                assert!(matches!(outcome, Outcome::Retry(_)));
            }
            task.abort();
        }
    }
    impl Sampler for SlowSampler {
        fn sample(&mut self) -> Sampling<'_> {
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                self.inner.sample().await
            })
        }
    }

    #[tokio::test]
    async fn collection_slower_than_the_interval_cannot_starve_delivery_or_shutdown() {
        let (endpoint, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::from_millis(50),
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let mut sampler = SlowSampler {
            inner: FakeSampler {
                count: 0,
                stop_after_samples: None,
            },
            delay: Duration::from_millis(100),
        };
        tokio::time::timeout(
            Duration::from_secs(10),
            run(
                &mut sampler,
                &configuration(endpoint.clone()),
                Spool::default(),
                "once",
                directory.path(),
                Duration::from_millis(5),
                std::future::pending(),
            ),
        )
        .await
        .expect("slow collection must not starve HTTP")
        .expect("accepted");
        assert!(directory.path().join("last-acceptance.json").exists());

        sampler.delay = Duration::from_secs(60);
        tokio::time::timeout(
            Duration::from_secs(5),
            run(
                &mut sampler,
                &configuration(endpoint),
                Spool::default(),
                "run",
                directory.path(),
                Duration::from_millis(5),
                tokio::time::sleep(Duration::from_millis(20)),
            ),
        )
        .await
        .expect("shutdown must not wait for collection")
        .expect("shutdown");
        task.abort();
    }

    #[tokio::test]
    async fn a_one_off_report_confirms_a_live_sample_even_when_sampling_overtakes_http() {
        use axum::{Router, http::StatusCode, routing::any};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let (second_sample, second_sample_wait) = tokio::sync::oneshot::channel();
        let response_gate = std::sync::Arc::new(tokio::sync::Mutex::new(Some(second_sample_wait)));
        let body = r#"{"accepted":true,"duplicate":false,"live_applied":true}"#.to_string();
        let app = Router::new().fallback(any(move || {
            let response_gate = response_gate.clone();
            let body = body.clone();
            async move {
                if let Some(response_gate) = response_gate.lock().await.take() {
                    let _ = response_gate.await;
                }
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
            }
        }));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let mut sampler = OvertakingSampler {
            inner: FakeSampler {
                count: 0,
                stop_after_samples: None,
            },
            second_sample: Some(second_sample),
        };
        tokio::time::timeout(
            Duration::from_secs(10),
            run(
                &mut sampler,
                &configuration(format!("http://{address}")),
                Spool::default(),
                "once",
                dir.path(),
                Duration::from_millis(10),
                std::future::pending(),
            ),
        )
        .await
        .expect("sample must advance before its HTTP response")
        .expect("accepted");
        assert!(sampler.inner.count >= 2);
        assert!(dir.path().join("last-acceptance.json").exists());
        task.abort();
    }

    #[tokio::test]
    async fn sampling_continues_during_slow_requests_and_spool_is_saved() {
        let (server, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::from_secs(10),
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: Some(sender),
        };
        let configuration = configuration(server);
        tokio::time::timeout(
            Duration::from_secs(2),
            run(
                &mut sampler,
                &configuration,
                Spool::default(),
                "run",
                directory.path(),
                Duration::from_millis(15),
                async {
                    let _ = receiver.await;
                },
            ),
        )
        .await
        .expect("Sampling must continue before the slow request finishes")
        .expect("run");
        task.abort();
        assert!(
            sampler.count >= 6,
            "Slow network must not postpone sampling"
        );
        let bytes = config::read_bounded(&directory.path().join("spool.json"), delivery::MAX_BYTES)
            .await
            .expect("spool");
        let queue: std::collections::VecDeque<AgentReport> =
            serde_json::from_slice(&bytes).expect("queue");
        assert!(queue.len() >= 3);
        assert!(
            !tokio::fs::try_exists(directory.path().join("last-acceptance.json"))
                .await
                .expect("marker")
        );
    }
    #[tokio::test]
    async fn rejected_or_false_acceptance_never_confirms_success() {
        for (status, body) in [
            ("400 Bad Request", "{}"),
            ("429 Too Many Requests", "{}"),
            ("200 OK", r#"{"accepted":false,"duplicate":false}"#),
            ("401 Unauthorized", "{}"),
            ("200 OK", &"x".repeat(16385)),
        ] {
            let (server, task) = server(status, body, Duration::ZERO).await;
            let directory = tempfile::tempdir().expect("temporary");
            let mut sampler = FakeSampler {
                count: 0,
                stop_after_samples: None,
            };
            assert!(
                run(
                    &mut sampler,
                    &configuration(server),
                    Spool::default(),
                    "once",
                    directory.path(),
                    Duration::from_secs(30),
                    std::future::pending()
                )
                .await
                .is_err()
            );
            task.abort();
            assert!(
                !tokio::fs::try_exists(directory.path().join("last-acceptance.json"))
                    .await
                    .expect("marker")
            );
        }
    }
    #[tokio::test]
    async fn offline_startup_and_tls_failure_do_not_confirm_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let offline = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let (plain, task) = server("200 OK", "{}", Duration::ZERO).await;
        for endpoint in [offline, plain.replacen("http:", "https:", 1)] {
            let directory = tempfile::tempdir().expect("temporary");
            let mut sampler = FakeSampler {
                count: 0,
                stop_after_samples: None,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                run(
                    &mut sampler,
                    &configuration(endpoint),
                    Spool::default(),
                    "once",
                    directory.path(),
                    Duration::from_secs(30),
                    std::future::pending(),
                ),
            )
            .await
            .expect("bounded transport");
            assert!(result.is_err());
            assert!(!directory.path().join("last-acceptance.json").exists());
            assert!(directory.path().join("spool.json").exists());
        }
        task.abort();
    }
    #[tokio::test]
    async fn only_accepted_current_report_creates_success_marker() {
        let (server, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::ZERO,
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        run(
            &mut sampler,
            &configuration(server),
            Spool::default(),
            "once",
            directory.path(),
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .expect("accepted");
        task.abort();
        let marker: serde_json::Value = serde_json::from_slice(
            &config::read_bounded(&directory.path().join("last-acceptance.json"), 4096)
                .await
                .expect("marker"),
        )
        .expect("json");
        assert_eq!(marker["operation"], "once");
        assert_eq!(marker["process_id"], std::process::id());
    }
    async fn report_once(directory: &std::path::Path, body: &str) {
        let (server, task) = server("200 OK", body, Duration::ZERO).await;
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        run(
            &mut sampler,
            &configuration(server),
            Spool::default(),
            "once",
            directory,
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .expect("accepted");
        task.abort();
    }
    #[tokio::test]
    async fn an_update_the_server_asks_for_is_left_for_the_updater() {
        let directory = tempfile::tempdir().expect("temporary");
        std::fs::create_dir(directory.path().join("update-queue")).expect("queue");
        report_once(
            directory.path(),
            r#"{"accepted":true,"duplicate":false,"update_to":"0.1.60"}"#,
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(directory.path().join("update-queue/request"))
                .expect("request"),
            "0.1.60\n"
        );
    }
    #[tokio::test]
    async fn no_request_is_left_when_none_is_asked_or_no_updater_exists() {
        let asked = tempfile::tempdir().expect("temporary");
        std::fs::create_dir(asked.path().join("update-queue")).expect("queue");
        report_once(asked.path(), r#"{"accepted":true,"duplicate":false}"#).await;
        assert!(!asked.path().join("update-queue/request").exists());
        let bare = tempfile::tempdir().expect("temporary");
        report_once(
            bare.path(),
            r#"{"accepted":true,"duplicate":false,"update_to":"0.1.60"}"#,
        )
        .await;
        assert!(!bare.path().join("update-queue").exists());
    }
}
