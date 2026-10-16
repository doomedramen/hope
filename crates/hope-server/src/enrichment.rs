use crate::state::{State, now};
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use hope_protocol::{EnrichmentStatus, Service};
use std::{collections::HashSet, net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

struct Work {
    state: State,
    id: String,
    ip: Ipv4Addr,
    cancel: CancellationToken,
}
type Worker = Arc<dyn Fn(Work) -> BoxFuture<'static, ()> + Send + Sync>;
#[derive(Clone)]
pub struct Queue {
    sender: mpsc::Sender<Work>,
    pending: Arc<Mutex<HashSet<String>>>,
}
impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}
impl Queue {
    pub fn new() -> Self {
        Self::with_worker(Arc::new(|work| Box::pin(enrich(work))))
    }
    fn with_worker(worker: Worker) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Work>(256);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let active_ids = pending.clone();
        tokio::spawn(async move {
            let mut active = FuturesUnordered::new();
            loop {
                tokio::select! {
                    work = receiver.recv(), if active.len() < 8 => {
                        let Some(work) = work else { while active.next().await.is_some() {} break; };
                        let id = work.id.clone(); let pending = active_ids.clone(); let worker = worker.clone(); let guard = work.state.track();
                        active.push(async move { let _guard = guard; worker(work).await; pending.lock().await.remove(&id); });
                    }
                    Some(()) = active.next(), if !active.is_empty() => {}
                }
            }
        });
        Self { sender, pending }
    }
    pub async fn enqueue(&self, state: State, id: String, ip: Ipv4Addr, cancel: CancellationToken) {
        if !self.pending.lock().await.insert(id.clone()) {
            return;
        }
        if self
            .sender
            .send(Work {
                state,
                id: id.clone(),
                ip,
                cancel,
            })
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
        }
    }
}
async fn status(state: &State, id: &str, status: EnrichmentStatus) {
    let _guard = state.0.writes.lock().await;
    if let Ok(Some(mut host)) = state.host(id).await {
        host.enrichment_status = status;
        if let Err(error) = state.save_host(&host).await {
            tracing::warn!(%error, "enrichment status could not be saved");
        }
        state.changed();
    }
}
/// How often a known device's common ports are scanned again.
pub const PORT_RESCAN_SECONDS: f64 = 24.0 * 3600.0;

/// Whether a host needs enrichment: never finished, or its port scan is older than a day.
pub fn due(host: &hope_protocol::Host) -> bool {
    !matches!(host.enrichment_status, EnrichmentStatus::Completed)
        || host
            .ports_scanned_at
            .is_none_or(|at| now() - at > PORT_RESCAN_SECONDS)
}

