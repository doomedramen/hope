//! Opt-in device and TCP availability checks, evaluated by the notification loop.
use crate::{
    error::{ApiError, ApiResult},
    notify::{AGENT_STALE_SECONDS, Problem},
    state::State,
};
use axum::{
    Json,
    extract::{Path, State as Extract},
};
use futures::{StreamExt, stream};
use hope_protocol::{Host, UptimeSettings};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

pub async fn configure(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(mut input): Json<UptimeSettings>,
) -> ApiResult<Json<Host>> {
    if input.ports.len() > 32 || input.ports.contains(&0) {
        return Err(ApiError::bad(
            "Choose at most 32 TCP ports between 1 and 65535",
        ));
    }
    input.ports.sort_unstable();
    input.ports.dedup();
    if input.enabled && !input.device && input.ports.is_empty() {
        return Err(ApiError::bad("Enable device downtime or choose a TCP port"));
    }
    let _guard = state.0.writes.lock().await;
    let mut host = state.host(&id).await?.ok_or_else(ApiError::not_found)?;
    if input.enabled && !input.ports.is_empty() && addresses(&host).is_empty() {
        return Err(ApiError::bad("This device has no IP address to check"));
    }
    if host.uptime != input {
        // Configuration changes are not recoveries. Reset the debounce and open alerts silently.
        sqlx::query("DELETE FROM alerts WHERE host_id=? AND kind='uptime'")
            .bind(&id)
            .execute(&state.0.db)
            .await?;
        state.set_meta(&format!("uptime:{id}"), "{}").await?;
        host.uptime = input;
        state.save_host(&host).await?;
        state.changed();
    }
    Ok(Json(host))
}

fn addresses(host: &Host) -> Vec<IpAddr> {
    host.addresses
        .iter()
        .filter_map(|address| address.parse().ok())
        .collect()
}

/// Unknown discovery state is not evidence of downtime.
fn device_down(host: &Host, now: f64, scans: &HashMap<String, (f64, u32)>) -> bool {
    if host.agent_id.is_some() && !host.agent_revoked && !host.agent_uninstalled {
        return host
            .last_report
            .is_none_or(|at| now - at > AGENT_STALE_SECONDS);
    }
    host.network_ids
        .iter()
        .filter_map(|id| scans.get(id))
        .any(|(finished, interval)| {
            *finished > host.last_seen
                && now - host.last_seen > (2 * u64::from(*interval) * 60).max(900) as f64
        })
}

async fn port_up(addresses: &[IpAddr], port: u16) -> bool {
    tokio::time::timeout(Duration::from_secs(3), async {
        stream::iter(addresses.iter().copied().map(|address| async move {
            tokio::net::TcpStream::connect(SocketAddr::new(address, port))
                .await
                .is_ok()
        }))
        .buffer_unordered(4)
        .any(|up| async move { up })
        .await
    })
    .await
    .unwrap_or(false)
}

/// Require two failed evaluations before opening an alert; retain counters across restarts.
fn update_failure(count: &mut u8, down: bool) -> bool {
    *count = if down { count.saturating_add(1) } else { 0 };
    *count >= 2
}

pub async fn problems(
    state: &State,
    hosts: &[Host],
    now: f64,
    scans: &HashMap<String, (f64, u32)>,
) -> anyhow::Result<Vec<(Problem, String)>> {
    let results = stream::iter(
        hosts
            .iter()
            .filter(|h| h.uptime.enabled)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|host| async move {
                let mut checks = Vec::new();
                if host.uptime.device {
                    checks.push((
                        "device".to_string(),
                        if (host.agent_id.is_some()
                            && !host.agent_revoked
                            && !host.agent_uninstalled)
                            || host.network_ids.iter().any(|id| scans.contains_key(id))
                            || now - host.last_seen <= AGENT_STALE_SECONDS
                        {
                            Some(device_down(&host, now, scans))
                        } else {
                            None
                        },
                        format!("{} is down", host.name),
                    ));
                }
                let ips = addresses(&host);
                // No current address means unknown, not a healthy port or a proven outage.
                let port_checks = stream::iter(host.uptime.ports.clone().into_iter().map(|port| {
                    let ips = ips.clone();
                    let name = host.name.clone();
                    async move {
                        let down = if ips.is_empty() {
                            None
                        } else {
                            Some(!port_up(&ips, port).await)
                        };
                        (
                            format!("tcp:{port}"),
                            down,
                            format!("TCP port {port} on {name} is down"),
                        )
                    }
                }))
                .buffer_unordered(4)
                .collect::<Vec<_>>()
                .await;
                checks.extend(port_checks);
                (host, checks)
            }),
    )
    .buffer_unordered(16)
    .collect::<Vec<_>>()
    .await;
    let _guard = state.0.writes.lock().await;
    let mut out = Vec::new();
    for (host, checks) in results {
        // A settings edit during a probe must not restore a disabled monitor.
        if state
            .host(&host.id)
            .await?
            .is_none_or(|h| h.uptime != host.uptime)
        {
            continue;
        }
        let key = format!("uptime:{}", host.id);
        let mut failures: HashMap<String, u8> = state
            .meta(&key)
            .await?
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        for (check, down, message) in checks {
            let count = failures.entry(check.clone()).or_default();
            let open = down.map_or(*count >= 2, |down| update_failure(count, down));
            if open {
                out.push((
                    Problem {
                        key: format!("{}:uptime:{check}", host.id),
                        offline: true,
                        critical: true,
                        message,
                    },
                    host.id.clone(),
                ));
            }
        }
        state
            .set_meta(&key, &serde_json::to_string(&failures)?)
            .await?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_debounce_and_reset() {
        let mut count = 0;
        assert!(!update_failure(&mut count, true));
        assert!(update_failure(&mut count, true));
        assert!(!update_failure(&mut count, false));
        assert!(!update_failure(&mut count, true));
    }

    #[test]
    fn downtime_requires_fresh_evidence_for_any_device_type() {
        let mut host = Host::new("phone".into(), "Phone".into(), "icmp");
        host.last_seen = 100.0;
        host.network_ids = vec!["lan".into()];
        assert!(!device_down(&host, 5000.0, &HashMap::new()));
        assert!(!device_down(
            &host,
            5000.0,
            &HashMap::from([("lan".into(), (50.0, 5))])
        ));
        assert!(device_down(
            &host,
            5000.0,
            &HashMap::from([("lan".into(), (4900.0, 5))])
        ));
    }

    #[tokio::test]
    async fn tcp_checks_use_live_connectivity() {
        // Other tests run in parallel and can immediately re-bind a freed
        // ephemeral port, so retry with a fresh port until one stays closed.
        for _ in 0..10 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            assert!(port_up(&[address.ip()], address.port()).await);
            drop(listener);
            if !port_up(&[address.ip()], address.port()).await {
                return;
            }
        }
        panic!("a freed ephemeral port stayed reachable on every attempt");
    }
}
