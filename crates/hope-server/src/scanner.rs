use crate::{
    error::{ApiError, ApiResult},
    state::{State, now},
};
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use hope_protocol::{Capabilities, Network, ScanJob, ScanStatus, Service};
use ipnet::Ipv4Net;
use sqlx::Row;
use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{Mutex, Semaphore, mpsc},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Batch {
    state: State,
    job: ScanJob,
    ips: VecDeque<Ipv4Addr>,
    cancel: CancellationToken,
    /// Scheduled background scan: paced well below a scan someone started by hand.
    gentle: bool,
}

/// Addresses a scheduled scan probes at once, and the minimum gap between starting two of them.
/// Each address costs about five probes, so this caps background discovery near 80 probes per
/// second; a /24 takes about 15 seconds, a /20 under four minutes.
const GENTLE_PARALLEL: usize = 4;
const GENTLE_SPACING: Duration = Duration::from_millis(60);

#[derive(Clone, Default)]
pub(crate) struct Observation {
    pub alive: bool,
    pub method: &'static str,
    pub mac: Option<String>,
    pub services: Vec<Service>,
}
pub(crate) trait Probe: Send + Sync {
    fn probe(&self, ip: Ipv4Addr) -> BoxFuture<'_, Observation>;
}
struct NativeProbe {
    limiter: Arc<ProbeLimiter>,
    arp: crate::arp::Arp,
    ping: Option<Arc<surge_ping::Client>>,
}
impl Probe for NativeProbe {
    fn probe(&self, ip: Ipv4Addr) -> BoxFuture<'_, Observation> {
        Box::pin(async move {
            let (ping, ssh, http, https, mac) = tokio::join!(
                ping(ip, &self.limiter, self.ping.as_deref()),
                tcp_probe(ip, 22, &self.limiter),
                tcp_probe(ip, 80, &self.limiter),
                tcp_probe(ip, 443, &self.limiter),
                async {
                    if self.arp.on_link(ip) {
                        let _permit = self.limiter.acquire().await.ok()?;
                        self.arp.probe(ip).await
                    } else {
                        None
                    }
                }
            );
            Observation {
                alive: alive(ping, mac.is_some(), &[ssh, http, https]),
                method: if ping {
                    "icmp"
                } else if mac.is_some() {
                    "arp"
                } else {
                    "tcp"
                },
                mac,
                services: [(ssh, 22, "SSH"), (http, 80, "HTTP"), (https, 443, "HTTPS")]
                    .into_iter()
                    .filter(|(result, _, _)| *result == TcpResult::Open)
                    .map(|(_, port, name)| Service {
                        port,
                        name: name.into(),
                        observed_at: now(),
                    })
                    .collect(),
            }
        })
    }
}
#[derive(Clone)]
pub struct Scheduler {
    sender: mpsc::Sender<Batch>,
    pub(crate) limiter: Arc<ProbeLimiter>,
    pub(crate) arp: crate::arp::Arp,
}
impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
impl Scheduler {
    pub fn new() -> Self {
        Self::native(std::env::var("HOPE_DISCOVERY_MODE").as_deref() != Ok("limited"))
    }
    pub(crate) fn native(enabled: bool) -> Self {
        let limiter = Arc::new(ProbeLimiter::new());
        let arp = crate::arp::Arp::new(enabled);
        let backend = Arc::new(NativeProbe {
            limiter: limiter.clone(),
            arp: arp.clone(),
            ping: surge_ping::Client::new(&surge_ping::Config::default())
                .ok()
                .map(Arc::new),
        });
        Self::with_backend(backend, limiter, arp)
    }
    pub(crate) fn with_backend(
        backend: Arc<dyn Probe>,
        limiter: Arc<ProbeLimiter>,
        arp: crate::arp::Arp,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Batch>(32);
        tokio::spawn(async move {
            let mut queue = VecDeque::<Batch>::new();
            let mut work = FuturesUnordered::new();
            let mut tick = tokio::time::interval(Duration::from_millis(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let gentle_live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let gentle_pacer = Arc::new(Mutex::new(Instant::now()));
            loop {
                tokio::select! {
                    batch = receiver.recv() => match batch { Some(batch) => queue.push_back(batch), None => break },
                    Some(()) = work.next(), if !work.is_empty() => {},
                    _ = tick.tick(), if !queue.is_empty() && work.len() < 64 => {
                        let gentle_ready = gentle_live.load(std::sync::atomic::Ordering::SeqCst) < GENTLE_PARALLEL;
                        // Round-robin across batches; a paced batch waits its turn without blocking others.
                        for _ in 0..queue.len() {
                            let Some(mut batch) = queue.pop_front() else { break };
                            if batch.cancel.is_cancelled() { continue; }
                            if batch.gentle && !gentle_ready { queue.push_back(batch); continue; }
                            if let Some(ip) = batch.ips.pop_front() {
                                let gentle = batch.gentle.then(|| gentle_live.clone());
                                if let Some(live) = &gentle {
                                    live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                }
                                let pacer = batch.gentle.then(|| gentle_pacer.clone());
                                let probe = run_probe(batch.state.clone(), batch.job.clone(), ip, batch.cancel.clone(), backend.clone(), pacer);
                                work.push(async move {
                                    probe.await;
                                    if let Some(live) = gentle { live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); }
                                });
                                if !batch.ips.is_empty() { queue.push_back(batch); }
                            }
                            break;
                        }
                    }
                }
            }
        });
        Self {
            sender,
            limiter,
            arp,
        }
    }
}

pub(crate) struct ProbeLimiter {
    next: Mutex<Instant>,
    sockets: Semaphore,
}
impl ProbeLimiter {
    pub(crate) fn new() -> Self {
        Self {
            next: Mutex::new(Instant::now()),
            sockets: Semaphore::new(64),
        }
    }
    pub(crate) async fn acquire(&self) -> anyhow::Result<tokio::sync::SemaphorePermit<'_>> {
        let permit = self.sockets.acquire().await?;
        let mut next = self.next.lock().await;
        tokio::time::sleep_until(*next).await;
        *next = Instant::now() + Duration::from_millis(5); // 200 probe starts/second, shared across ranges.
        Ok(permit)
    }
}

