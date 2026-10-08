use crate::{
    error::{ApiError, ApiResult},
    state::{State, now},
};
use futures::{StreamExt, stream::FuturesUnordered};
use hope_protocol::{Capabilities, Host, Network, ScanJob, Service};
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
        let (sender, mut receiver) = mpsc::channel::<Batch>(32);
        let limiter = Arc::new(ProbeLimiter {
            next: Mutex::new(Instant::now()),
            sockets: Semaphore::new(64),
        });
        let scheduler = Self {
            sender,
            limiter: limiter.clone(),
            arp: crate::arp::Arp::new(
                std::env::var("HOPE_DISCOVERY_MODE").as_deref() != Ok("limited"),
            ),
        };
        tokio::spawn(async move {
            let mut queue = VecDeque::<Batch>::new();
            let mut work = FuturesUnordered::new();
            let mut tick = tokio::time::interval(Duration::from_millis(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let ping = surge_ping::Client::new(&surge_ping::Config::default())
                .ok()
                .map(Arc::new);
            loop {
                tokio::select! {
                    batch = receiver.recv() => match batch { Some(batch) => queue.push_back(batch), None => break },
                    Some(()) = work.next(), if !work.is_empty() => {},
                    _ = tick.tick(), if !queue.is_empty() && work.len() < 64 => {
                        if let Some(mut batch) = queue.pop_front()
                            && !batch.cancel.is_cancelled()
                                && let Some(ip) = batch.ips.pop_front() {
                                    work.push(run_probe(batch.state.clone(), batch.job.clone(), ip, batch.cancel.clone(), limiter.clone(), ping.clone()));
                                    if !batch.ips.is_empty() { queue.push_back(batch); }
                                }
                    }
                }
            }
        });
        scheduler
    }
}

pub(crate) struct ProbeLimiter {
    next: Mutex<Instant>,
    sockets: Semaphore,
}
impl ProbeLimiter {
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

pub async fn start(state: &State, network: &Network) -> ApiResult<ScanJob> {
    let net = parse_range(&network.cidr)?;
    let mut active = state.0.scans.lock().await;
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
        status: "running".into(),
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
    .bind(&job.status)
    .bind(job.total)
    .bind(job.started_at)
    .execute(&state.0.db)
    .await?;
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
    let rows = sqlx::query("SELECT * FROM scans ORDER BY started_at DESC LIMIT 100")
        .fetch_all(&state.0.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ScanJob {
            id: r.get("id"),
            network_id: r.get("network_id"),
            cidr: r.get("cidr"),
            status: r.get("status"),
            scanned: r.get::<i64, _>("scanned") as u32,
            total: r.get::<i64, _>("total") as u32,
            discovered: r.get::<i64, _>("discovered") as u32,
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
            error: r.get("error"),
        })
        .collect())
}

pub(crate) async fn tcp(ip: Ipv4Addr, port: u16, limiter: &ProbeLimiter) -> bool {
    let Ok(_permit) = limiter.acquire().await else {
        return false;
    };
    matches!(
        timeout(
            Duration::from_millis(750),
            TcpStream::connect(SocketAddr::new(IpAddr::V4(ip), port))
        )
        .await,
        Ok(Ok(_))
    )
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
    limiter: Arc<ProbeLimiter>,
    client: Option<Arc<surge_ping::Client>>,
) {
    let arp = state.0.scheduler.arp.clone();
    let result = tokio::select! {
        _ = cancel.cancelled() => return,
        result = async { tokio::join!(ping(ip, &limiter, client.as_deref()), tcp(ip,22,&limiter), tcp(ip,443,&limiter), async {
            if arp.on_link(ip) { let _permit=limiter.acquire().await.ok()?; arp.probe(ip).await } else { None }
        }) } => result,
    };
    let mac = result.3;
    let found = result.0 || result.1 || result.2 || mac.is_some();
    if cancel.is_cancelled() {
        return;
    }
    let update = async {
        if found {
            let _guard = state.0.writes.lock().await;
            let mut hosts = state.hosts().await?;
            let matching: Vec<_> = hosts.iter_mut().filter(|h| h.addresses.contains(&ip.to_string()) && (mac.is_none() || h.macs.is_empty() || h.macs.contains(mac.as_ref().unwrap_or(&String::new())))).collect();
            let mut host = if matching.len() == 1 { matching[0].clone() } else {
                Host { id:Uuid::new_v4().to_string(),agent_id:None,name:ip.to_string(),addresses:vec![ip.to_string()],macs:vec![],network_ids:vec![],last_seen:now(),last_report:None,last_collected_at:None,agent_revoked:false,discovery_method:String::new(),inventory:None,metrics:None,collectors:vec![],services:vec![] }
            };
            host.last_seen = now();
            host.discovery_method = if result.0 { "icmp" } else if mac.is_some() { "arp" } else { "tcp" }.into();
            if let Some(mac) = mac && !host.macs.contains(&mac) { host.macs.push(mac); }
            if !host.network_ids.contains(&job.network_id) { host.network_ids.push(job.network_id.clone()); }
            for (open, port, name) in [(result.1,22,"SSH"),(result.2,443,"HTTPS")] {
                if open && !host.services.iter().any(|s| s.port == port) { host.services.push(Service {port,name:name.into(),observed_at:now()}); }
            }
            state.save_host(&host).await?;
            // Enrichment has its own bounded queue and never holds discovery completion open.
            crate::enrichment::enqueue(state.clone(), host.id.clone(), ip);
        }
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
        let _ = sqlx::query("UPDATE scans SET status='failed',finished_at=?,error='Could not persist scan results' WHERE id=? AND status='running'").bind(now()).bind(&job.id).execute(&state.0.db).await;
        cancel.cancel();
        state.0.scans.lock().await.remove(&job.id);
        state.changed();
    }
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
        limitations.push("Layer 2 discovery is unavailable. MAC addresses and physical topology may be incomplete.".into());
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
