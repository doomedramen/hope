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
async fn enrich(work: Work) {
    let Work {
        state,
        id,
        ip,
        cancel,
    } = work;
    status(&state, &id, EnrichmentStatus::Running).await;
    let result = tokio::select! {
        _ = cancel.cancelled() => { status(&state, &id, EnrichmentStatus::Cancelled).await; return; }
        result = timeout(Duration::from_secs(12), async {
            let mut services = vec![];
            for (port, name) in [(22, "SSH"), (80, "HTTP"), (443, "HTTPS"), (445, "SMB"), (3389, "RDP"), (8080, "HTTP alternate")] {
                if crate::scanner::tcp(ip, port, &state.0.scheduler.limiter).await {
                    services.push(Service { port, name: name.into(), observed_at: now() });
                }
            }
            (services, reverse_dns(ip).await)
        }) => result,
    };
    let Ok((services, hostname)) = result else {
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
        // Keep previous service observations when the current probe is unanswered.
        for service in services {
            host.services.retain(|old| old.port != service.port);
            host.services.push(service);
        }
        host.enrichment_status = EnrichmentStatus::Completed;
        if host.agent_id.is_none()
            && let Some(name) = hostname
        {
            host.name = name;
        }
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