pub fn parse_range(cidr: &str) -> ApiResult<Ipv4Net> {
    let net: Ipv4Net = cidr
        .parse()
        .map_err(|_| ApiError::bad("Enter an IPv4 CIDR such as 192.168.1.0/24"))?;
    if net.prefix_len() < 20 {
        return Err(ApiError::bad(
            "Use /20 or smaller ranges; split larger networks into separate ranges",
        ));
    }
    if net.network().is_multicast()
        || net.network().is_unspecified()
        || net.network().is_broadcast()
    {
        return Err(ApiError::bad("This address range cannot be scanned"));
    }
    Ok(net.trunc())
}

/// Scan a range at full speed (someone asked for it).
pub async fn start(state: &State, network: &Network) -> ApiResult<ScanJob> {
    begin(state, network, false).await
}

/// Scan a range as a scheduled, gently paced background scan.
pub async fn start_scheduled(state: &State, network: &Network) -> ApiResult<ScanJob> {
    begin(state, network, true).await
}

async fn begin(state: &State, network: &Network, gentle: bool) -> ApiResult<ScanJob> {
    if state.0.shutdown.is_cancelled() {
        return Err(ApiError::bad("The server is shutting down"));
    }
    let net = parse_range(&network.cidr)?;
    let mut active = state.0.scans.lock().await;
    // A failed terminal write can leave a running row after its worker was cancelled.
    // Serialize with scan creation and recover only rows without live work.
    let unfinished: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM scans WHERE network_id=? AND status IN ('queued','running')",
    )
    .bind(&network.id)
    .fetch_all(&state.0.db)
    .await?;
    for id in unfinished {
        if active.get(&id).is_some_and(|token| !token.is_cancelled()) {
            continue;
        }
        sqlx::query("UPDATE scans SET status='interrupted',finished_at=?,error='Scan worker stopped; discovery will retry' WHERE id=? AND status IN ('queued','running')")
            .bind(now()).bind(&id).execute(&state.0.db).await?;
        active.remove(&id);
        tracing::warn!(scan_id = %id, "recovered abandoned scan");
    }
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM scans WHERE network_id=? AND status IN ('queued','running')",
    )
    .bind(&network.id)
    .fetch_one(&state.0.db)
    .await?;
    if running > 0 {
        return Err(ApiError::bad("This network already has an active scan"));
    }
    if active.len() >= 32 {
        return Err(ApiError::bad("At most 32 ranges can scan at once"));
    }
    let ips: VecDeque<_> = net.hosts().collect();
    let job = ScanJob {
        id: Uuid::new_v4().to_string(),
        network_id: network.id.clone(),
        cidr: net.to_string(),
        status: ScanStatus::Running,
        scanned: 0,
        total: ips.len() as u32,
        discovered: 0,
        started_at: now(),
        finished_at: None,
        error: None,
    };
    sqlx::query(
        "INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES(?,?,?,?,?,?)",
    )
    .bind(&job.id)
    .bind(&job.network_id)
    .bind(&job.cidr)
    .bind("running")
    .bind(job.total)
    .bind(job.started_at)
    .execute(&state.0.db)
    .await?;
    if state.0.shutdown.is_cancelled() {
        // Shutdown began after the first check; do not leave a row claiming to run.
        sqlx::query("UPDATE scans SET status='interrupted',finished_at=?,error='Server stopped; start a new scan' WHERE id=?")
            .bind(now())
            .bind(&job.id)
            .execute(&state.0.db)
            .await?;
        return Err(ApiError::bad("The server is shutting down"));
    }
    let cancel = CancellationToken::new();
    active.insert(job.id.clone(), cancel.clone());
    state
        .0
        .scheduler
        .sender
        .send(Batch {
            state: state.clone(),
            job: job.clone(),
            ips,
            cancel,
            gentle,
        })
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    state.changed();
    Ok(job)
}