async fn enrich(work: Work) {
    let Work {
        state,
        id,
        ip,
        cancel,
    } = work;
    status(&state, &id, EnrichmentStatus::Running).await;
    let rescan = state.host(&id).await.ok().flatten().is_some_and(|h| {
        h.ports_scanned_at
            .is_none_or(|at| now() - at > PORT_RESCAN_SECONDS)
    });
    let result = tokio::select! {
        _ = cancel.cancelled() => { status(&state, &id, EnrichmentStatus::Cancelled).await; return; }
        result = timeout(Duration::from_secs(30), async {
            let services = if rescan {
                let limiter = &state.0.scheduler.limiter;
                let probes = crate::fingerprint::COMMON_PORTS.iter().map(|(port, name)| async move {
                    crate::scanner::tcp(ip, *port, limiter).await.then(|| Service { port: *port, name: (*name).into(), observed_at: now() })
                });
                Some(futures::future::join_all(probes).await.into_iter().flatten().collect::<Vec<_>>())
            } else {
                None
            };
            let banner = match &services {
                Some(open) if !open.is_empty() => {
                    let ports: Vec<u16> = open.iter().map(|s| s.port).collect();
                    Some(crate::fingerprint::http_banner(&state.0.http, ip, &ports).await)
                }
                Some(_) => Some((None, None)),
                None => None,
            };
            (services, banner, reverse_dns(ip).await)
        }) => result,
    };
    let Ok((services, banner, hostname)) = result else {
        status(&state, &id, EnrichmentStatus::Failed).await;
        return;
    };
    let _guard = state.0.writes.lock().await;
    if cancel.is_cancelled() {
        drop(_guard);
        status(&state, &id, EnrichmentStatus::Cancelled).await;
        return;
    }
    if let Ok(Some(mut host)) = state.host(&id).await {
        if let Some(open) = services {
            // A full scan is authoritative for the common ports; anything else (seen by an agent or
            // a quick probe outside the list) is kept.
            host.services
                .retain(|old| crate::fingerprint::port_name(old.port).is_none());
            host.services.extend(open);
            host.services.sort_by_key(|s| s.port);
            host.ports_scanned_at = Some(now());
        }
        if let Some((title, server)) = banner {
            host.evidence.http_title = title;
            host.evidence.http_server = server;
        }
        if hostname.is_some() {
            host.evidence.dns_name = hostname.clone();
        }
        host.enrichment_status = EnrichmentStatus::Completed;
        if host.agent_id.is_none()
            && let Some(name) = hostname
        {
            host.name = name;
        }
        state.apply_sightings(&mut host);
        crate::classify::refresh(&mut host, &state.gateway_set());
        if let Err(error) = state.save_host(&host).await {
            tracing::warn!(%error, "enrichment could not be saved");
        }
        state.changed();
    }
}
async fn reverse_dns(ip: Ipv4Addr) -> Option<String> {
    // Killable native resolvers keep DNS independent of the discovery deadline.
    let command = if cfg!(target_os = "macos") {
        let mut command = tokio::process::Command::new("dscacheutil");
        command.args(["-q", "host", "-a", "ip_address", &ip.to_string()]);
        command
    } else {
        let mut command = tokio::process::Command::new("getent");
        command.args(["hosts", &ip.to_string()]);
        command
    };
    let output = bounded_output(command, Duration::from_secs(2)).await?;
    let name = if cfg!(target_os = "macos") {
        output
            .lines()
            .find_map(|line| line.strip_prefix("name: "))?
    } else {
        output.split_whitespace().nth(1)?
    };
    let name = name.trim().trim_end_matches('.');
    (!name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control))
        .then(|| name.into())
}

async fn bounded_output(
    mut command: tokio::process::Command,
    deadline: Duration,
) -> Option<String> {
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
    let result = timeout(deadline, async {
        let mut output = vec![];
        child
            .stdout
            .take()?
            .take(16385)
            .read_to_end(&mut output)
            .await
            .ok()?;
        if output.len() > 16384 {
            return None;
        }
        let status = child.wait().await.ok()?;
        if !status.success() {
            return None;
        }
        String::from_utf8(output).ok()
    })
    .await;
    match result {
        Ok(Some(output)) => Some(output),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn resolver_deadlines_and_output_limits_are_enforced() {
        let mut slow = tokio::process::Command::new("sh");
        slow.args(["-c", "exec sleep 10"]);
        let start = tokio::time::Instant::now();
        assert!(
            bounded_output(slow, Duration::from_millis(30))
                .await
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        let mut noisy = tokio::process::Command::new("sh");
        noisy.args(["-c", "exec yes oversized-output"]);
        assert!(
            bounded_output(noisy, Duration::from_secs(1))
                .await
                .is_none()
        );
    }
    #[tokio::test]
    async fn all_250_hosts_receive_work_without_exceeding_eight_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let completed = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let queue = Queue::with_worker({
            let completed = completed.clone();
            let active = active.clone();
            let peak = peak.clone();
            Arc::new(move |_| {
                let completed = completed.clone();
                let active = active.clone();
                let peak = peak.clone();
                Box::pin(async move {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    completed.fetch_add(1, Ordering::SeqCst);
                })
            })
        });
        let directory = tempfile::tempdir().expect("temporary");
        let state = State::open(
            directory.path().join("test.db").to_str().expect("path"),
            "http://localhost:8080".into(),
            "releases".into(),
            crate::scanner::capabilities(),
        )
        .await
        .expect("state");
        for id in 0..250 {
            queue
                .enqueue(
                    state.clone(),
                    id.to_string(),
                    Ipv4Addr::LOCALHOST,
                    CancellationToken::new(),
                )
                .await;
        }
        timeout(Duration::from_secs(5), async {
            while completed.load(Ordering::SeqCst) < 250 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("all work completed");
        assert!(peak.load(Ordering::SeqCst) <= 8);
        assert_eq!(completed.load(Ordering::SeqCst), 250);
    }
}
