//! The background discovery loop. Every minute it starts scheduled scans that are due (gently
//! paced, see `scanner`), every five minutes it listens for mDNS and SSDP announcements and reads
//! the routing table, every hour it traces routed ranges, and after each step it brings vendors,
//! device types, names and parents up to date and evaluates notifications.
use crate::{
    api, classify, fingerprint, notify, scanner,
    state::{State, now},
    topology,
};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

const SWEEP_EVERY: Duration = Duration::from_secs(300);
const TRACE_EVERY: Duration = Duration::from_secs(3600);

pub fn spawn(state: State) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_sweep: Option<Instant> = None;
        let mut last_trace: Option<Instant> = None;
        let mut traces: HashMap<String, Vec<Ipv4Addr>> = HashMap::new();
        // Let the server settle (and agents report) before the first round.
        tokio::select! {
            _ = state.0.shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(20)) => {}
        }
        loop {
            if let Err(error) = scheduled_scans(&state).await {
                tracing::warn!(%error, "scheduled scans could not start");
            }
            if last_sweep.is_none_or(|t| t.elapsed() >= SWEEP_EVERY) {
                last_sweep = Some(Instant::now());
                sweep(&state).await;
            }
            if last_trace.is_none_or(|t| t.elapsed() >= TRACE_EVERY) {
                last_trace = Some(Instant::now());
                traces = trace_routed(&state).await;
            }
            if let Err(error) = refresh_hosts(&state, &traces).await {
                tracing::warn!(%error, "device refresh failed");
            }
            if let Err(error) = notify::evaluate(&state).await {
                tracing::warn!(%error, "notification evaluation failed");
            }
            tokio::select! {
                _ = state.0.shutdown.cancelled() => break,
                _ = timer.tick() => {}
            }
        }
    });
}

/// Start a gentle scan of every enabled range whose interval has elapsed since its last scan.
async fn scheduled_scans(state: &State) -> anyhow::Result<()> {
    for network in api::networks(state)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?
    {
        if !network.enabled || state.0.shutdown.is_cancelled() {
            continue;
        }
        let last: Option<f64> =
            sqlx::query_scalar("SELECT MAX(started_at) FROM scans WHERE network_id=?")
                .bind(&network.id)
                .fetch_one(&state.0.db)
                .await?;
        // A few seconds of slack so a one-minute timer does not push a 5-minute range to 6.
        let due = last.is_none_or(|t| now() - t >= network.interval_minutes as f64 * 60.0 - 5.0);
        if due {
            let _ = scanner::start_scheduled(state, &network).await;
        }
    }
    Ok(())
}

/// Listen for mDNS and SSDP announcements and keep the latest per address.
async fn sweep(state: &State) {
    let (mdns, ssdp) = tokio::join!(
        fingerprint::mdns_sweep(Duration::from_secs(3)),
        fingerprint::ssdp_sweep(&state.0.http, Duration::from_secs(3)),
    );
    let mut merged = mdns;
    for (ip, seen) in ssdp {
        let entry = merged.entry(ip).or_default();
        entry.ssdp_server = seen.ssdp_server;
        entry.upnp_device_type = seen.upnp_device_type;
        entry.upnp_friendly_name = seen.upnp_friendly_name;
        entry.upnp_manufacturer = seen.upnp_manufacturer;
        entry.upnp_model = seen.upnp_model;
    }
    tracing::debug!(devices = merged.len(), "mDNS/SSDP sweep");
    if let Ok(mut sightings) = state.0.sightings.lock() {
        *sightings = merged;
    }
}

/// Trace each enabled range the server is not attached to.
async fn trace_routed(state: &State) -> HashMap<String, Vec<Ipv4Addr>> {
    let routes = topology::routes();
    let mut traces = HashMap::new();
    let Ok(networks) = api::networks(state).await else {
        return traces;
    };
    let hosts = state.hosts().await.unwrap_or_default();
    for network in networks.into_iter().filter(|n| n.enabled) {
        let Ok(net) = network.cidr.parse::<ipnet::Ipv4Net>() else {
            continue;
        };
        if routes.attached.iter().any(|a| a.contains(&net.network())) {
            continue;
        }
        // Prefer a device known to answer; fall back to the first address of the range.
        let target = hosts
            .iter()
            .filter(|h| h.network_ids.contains(&network.id))
            .flat_map(|h| h.addresses.iter())
            .filter_map(|a| a.parse::<Ipv4Addr>().ok())
            .find(|ip| net.contains(ip))
            .or_else(|| net.hosts().next());
        if let Some(target) = target {
            let hops = topology::trace(target, 8).await;
            if !hops.is_empty() {
                traces.insert(network.id, hops);
            }
        }
    }
    traces
}

/// Bring every host's vendor, evidence, type, name and parent up to date; save only changes.
async fn refresh_hosts(
    state: &State,
    traces: &HashMap<String, Vec<Ipv4Addr>>,
) -> anyhow::Result<()> {
    let networks = api::networks(state)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    let layout = topology::layout(&networks, &topology::routes(), traces);
    let gateways = layout.addresses();
    if let Ok(mut known) = state.0.gateways.lock() {
        *known = gateways.clone();
    }
    topology::ensure_router_hosts(state, &layout).await?;
    let _guard = state.0.writes.lock().await;
    let mut hosts = state.hosts().await?;
    let mut changed = vec![false; hosts.len()];
    for (index, host) in hosts.iter_mut().enumerate() {
        let before = host.evidence.clone();
        state.apply_sightings(host);
        let evidence_changed = before != host.evidence;
        changed[index] = classify::refresh(host, &gateways) || evidence_changed;
    }
    for index in topology::assign(&mut hosts, &layout, &networks) {
        changed[index] = true;
    }
    let mut any = false;
    for (host, changed) in hosts.iter().zip(changed) {
        if changed {
            state.save_host(host).await?;
            any = true;
        }
    }
    if any {
        state.changed();
    }
    Ok(())
}