pub async fn cancel(state: &State, id: &str) -> ApiResult<()> {
    if let Some(token) = state.0.scans.lock().await.remove(id) {
        token.cancel();
    }
    sqlx::query("UPDATE scans SET status='cancelled',finished_at=? WHERE id=? AND status IN ('queued','running')")
        .bind(now()).bind(id).execute(&state.0.db).await?;
    state.changed();
    Ok(())
}

pub async fn jobs(state: &State) -> ApiResult<Vec<ScanJob>> {
    let rows = sqlx::query("SELECT * FROM scans WHERE id IN (SELECT id FROM scans ORDER BY started_at DESC LIMIT 100) OR id IN (SELECT id FROM (SELECT id,ROW_NUMBER() OVER(PARTITION BY network_id ORDER BY started_at DESC) AS rank FROM scans) WHERE rank=1) ORDER BY started_at DESC")
        .fetch_all(&state.0.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| -> anyhow::Result<ScanJob> {
            Ok(ScanJob {
                id: r.get("id"),
                network_id: r.get("network_id"),
                cidr: r.get("cidr"),
                status: serde_json::from_value(serde_json::Value::String(r.get("status")))?,
                scanned: r.get::<i64, _>("scanned") as u32,
                total: r.get::<i64, _>("total") as u32,
                discovered: r.get::<i64, _>("discovered") as u32,
                started_at: r.get("started_at"),
                finished_at: r.get("finished_at"),
                error: r.get("error"),
            })
        })
        .collect::<anyhow::Result<_>>()?)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TcpResult {
    Open,
    Refused,
    Unanswered,
}
/// A refused connection is not evidence of a device: firewalls and routers that reject
/// traffic to a routed range answer with a reset for every address in it.
fn alive(ping: bool, mac: bool, tcp: &[TcpResult]) -> bool {
    ping || mac || tcp.contains(&TcpResult::Open)
}
async fn tcp_probe(ip: Ipv4Addr, port: u16, limiter: &ProbeLimiter) -> TcpResult {
    let Ok(_permit) = limiter.acquire().await else {
        return TcpResult::Unanswered;
    };
    match timeout(
        Duration::from_millis(750),
        TcpStream::connect(SocketAddr::new(IpAddr::V4(ip), port)),
    )
    .await
    {
        Ok(Ok(_)) => TcpResult::Open,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            TcpResult::Refused
        }
        _ => TcpResult::Unanswered,
    }
}
pub(crate) async fn tcp(ip: Ipv4Addr, port: u16, limiter: &ProbeLimiter) -> bool {
    tcp_probe(ip, port, limiter).await == TcpResult::Open
}

