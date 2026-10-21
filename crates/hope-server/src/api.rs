use crate::{
    error::{ApiError, ApiResult},
    scanner,
    state::State,
};
use axum::{
    Json,
    extract::{Path, State as Extract},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::StreamExt;
use hope_protocol::OkResponse;
use hope_protocol::{Bootstrap, Host, MapState, Network};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

pub async fn networks(state: &State) -> ApiResult<Vec<Network>> {
    Ok(sqlx::query("SELECT * FROM networks ORDER BY name")
        .fetch_all(&state.0.db)
        .await?
        .into_iter()
        .map(|r| Network {
            id: r.get("id"),
            name: r.get("name"),
            cidr: r.get("cidr"),
            enabled: r.get("enabled"),
            interval_minutes: r.get::<i64, _>("interval_minutes") as u32,
        })
        .collect())
}
pub async fn get_host(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    Ok(Json(
        state.host(&id).await?.ok_or_else(ApiError::not_found)?,
    ))
}
pub async fn retire_host(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    set_retired(&state, &id, true).await.map(Json)
}
pub async fn restore_host(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    set_retired(&state, &id, false).await.map(Json)
}
async fn set_retired(state: &State, id: &str, retired: bool) -> ApiResult<Host> {
    let _guard = state.0.writes.lock().await;
    let host = state.host(id).await?.ok_or_else(ApiError::not_found)?;
    let mut identities = HashSet::from([host.id.clone()]);
    identities.extend(host.source_device_id.iter().cloned());
    let mut tx = state.write_transaction().await?;
    for identity in identities {
        if retired {
            sqlx::query("INSERT INTO retired_devices(device_id,retired_at) VALUES(?,?) ON CONFLICT(device_id) DO UPDATE SET retired_at=excluded.retired_at")
                .bind(identity)
                .bind(crate::state::now())
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("DELETE FROM retired_devices WHERE device_id=?")
                .bind(identity)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    state.changed();
    state.host(&host.id).await?.ok_or_else(ApiError::not_found)
}
pub async fn matching_networks(state: &State, addresses: &[String]) -> ApiResult<Vec<String>> {
    Ok(networks(state)
        .await?
        .into_iter()
        .filter(|n| {
            n.cidr.parse::<ipnet::Ipv4Net>().is_ok_and(|net| {
                addresses.iter().any(|s| {
                    s.parse::<std::net::Ipv4Addr>()
                        .is_ok_and(|ip| net.contains(&ip))
                })
            })
        })
        .map(|n| n.id)
        .collect())
}
pub async fn bootstrap(Extract(state): Extract<State>) -> ApiResult<Json<Bootstrap>> {
    let current_release_id = crate::releases::current_linux_x86_64(&state.0.release_dir)
        .await
        .map(|(id, _)| id);
    let agent_auto_update_blocked_release_id = state
        .meta("auto_update_blocked_release_id")
        .await?
        .filter(|id| !id.is_empty() && current_release_id.as_deref() == Some(id));
    Ok(Json(Bootstrap {
        networks: networks(&state).await?,
        hosts: state.hosts().await?,
        scans: scanner::jobs(&state).await?,
        map: state
            .meta("map")
            .await?
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or_default(),
        capabilities: {
            let mut capabilities = state.0.capabilities.clone();
            capabilities.arp = capabilities.arp && state.0.scheduler.arp.available();
            const ARP_LIMITATION: &str = "Active ARP is unavailable: the server could not open a raw packet socket on any network interface, which needs the CAP_NET_RAW capability. Devices are still found by ping and TCP, but MAC addresses, manufacturers and most device types are missing. The server log says why for each interface (journalctl -u hope | grep ARP).";
            capabilities
                .limitations
                .retain(|limitation| limitation != ARP_LIMITATION);
            if state.0.capabilities.arp && !capabilities.arp {
                capabilities.limitations.push(ARP_LIMITATION.into());
            }
            capabilities
        },
        server_url: state.0.server_url.clone(),
        agent_release: crate::releases::current_version(&state.0.release_dir).await,
        report_interval_seconds: state.report_interval(),
        agent_auto_updates_enabled: state.auto_updates_enabled().await?,
        agent_auto_update_blocked_release_id,
        agent_upgrade_command: crate::agents::upgrade_command(&state),
    }))
}
/// Pause or resume automatic updates for supported Linux x86_64 agents.
pub async fn set_agent_updates(
    Extract(state): Extract<State>,
    Json(body): Json<hope_protocol::AgentUpdateSettings>,
) -> ApiResult<Json<hope_protocol::AgentUpdateSettings>> {
    let _guard = state.0.writes.lock().await;
    state
        .set_meta(
            "agent_auto_updates_enabled",
            if body.enabled { "true" } else { "false" },
        )
        .await?;
    if !body.enabled {
        for mut host in state.stored_hosts().await? {
            if host.update_request_source.as_deref() == Some("automatic") {
                host.update_requested_at = None;
                host.update_target = None;
                host.update_target_release_id = None;
                host.update_request_source = None;
                sqlx::query("UPDATE hosts SET body=? WHERE id=?")
                    .bind(serde_json::to_string(&host)?)
                    .bind(&host.id)
                    .execute(&state.0.db)
                    .await?;
                state.publish_host(&host.id).await;
            }
        }
        state.set_meta("auto_update_canary_agent", "").await?;
        state.set_meta("auto_update_canary_passed", "false").await?;
    }
    state.changed();
    Ok(Json(body))
}
/// Choose how often agents sample and report; they adopt it with their next accepted report.
pub async fn set_reporting(
    Extract(state): Extract<State>,
    Json(body): Json<hope_protocol::ReportingSettings>,
) -> ApiResult<Json<hope_protocol::ReportingSettings>> {
    if !hope_protocol::REPORT_INTERVALS_SECONDS.contains(&body.interval_seconds) {
        return Err(ApiError::bad(
            "Choose a report interval of 5, 10, 15, 30 or 60 seconds",
        ));
    }
    let _guard = state.0.writes.lock().await;
    state
        .set_meta(
            "report_interval_seconds",
            &body.interval_seconds.to_string(),
        )
        .await?;
    state
        .0
        .report_interval
        .store(body.interval_seconds, std::sync::atomic::Ordering::Relaxed);
    state.changed();
    Ok(Json(body))
}
pub use hope_protocol::NetworkInput;
/// A supported scheduled-scan interval, defaulting to five minutes.
fn interval(minutes: Option<u32>) -> ApiResult<u32> {
    let minutes = minutes.unwrap_or(hope_protocol::DEFAULT_SCAN_INTERVAL_MINUTES);
    if hope_protocol::SCAN_INTERVALS_MINUTES.contains(&minutes) {
        Ok(minutes)
    } else {
        Err(ApiError::bad(
            "Choose a scan interval of 5, 15 or 30 minutes, or 1, 6 or 24 hours",
        ))
    }
}
pub async fn add_network(
    Extract(state): Extract<State>,
    Json(body): Json<NetworkInput>,
) -> ApiResult<Json<Network>> {
    let _guard = state.0.writes.lock().await;
    if body.name.trim().is_empty() || body.name.len() > 120 {
        return Err(ApiError::bad("Network name must contain 1–120 characters"));
    }
    if networks(&state).await?.len() >= 32 {
        return Err(ApiError::bad("At most 32 ranges are supported"));
    }
    let cidr = scanner::parse_range(&body.cidr)?.to_string();
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks WHERE cidr=?")
        .bind(&cidr)
        .fetch_one(&state.0.db)
        .await?;
    if existing > 0 {
        return Err(ApiError::bad("This range already exists"));
    }
    let network = Network {
        id: Uuid::new_v4().to_string(),
        name: body.name.trim().into(),
        cidr,
        enabled: body.enabled.unwrap_or(true),
        interval_minutes: interval(body.interval_minutes)?,
    };
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES(?,?,?,?,?)")
        .bind(&network.id)
        .bind(&network.name)
        .bind(&network.cidr)
        .bind(network.enabled)
        .bind(network.interval_minutes)
        .execute(&state.0.db)
        .await?;
    for mut host in state.stored_hosts().await? {
        host.network_ids = matching_networks(&state, &host.addresses).await?;
        state.save_host(&host).await?;
    }
    let mut tx = state.write_transaction().await?;
    crate::identity::reconcile(&mut tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(network))
}
pub use hope_protocol::NetworkUpdate;
pub async fn update_network(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(body): Json<NetworkUpdate>,
) -> ApiResult<Json<OkResponse>> {
    let _guard = state.0.writes.lock().await;
    if !networks(&state).await?.iter().any(|n| n.id == id) {
        return Err(ApiError::not_found());
    }
    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE networks SET enabled=? WHERE id=?")
            .bind(enabled)
            .bind(&id)
            .execute(&state.0.db)
            .await?;
    }
    if body.interval_minutes.is_some() {
        sqlx::query("UPDATE networks SET interval_minutes=? WHERE id=?")
            .bind(interval(body.interval_minutes)?)
            .bind(&id)
            .execute(&state.0.db)
            .await?;
    }
    let mut tx = state.write_transaction().await?;
    crate::identity::reconcile(&mut tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(OkResponse { ok: true }))
}
/// Removes a range. Hosts discovered through it stay in the inventory and simply lose this
/// membership; the range's scan history goes with it, and an active scan is cancelled first.
pub async fn delete_network(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    if !networks(&state).await?.iter().any(|n| n.id == id) {
        return Err(ApiError::not_found());
    }
    let active: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM scans WHERE network_id=? AND status IN ('queued','running')",
    )
    .bind(&id)
    .fetch_all(&state.0.db)
    .await?;
    for scan in active {
        scanner::cancel(&state, &scan).await?;
    }
    let _guard = state.0.writes.lock().await;
    let mut tx = state.write_transaction().await?;
    sqlx::query("DELETE FROM networks WHERE id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scans WHERE network_id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    for mut host in state.stored_hosts().await? {
        if host.network_ids.iter().any(|n| n == &id) {
            host.network_ids.retain(|n| n != &id);
            sqlx::query("UPDATE hosts SET body=? WHERE id=?")
                .bind(serde_json::to_string(&host)?)
                .bind(&host.id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    state.changed();
    Ok(Json(OkResponse { ok: true }))
}
pub async fn start_scan(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<hope_protocol::ScanJob>> {
    let network = networks(&state)
        .await?
        .into_iter()
        .find(|n| n.id == id)
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(scanner::start(&state, &network).await?))
}
pub async fn cancel_scan(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    scanner::cancel(&state, &id).await?;
    Ok(Json(OkResponse { ok: true }))
}
pub async fn map(
    Extract(state): Extract<State>,
    Json(mut map): Json<MapState>,
) -> ApiResult<Json<MapState>> {
    let _guard = state.0.writes.lock().await;
    let current: MapState = state
        .meta("map")
        .await?
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    if map.revision != current.revision {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Map changed; reload before replacing the layout".into(),
        ));
    }
    map.revision = next_revision(current.revision)?;
    validate_map(&state, &map).await?;
    state.set_meta("map", &serde_json::to_string(&map)?).await?;
    state.changed();
    Ok(Json(map))
}
fn next_revision(revision: u64) -> ApiResult<u64> {
    if revision >= 9_007_199_254_740_991 {
        return Err(ApiError::bad("Map revision limit reached"));
    }
    revision
        .checked_add(1)
        .ok_or_else(|| ApiError::bad("Map revision limit reached"))
}
async fn validate_map(state: &State, map: &MapState) -> ApiResult<()> {
    if map.positions.len() > 2000
        || map.links.len() > 2000
        || map
            .positions
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > 1e6 || p.y.abs() > 1e6)
        || map
            .links
            .iter()
            .any(|l| l.label.len() > 120 || l.source == l.target)
    {
        return Err(ApiError::bad("Invalid map layout"));
    }
    let ids: Vec<_> = state.hosts().await?.into_iter().map(|h| h.id).collect();
    if map
        .links
        .iter()
        .any(|l| !ids.contains(&l.source) || !ids.contains(&l.target))
    {
        return Err(ApiError::bad("Manual links must connect existing hosts"));
    }
    if map
        .positions
        .iter()
        .any(|p| p.id.is_empty() || p.id.len() > 255)
        || map
            .links
            .iter()
            .any(|l| l.id.is_empty() || l.id.len() > 128)
        || map
            .positions
            .iter()
            .enumerate()
            .any(|(i, p)| map.positions[..i].iter().any(|other| other.id == p.id))
        || map
            .links
            .iter()
            .enumerate()
            .any(|(i, l)| map.links[..i].iter().any(|other| other.id == l.id))
    {
        return Err(ApiError::bad("Invalid or duplicate map identifiers"));
    }
    Ok(())
}
pub async fn edit_map(
    Extract(state): Extract<State>,
    Json(edit): Json<hope_protocol::MapEdit>,
) -> ApiResult<Json<MapState>> {
    let _guard = state.0.writes.lock().await;
    let mut map: MapState = state
        .meta("map")
        .await?
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    match edit {
        hope_protocol::MapEdit::SetPosition { position } => {
            map.positions.retain(|p| p.id != position.id);
            map.positions.push(position);
        }
        hope_protocol::MapEdit::AddLink { link } => {
            if let Some(existing) = map.links.iter().find(|l| l.id == link.id) {
                if existing.source != link.source
                    || existing.target != link.target
                    || existing.label != link.label
                {
                    return Err(ApiError::bad("This map link identifier already exists"));
                }
            } else {
                map.links.push(link);
            }
        }
        hope_protocol::MapEdit::RemoveLink { id } => {
            map.links.retain(|l| l.id != id);
        }
    }
    map.revision = next_revision(map.revision)?;
    validate_map(&state, &map).await?;
    state.set_meta("map", &serde_json::to_string(&map)?).await?;
    state.changed();
    Ok(Json(map))
}
pub use hope_protocol::MergeRequest as Merge;
pub async fn merge(
    Extract(state): Extract<State>,
    Path(target): Path<String>,
    Json(body): Json<Merge>,
) -> ApiResult<Json<Host>> {
    let _guard = state.0.writes.lock().await;
    if target == body.source_id {
        return Err(ApiError::bad("Select two different hosts"));
    }
    let mut target = state
        .stored_host(&target)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let source = state
        .stored_host(&body.source_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if target.id == source.id {
        return Err(ApiError::bad("Select two different hosts"));
    }
    let target_has_agent = target.agent_id.is_some() && !target.agent_uninstalled;
    let source_has_agent = source.agent_id.is_some() && !source.agent_uninstalled;
    if target_has_agent && source_has_agent {
        return Err(ApiError::bad("Cannot merge two enrolled agents"));
    }
    if !target_has_agent && source_has_agent {
        return Err(ApiError::bad(
            "Select the enrolled agent as the destination",
        ));
    }
    let source = source.into_view();
    for address in source.addresses {
        if !target.addresses.contains(&address) {
            target.addresses.push(address);
        }
    }
    for mac in source.macs {
        if !target.macs.contains(&mac) {
            target.macs.push(mac);
        }
    }
    for service in source.services {
        if !target.services.iter().any(|s| s.port == service.port) {
            target.services.push(service);
        }
    }
    target.network_ids = matching_networks(&state, &target.addresses).await?;
    target.primary_ip = crate::addresses::primary_ip(&target.addresses, &networks(&state).await?);
    let mut map: MapState = state
        .meta("map")
        .await?
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default();
    for link in &mut map.links {
        if link.source == source.id {
            link.source = target.id.clone();
        }
        if link.target == source.id {
            link.target = target.id.clone();
        }
    }
    map.links.retain(|l| l.source != l.target);
    map.positions.retain(|p| {
        p.id != source.id && p.id.rsplit_once(':').is_none_or(|(_, id)| id != source.id)
    });
    map.revision = next_revision(map.revision)?;
    let mut tx = state.write_transaction().await?;
    let linked_sources: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT source_device_id) FROM device_associations WHERE host_id IN (?,?) AND status='confirmed'")
        .bind(&source.id).bind(&target.id).fetch_one(&mut *tx).await?;
    if linked_sources > 1 {
        return Err(ApiError::bad(
            "Unlink the conflicting source devices before merging these hosts",
        ));
    }
    // Keep child inventory, links, and their source-specific history attached to the survivor.
    move_source_host_references(&mut tx, &source.id, &target.id).await?;
    sqlx::query("INSERT OR IGNORE INTO retired_devices(device_id,retired_at) SELECT ?,retired_at FROM retired_devices WHERE device_id=?")
        .bind(&target.id)
        .bind(&source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM retired_devices WHERE device_id=?")
        .bind(&source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE hosts SET body=? WHERE id=?")
        .bind(serde_json::to_string(&target)?)
        .bind(&target.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM hosts WHERE id=?")
        .bind(&source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO device_id_aliases(alias_id,survivor_id) VALUES(?,?) ON CONFLICT(alias_id) DO UPDATE SET survivor_id=excluded.survivor_id")
        .bind(&source.id).bind(&target.id).execute(&mut *tx).await?;
    sqlx::query("UPDATE device_id_aliases SET survivor_id=? WHERE survivor_id=?")
        .bind(&target.id)
        .bind(&source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO meta(key,value) VALUES('map',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(serde_json::to_string(&map)?).execute(&mut *tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(
        state
            .host(&target.id)
            .await?
            .ok_or_else(ApiError::not_found)?,
    ))
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateCandidate {
    pub id: String,
    pub name: String,
    pub primary_ip: Option<String>,
    pub device_type: Option<String>,
    pub platform: Option<String>,
    pub source_count: i64,
    pub last_seen: f64,
    pub sources: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct DuplicateEvidence {
    pub label: String,
    pub value: String,
}
#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub suggested_survivor_id: String,
    pub review_only: bool,
    pub candidates: Vec<DuplicateCandidate>,
    pub evidence: Vec<DuplicateEvidence>,
}
#[derive(Debug, Serialize)]
pub struct DuplicateGroups {
    pub groups: Vec<DuplicateGroup>,
}
#[derive(Debug, Deserialize)]
pub struct ReconcileDuplicates {
    pub merges: Vec<ReconcileMerge>,
}
#[derive(Debug, Deserialize)]
pub struct ReconcileMerge {
    pub survivor_id: String,
    pub duplicate_ids: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct ReconcileResult {
    pub survivor_id: String,
    pub merged_ids: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct ReconcileResponse {
    pub merged: Vec<ReconcileResult>,
}

fn reported_macs(child: &hope_protocol::ChildDeviceReport) -> HashSet<String> {
    child
        .interfaces
        .iter()
        .filter_map(|interface| interface.mac.as_deref())
        .chain(
            child
                .network_configuration
                .iter()
                .filter_map(|network| network.mac.as_deref()),
        )
        .map(str::to_ascii_lowercase)
        .collect()
}

pub async fn duplicate_groups(Extract(state): Extract<State>) -> ApiResult<Json<DuplicateGroups>> {
    let rows = sqlx::query("SELECT d.id,d.source,d.source_scope,d.source_key,d.incarnation,d.name,d.observed_at,d.body,(SELECT COUNT(*) FROM source_observations o WHERE o.source_device_id=d.id) AS source_count FROM source_devices d WHERE d.lifecycle!='archived' AND NOT EXISTS(SELECT 1 FROM source_device_aliases a WHERE a.alias_id=d.id) AND (NOT EXISTS(SELECT 1 FROM shared_source_scopes s WHERE s.source=d.source AND s.source_scope=d.source_scope) OR EXISTS(SELECT 1 FROM shared_source_members m WHERE m.source=d.source AND m.source_scope=d.source_scope AND m.agent_id=d.parent_agent_id)) ORDER BY d.ownership_generation DESC,d.observed_at DESC LIMIT 1000")
        .fetch_all(&state.0.db).await?;
    let mut members = Vec::new();
    for row in rows {
        let body: String = row.get("body");
        let child: hope_protocol::ChildDeviceReport = serde_json::from_str(&body)?;
        members.push((
            DuplicateCandidate {
                id: row.get("id"),
                name: row.get("name"),
                primary_ip: child
                    .interfaces
                    .iter()
                    .flat_map(|i| &i.addresses)
                    .next()
                    .cloned(),
                device_type: Some(format!("{:?}", child.identity.kind).to_ascii_lowercase()),
                platform: child.guest_os.as_ref().and_then(|os| os.name.clone()),
                source_count: row.get("source_count"),
                last_seen: row.get("observed_at"),
                sources: vec![row.get("source")],
            },
            row.get::<String, _>("source"),
            row.get::<String, _>("source_scope"),
            row.get::<String, _>("source_key"),
            row.get::<String, _>("incarnation"),
            child,
        ));
    }
    let shared_scopes: HashSet<(String, String)> =
        sqlx::query("SELECT source,source_scope FROM shared_source_scopes")
            .fetch_all(&state.0.db)
            .await?
            .into_iter()
            .map(|r| (r.get("source"), r.get("source_scope")))
            .collect();
    let mut groups = Vec::new();
    let mut used = HashSet::new();
    // Exact identity duplicates under a token-bound scope are proven safe to reconcile.
    let mut exact = HashMap::<(String, String, String, String), Vec<usize>>::new();
    for (index, (_, source, scope, key, incarnation, _)) in members.iter().enumerate() {
        if shared_scopes.contains(&(source.clone(), scope.clone())) {
            exact
                .entry((
                    source.clone(),
                    scope.clone(),
                    key.clone(),
                    incarnation.clone(),
                ))
                .or_default()
                .push(index);
        }
    }
    for ((source, scope, key, incarnation), indices) in exact {
        if indices.len() < 2 {
            continue;
        }
        let candidates: Vec<_> = indices.iter().map(|&i| members[i].0.clone()).collect();
        let survivor = candidates[0].id.clone();
        used.extend(candidates.iter().map(|c| c.id.clone()));
        groups.push(DuplicateGroup {
            suggested_survivor_id: survivor,
            review_only: false,
            candidates,
            evidence: vec![DuplicateEvidence {
                label: "Authorized source identity".into(),
                value: format!("{source} / {scope} / {key} / {incarnation}"),
            }],
        });
    }
    // Legacy per-node scopes have no cluster proof. Surface only same-kind/key records with a
    // matching guest-interface or configured NIC MAC, and require an explicit human decision.
    let mut by_mac = HashMap::<(String, String, String, String), Vec<usize>>::new();
    for (index, (_, source, scope, key, _, child)) in members.iter().enumerate() {
        if used.contains(&members[index].0.id)
            || shared_scopes.contains(&(source.clone(), scope.clone()))
        {
            continue;
        }
        let kind = format!("{:?}", child.identity.kind);
        for mac in reported_macs(child) {
            by_mac
                .entry((source.clone(), kind.clone(), key.clone(), mac))
                .or_default()
                .push(index);
        }
    }
    for ((source, kind, key, mac), indices) in by_mac {
        let scopes: HashSet<_> = indices.iter().map(|&i| members[i].2.clone()).collect();
        let times: Vec<f64> = indices.iter().map(|&i| members[i].0.last_seen).collect();
        let recent_together = times.iter().copied().fold(f64::INFINITY, f64::min) + 7.0 * 86400.0
            >= times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if indices.len() < 2 || scopes.len() < 2 || !recent_together {
            continue;
        }
        let candidates: Vec<_> = indices.iter().map(|&i| members[i].0.clone()).collect();
        let survivor = candidates[0].id.clone();
        groups.push(DuplicateGroup {
            suggested_survivor_id: survivor,
            review_only: true,
            candidates,
            evidence: vec![DuplicateEvidence {
                label: "Shared interface MAC; legacy scope needs review".into(),
                value: format!("{source} / {kind} / {key} / {mac}"),
            }],
        });
    }
    groups.truncate(100);
    Ok(Json(DuplicateGroups { groups }))
}

pub async fn reconcile_duplicate_groups(
    Extract(state): Extract<State>,
    Json(request): Json<ReconcileDuplicates>,
) -> ApiResult<Json<ReconcileResponse>> {
    let _guard = state.0.writes.lock().await;
    let mut tx = state.write_transaction().await?;
    let mut merged = Vec::new();
    for group in request.merges {
        if group.duplicate_ids.len() > 100 || group.duplicate_ids.contains(&group.survivor_id) {
            return Err(ApiError::bad("Invalid duplicate merge group"));
        }
        let survivor: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_devices WHERE id=?)")
                .bind(&group.survivor_id)
                .fetch_one(&mut *tx)
                .await?;
        if !survivor {
            return Err(ApiError::not_found());
        }
        let mut merged_ids = Vec::new();
        for duplicate in group.duplicate_ids {
            if sqlx::query("SELECT 1 FROM source_devices WHERE id=?")
                .bind(&duplicate)
                .fetch_optional(&mut *tx)
                .await?
                .is_none()
            {
                return Err(ApiError::not_found());
            }
            crate::identity::merge_source_device(&mut tx, &group.survivor_id, &duplicate).await?;
            merged_ids.push(duplicate);
        }
        merged.push(ReconcileResult {
            survivor_id: group.survivor_id,
            merged_ids,
        });
    }
    crate::identity::reconcile(&mut tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(ReconcileResponse { merged }))
}

async fn move_source_host_references(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    source: &str,
    target: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE source_devices SET host_id=? WHERE host_id=?")
        .bind(target)
        .bind(source)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE OR IGNORE device_associations SET host_id=? WHERE host_id=?")
        .bind(target)
        .bind(source)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE OR IGNORE device_relationships SET from_device_id=? WHERE from_device_id=?",
    )
    .bind(target)
    .bind(source)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE OR IGNORE device_relationships SET to_device_id=? WHERE to_device_id=?")
        .bind(target)
        .bind(source)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM device_associations WHERE host_id=?")
        .bind(source)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM device_relationships WHERE from_device_id=? OR to_device_id=?")
        .bind(source)
        .bind(source)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
/// Server-sent change notifications: `host` carries one host's new state, `refresh` asks the
/// client to reload everything. A receiver that falls behind the broadcast buffer gets a
/// `resync` event (the client reloads everything) instead of silently missing changes. The stream
/// ends when the server shuts down, after ten minutes, or when the client disconnects; dropping
/// the response drops the broadcast receiver.
pub async fn events(
    Extract(state): Extract<State>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.0.events.subscribe())
        .map(|message| {
            Ok(match message {
                Ok(crate::state::Change::Refresh) => Event::default().event("refresh").data("{}"),
                Ok(crate::state::Change::Host(body)) => Event::default().event("host").data(&*body),
                Err(_) => Event::default().event("resync").data("{}"),
            })
        })
        .take_until(tokio::time::sleep(std::time::Duration::from_secs(600)))
        .take_until(state.0.shutdown.clone().cancelled_owned());
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Set a device's type by hand, or (with `kind: null`) return it to automatic detection.
pub async fn set_device_type(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(body): Json<hope_protocol::DeviceTypeUpdate>,
) -> ApiResult<Json<Host>> {
    let _guard = state.0.writes.lock().await;
    let mut host = state.host(&id).await?.ok_or_else(ApiError::not_found)?;
    match body.kind.as_deref() {
        Some(kind) => {
            host.device = Some(
                crate::classify::manual(kind)
                    .ok_or_else(|| ApiError::bad("Unknown device type"))?,
            );
        }
        None => {
            host.device = None;
            crate::classify::refresh(&mut host, &state.gateway_set());
        }
    }
    state.save_settings(&host).await?;
    state.changed();
    Ok(Json(host))
}

/// Set a host's display name and icon, or clear either override with null.
pub async fn set_host_identity(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(mut body): Json<hope_protocol::HostIdentityUpdate>,
) -> ApiResult<Json<Host>> {
    if let Some(name) = &mut body.name {
        *name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(ApiError::bad("Name must be between 1 and 80 characters"));
        }
    }
    if let Some(icon) = &body.icon {
        let valid = icon.strip_prefix("selfhst:").is_some_and(|slug| {
            !slug.is_empty()
                && slug.len() <= 80
                && slug
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        }) || icon
            .strip_prefix("unifi:")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()));
        if !valid {
            return Err(ApiError::bad(
                "Icon must be a selfh.st slug or UniFi fingerprint ID",
            ));
        }
    }

    let _guard = state.0.writes.lock().await;
    let mut host = state.host(&id).await?.ok_or_else(ApiError::not_found)?;
    host.name_override = body.name;
    host.icon_override = body.icon;
    state.save_settings(&host).await?;
    state.changed();
    Ok(Json(host))
}
