use crate::state::{State, now};
use hope_protocol::Service;
use std::{
    net::Ipv4Addr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{sync::Semaphore, time::timeout};

static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub fn enqueue(state: State, id: String, ip: Ipv4Addr) {
    // Dropping enrichment under load is preferable to an unbounded background queue.
    let Ok(permit) = LIMIT
        .get_or_init(|| Arc::new(Semaphore::new(8)))
        .clone()
        .try_acquire_owned()
    else {
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        let mut services = vec![];
        for (port, name) in [
            (22, "SSH"),
            (80, "HTTP"),
            (443, "HTTPS"),
            (445, "SMB"),
            (3389, "RDP"),
            (8080, "HTTP alternate"),
        ] {
            if matches!(
                timeout(
                    Duration::from_millis(500),
                    tokio::net::TcpStream::connect((ip, port))
                )
                .await,
                Ok(Ok(_))
            ) {
                services.push(Service {
                    port,
                    name: name.into(),
                    observed_at: now(),
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let hostname = reverse_dns(ip).await;
        let _guard = state.0.writes.lock().await;
        if let Ok(Some(mut host)) = state.host(&id).await {
            host.services = services;
            if host.agent_id.is_none()
                && let Some(name) = hostname {
                    host.name = name;
                }
            if let Err(error) = state.save_host(&host).await {
                tracing::warn!(%error,"enrichment could not be saved");
            }
            state.changed();
        }
    });
}

async fn reverse_dns(ip: Ipv4Addr) -> Option<String> {
    // A subprocess can be killed on deadline; blocking libc resolver calls cannot.
    let mut command = tokio::process::Command::new("getent");
    command.args(["hosts", &ip.to_string()]).kill_on_drop(true);
    let output = timeout(Duration::from_secs(2), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .nth(1)
        .map(|s| s.trim_end_matches('.').to_string())
}