async fn ping(ip: Ipv4Addr, limiter: &ProbeLimiter, client: Option<&surge_ping::Client>) -> bool {
    let Some(client) = client else { return false };
    let Ok(_permit) = limiter.acquire().await else {
        return false;
    };
    let mut pinger = client
        .pinger(IpAddr::V4(ip), surge_ping::PingIdentifier(rand::random()))
        .await;
    pinger.timeout(Duration::from_millis(750));
    pinger
        .ping(surge_ping::PingSequence(0), b"HOPE")
        .await
        .is_ok()
}

async fn run_probe(
    state: State,
    job: ScanJob,
    ip: Ipv4Addr,
    cancel: CancellationToken,
    backend: Arc<dyn Probe>,
    gentle_pacer: Option<Arc<Mutex<Instant>>>,
) {
    let _task = state.track();
    if let Some(pacer) = gentle_pacer {
        let mut next = pacer.lock().await;
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep_until(*next) => {}
        }
        *next = Instant::now() + GENTLE_SPACING;
    }
    let observation = tokio::select! {
        _ = cancel.cancelled() => return,
        result = timeout(Duration::from_secs(6), backend.probe(ip)) => result.unwrap_or_default(),
    };
    let found = observation.alive;
    if cancel.is_cancelled() {
        return;
    }
    let update = async {
        if found { persist_observation(&state, &job, ip, observation, &cancel).await?; }
        // Share the host-writer gate instead of competing with agent/enrichment writes.
        let _write_guard = state.0.writes.lock().await;
        let row = sqlx::query("UPDATE scans SET scanned=scanned+1,discovered=discovered+? WHERE id=? AND status='running' RETURNING scanned,total")
            .bind(if found {1} else {0}).bind(&job.id).fetch_optional(&state.0.db).await?;
        if let Some(row) = row
            && row.get::<i64,_>("scanned") >= row.get::<i64,_>("total") {
                sqlx::query("UPDATE scans SET status='completed',finished_at=? WHERE id=? AND status='running'").bind(now()).bind(&job.id).execute(&state.0.db).await?;
                state.0.scans.lock().await.remove(&job.id);
            }
        state.changed(); anyhow::Ok(())
    }.await;
    if let Err(error) = update {
        tracing::error!(%error, "scan persistence failed");
        if let Err(terminal_error) = sqlx::query("UPDATE scans SET status='failed',finished_at=?,error='Could not persist scan results' WHERE id=? AND status='running'").bind(now()).bind(&job.id).execute(&state.0.db).await {
            tracing::error!(%terminal_error, scan_id = %job.id, "could not mark scan failed; next scan will recover it");
        }
        cancel.cancel();
        state.0.scans.lock().await.remove(&job.id);
        state.changed();
    }
}

async fn persist_observation(
    state: &State,
    job: &ScanJob,
    ip: Ipv4Addr,
    observation: Observation,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let _guard = state.0.writes.lock().await;
    if cancel.is_cancelled() {
        return Ok(());
    }
    let address = ip.to_string();
    let hosts = state.stored_hosts().await?;
    let candidates: Vec<_> = hosts
        .iter()
        .filter(|host| {
            host.addresses.contains(&address)
                && observation
                    .mac
                    .as_ref()
                    .is_none_or(|mac| host.macs.is_empty() || host.macs.contains(mac))
        })
        .collect();
    // Ambiguous evidence stays available for manual reconciliation. Repeated scans
    // must not create a new duplicate every time the same ambiguity is observed.
    if candidates.len() > 1 {
        return Ok(());
    }
    let is_new = candidates.is_empty();
    let mut host = candidates
        .first()
        .map(|host| (*host).clone())
        .unwrap_or_else(|| {
            let mut host =
                crate::inventory::StoredHost::new(Uuid::new_v4().to_string(), address.clone(), "");
            host.addresses = vec![address];
            host.last_seen = now();
            host
        });
    host.last_seen = now();
    host.discovery_method = observation.method.into();
    if let Some(mac) = observation.mac
        && !host.macs.contains(&mac)
    {
        host.macs.push(mac);
    }
    if !host.network_ids.contains(&job.network_id) {
        host.network_ids.push(job.network_id.clone());
    }
    for service in observation.services {
        host.services.retain(|old| old.port != service.port);
        host.services.push(service);
    }
    state.apply_sightings(&mut host);
    crate::classify::refresh(&mut host, &state.gateway_set());
    // Probing every known device again on each scheduled scan would be the heaviest part of
    // discovery; only new devices, unfinished ones and day-old port scans are enriched.
    let enrich = crate::enrichment::due(&host);
    if enrich {
        host.enrichment_status = hope_protocol::EnrichmentStatus::Pending;
    }
    state.save_host(&host).await?;
    state.changed();
    drop(_guard);
    if is_new {
        crate::notify::new_device(state, &host, &job.network_id).await;
    }
    if enrich {
        state
            .0
            .enrichment
            .enqueue(state.clone(), host.id.clone(), ip, cancel.clone())
            .await;
    }
    Ok(())
}

pub fn capabilities() -> Capabilities {
    let mode = std::env::var("HOPE_DISCOVERY_MODE").unwrap_or_else(|_| {
        if cfg!(target_os = "linux") {
            "native"
        } else {
            "limited"
        }
        .into()
    });
    let arp = cfg!(target_os = "linux") && mode == "native";
    let icmp = surge_ping::Client::new(&surge_ping::Config::default()).is_ok();
    let mut limitations = vec![];
    if !arp {
        limitations.push(if mode == "limited" {
            "Limited discovery mode (for example Docker Desktop): the server cannot see your LAN's layer 2, so ARP, MAC addresses and physical topology are unavailable, and probes reach only what the container network can route to. Run the Linux host-network deployment for full discovery."
        } else {
            "Layer 2 discovery is unavailable. MAC addresses and physical topology may be incomplete."
        }.into());
    }
    if !icmp {
        limitations.push("ICMP is unavailable. Allow raw sockets for ping discovery; TCP probing remains active.".into());
    }
    Capabilities {
        platform: std::env::consts::OS.into(),
        discovery_mode: mode,
        arp,
        icmp,
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeProbe {
        calls: Arc<Mutex<Vec<Ipv4Addr>>>,
    }
    impl Probe for FakeProbe {
        fn probe(&self, ip: Ipv4Addr) -> BoxFuture<'_, Observation> {
            Box::pin(async move {
                self.calls.lock().await.push(ip);
                tokio::time::sleep(Duration::from_millis(50)).await;
                Observation::default()
            })
        }
    }
    async fn fixture() -> (tempfile::TempDir, State, Arc<Mutex<Vec<Ipv4Addr>>>) {
        let directory = tempfile::tempdir().expect("temporary");
        let calls = Arc::new(Mutex::new(vec![]));
        let scheduler = Scheduler::with_backend(
            Arc::new(FakeProbe {
                calls: calls.clone(),
            }),
            Arc::new(ProbeLimiter::new()),
            crate::arp::Arp::new(false),
        );
        let state = State::open_with_scheduler(
            directory.path().join("test.db").to_str().expect("path"),
            "http://localhost:8080".into(),
            "releases".into(),
            Capabilities {
                platform: "test".into(),
                discovery_mode: "limited".into(),
                arp: false,
                icmp: false,
                limitations: vec![],
            },
            Some(scheduler),
        )
        .await
        .expect("state");
        (directory, state, calls)
    }
    #[tokio::test]
    async fn abandoned_running_scan_does_not_block_replacement() {
        let (_directory, state, _) = fixture().await;
        let network = Network {
            id: "orphan-network".into(),
            name: "Test".into(),
            cidr: "192.168.1.0/30".into(),
            enabled: true,
            interval_minutes: 5,
        };
        sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('orphan',?,?,'running',2,0)")
            .bind(&network.id).bind(&network.cidr).execute(&state.0.db).await.expect("orphan");
        start_scheduled(&state, &network)
            .await
            .expect("replacement scan");
        let status: String = sqlx::query_scalar("SELECT status FROM scans WHERE id='orphan'")
            .fetch_one(&state.0.db)
            .await
            .expect("status");
        assert_eq!(status, "interrupted");
        assert!(
            start_scheduled(&state, &network).await.is_err(),
            "live scan must still block duplicates"
        );
    }

    #[tokio::test]
    async fn ambiguous_observations_do_not_create_repeated_duplicates() {
        let (_directory, state, _) = fixture().await;
        for id in ["first", "second"] {
            let host: crate::inventory::StoredHost = serde_json::from_value(serde_json::json!({"id":id,"agent_id":null,"name":id,"addresses":["192.168.1.10"],"macs":[],"network_ids":[],"last_seen":0,"last_report":null,"discovery_method":"tcp","inventory":null,"metrics":null,"collectors":[],"services":[]})).expect("host");
            state.save_host(&host).await.expect("save");
        }
        let job = ScanJob {
            id: "job".into(),
            network_id: "network".into(),
            cidr: "192.168.1.0/24".into(),
            status: ScanStatus::Running,
            scanned: 0,
            total: 254,
            discovered: 0,
            started_at: now(),
            finished_at: None,
            error: None,
        };
        for _ in 0..3 {
            persist_observation(
                &state,
                &job,
                "192.168.1.10".parse().expect("ip"),
                Observation {
                    alive: true,
                    method: "tcp",
                    ..Default::default()
                },
                &CancellationToken::new(),
            )
            .await
            .expect("observation");
        }
        assert_eq!(state.hosts().await.expect("hosts").len(), 2);
    }
    #[tokio::test]
    async fn two_silent_ranges_receive_work_before_either_finishes() {
        let (_directory, state, calls) = fixture().await;
        let first = start(
            &state,
            &Network {
                id: "first".into(),
                name: "First".into(),
                cidr: "192.168.1.0/24".into(),
                enabled: false,
                interval_minutes: 5,
            },
        )
        .await
        .expect("first scan");
        let second = start(
            &state,
            &Network {
                id: "second".into(),
                name: "Second".into(),
                cidr: "192.168.30.0/24".into(),
                enabled: false,
                interval_minutes: 5,
            },
        )
        .await
        .expect("second scan");
        timeout(Duration::from_secs(2), async {
            loop {
                let trace = calls.lock().await;
                let one = trace.iter().any(|ip| ip.octets()[2] == 1);
                let two = trace.iter().any(|ip| ip.octets()[2] == 30);
                drop(trace);
                if one && two {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fair dispatch");
        assert!(
            jobs(&state)
                .await
                .expect("jobs")
                .iter()
                .all(|j| j.scanned < j.total)
        );
        timeout(Duration::from_secs(10), async {
            loop {
                let scans = jobs(&state).await.expect("jobs");
                if scans
                    .iter()
                    .filter(|j| j.id == first.id || j.id == second.id)
                    .all(|j| j.status == ScanStatus::Completed)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("both completed");
        assert_eq!(calls.lock().await.len(), 508);
    }
    struct PacedProbe {
        live: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        starts: Arc<std::sync::Mutex<Vec<Instant>>>,
    }
    impl Probe for PacedProbe {
        fn probe(&self, _: Ipv4Addr) -> BoxFuture<'_, Observation> {
            Box::pin(async move {
                use std::sync::atomic::Ordering;
                self.starts.lock().expect("start lock").push(Instant::now());
                let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
                self.live.fetch_sub(1, Ordering::SeqCst);
                Observation::default()
            })
        }
    }
    #[tokio::test]
    async fn scheduled_scans_are_paced_and_manual_scans_are_not() {
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let starts = Arc::new(std::sync::Mutex::new(vec![]));
        let directory = tempfile::tempdir().expect("temporary");
        let scheduler = Scheduler::with_backend(
            Arc::new(PacedProbe {
                live: Default::default(),
                peak: peak.clone(),
                starts: starts.clone(),
            }),
            Arc::new(ProbeLimiter::new()),
            crate::arp::Arp::new(false),
        );
        let state = State::open_with_scheduler(
            directory.path().join("test.db").to_str().expect("path"),
            "http://localhost:8080".into(),
            "releases".into(),
            Capabilities {
                platform: "test".into(),
                discovery_mode: "limited".into(),
                arp: false,
                icmp: false,
                limitations: vec![],
            },
            Some(scheduler),
        )
        .await
        .expect("state");
        let network = |id: &str, cidr: &str| Network {
            id: id.into(),
            name: id.into(),
            cidr: cidr.into(),
            enabled: true,
            interval_minutes: 5,
        };
        let gentle = start_scheduled(&state, &network("gentle", "192.168.1.0/28"))
            .await
            .expect("scheduled");
        timeout(Duration::from_secs(10), async {
            loop {
                if jobs(&state)
                    .await
                    .expect("jobs")
                    .iter()
                    .any(|job| job.id == gentle.id && job.status == ScanStatus::Completed)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scheduled scan completed");
        let gentle_peak = peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(gentle_peak <= GENTLE_PARALLEL, "peak {gentle_peak}");
        let mut times = starts.lock().expect("start lock").clone();
        assert_eq!(times.len(), gentle.total as usize);
        times.sort_unstable();
        assert!(
            times
                .windows(2)
                .all(|w| w[1].duration_since(w[0]) >= GENTLE_SPACING - Duration::from_millis(10)),
            "probe starts were not paced: {times:?}"
        );
        // A manual scan of another range is not held to the pace.
        peak.store(0, std::sync::atomic::Ordering::SeqCst);
        start(&state, &network("manual", "192.168.2.0/27"))
            .await
            .expect("manual");
        timeout(Duration::from_secs(2), async {
            while peak.load(std::sync::atomic::Ordering::SeqCst) <= GENTLE_PARALLEL {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("manual probes exceed the gentle concurrency cap");
        assert!(peak.load(std::sync::atomic::Ordering::SeqCst) > GENTLE_PARALLEL);
    }
    #[tokio::test]
    async fn cancellation_stops_dispatch_and_persists_status() {
        let (_directory, state, calls) = fixture().await;
        let job = start(
            &state,
            &Network {
                id: "network".into(),
                name: "Network".into(),
                cidr: "192.168.0.0/20".into(),
                enabled: false,
                interval_minutes: 5,
            },
        )
        .await
        .expect("scan");
        timeout(Duration::from_secs(2), async {
            while calls.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scan started probing before cancellation");
        cancel(&state, &job.id).await.expect("cancel");
        timeout(Duration::from_secs(2), async {
            while state
                .0
                .active_tasks
                .load(std::sync::atomic::Ordering::SeqCst)
                != 0
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cancelled probes completed");
        let count = calls.lock().await.len();
        // Allow several scheduler ticks to catch accidental dispatch from a cancelled batch.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(calls.lock().await.len(), count);
        assert_eq!(
            jobs(&state).await.expect("jobs")[0].status,
            ScanStatus::Cancelled
        );
    }
    #[tokio::test]
    async fn limiter_bounds_starts_and_live_probes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let limiter = Arc::new(ProbeLimiter::new());
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let times = Arc::new(std::sync::Mutex::new(vec![]));
        let mut work = FuturesUnordered::new();
        for _ in 0..100 {
            let limiter = limiter.clone();
            let live = live.clone();
            let peak = peak.clone();
            let times = times.clone();
            work.push(async move {
                let _permit = limiter.acquire().await.expect("permit");
                times.lock().expect("start lock").push(Instant::now());
                let count = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(400)).await;
                live.fetch_sub(1, Ordering::SeqCst);
            });
        }
        while work.next().await.is_some() {}
        assert!(peak.load(Ordering::SeqCst) <= 64);
        let mut times = times.lock().expect("start lock");
        times.sort_unstable();
        assert_eq!(times.len(), 100);
        let elapsed = times.last().unwrap().duration_since(times[0]);
        let minimum = Duration::from_millis((times.len() as u64 - 1) * 5);
        assert!(
            elapsed + Duration::from_millis(10) >= minimum,
            "100 probes started in {elapsed:?}; expected at least {minimum:?}"
        );
    }
    #[tokio::test]
    async fn tcp_refusal_is_neither_liveness_nor_an_open_service() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let port = listener.local_addr().expect("address").port();
        drop(listener);
        let limiter = ProbeLimiter::new();
        assert!(tcp_probe(Ipv4Addr::LOCALHOST, port, &limiter).await == TcpResult::Refused);
        assert!(!tcp(Ipv4Addr::LOCALHOST, port, &limiter).await);
    }
    #[test]
    fn only_ping_arp_or_an_open_port_prove_a_device() {
        use TcpResult::*;
        let refused = [Refused, Refused, Refused];
        assert!(!alive(false, false, &refused));
        assert!(!alive(false, false, &[Unanswered, Refused, Unanswered]));
        assert!(alive(true, false, &refused));
        assert!(alive(false, true, &refused));
        assert!(alive(false, false, &[Refused, Open, Unanswered]));
    }
}
