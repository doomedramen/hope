//! Authenticated child inventory ingestion and source identity migration.
use crate::{
    auth::limited,
    error::{ApiError, ApiResult},
    limits::ClientKey,
    state::{State, hash, now},
};
use axum::{
    Json,
    extract::{Extension, State as Extract},
    http::HeaderMap,
};
use hope_protocol::{ChildReportBatch, ChildReportResponse, ChildSourceKind, Host};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

fn same_interface_mac(
    left: &hope_protocol::ChildDeviceReport,
    right: &hope_protocol::ChildDeviceReport,
) -> bool {
    let macs = |report: &hope_protocol::ChildDeviceReport| {
        report
            .interfaces
            .iter()
            .filter_map(|interface| interface.mac.as_deref())
            .chain(
                report
                    .network_configuration
                    .iter()
                    .filter_map(|network| network.mac.as_deref()),
            )
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let left = macs(left);
    macs(right).iter().any(|mac| left.contains(mac))
}

/// Stores a bounded, sequenced child inventory independently of the parent heartbeat. Only a
/// complete authoritative generation advances absence tracking; partial inventories cannot age
/// children toward archival.
pub async fn child_report(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    headers: HeaderMap,
    Json(batch): Json<ChildReportBatch>,
) -> ApiResult<Json<ChildReportResponse>> {
    limited(state.0.report_denied.check(&client.0, now()))?;
    let Some(credential) = headers
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    };
    let known: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&batch.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    if known
        .as_ref()
        .is_none_or(|(expected, _)| expected != &hash(credential))
    {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    }
    if known.is_some_and(|(_, revoked)| revoked) {
        return Err(ApiError::agent_revoked());
    }
    limited(state.0.report_limit.check(&batch.agent_id, now()))?;
    state.0.report_limit.hit(&batch.agent_id, now());
    hope_protocol::validation::child_batch(&batch).map_err(ApiError::bad)?;
    if batch.legacy_identities.len() > 500
        || batch.legacy_identities.iter().any(|item| {
            item.key.is_empty()
                || item.incarnation.is_empty()
                || item.canonical_incarnation.is_empty()
        })
    {
        return Err(ApiError::bad("Invalid legacy child identity map"));
    }
    if batch.observed_at > now() + 300.0 || batch.observed_at < now() - 86400.0 {
        return Err(ApiError::bad("Invalid child inventory timestamp"));
    }

    // Authenticate again under the shared write lock so revocation and credential replacement
    // are ordered against source writes exactly like core reports.
    let _guard = state.0.writes.lock().await;
    let current: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&batch.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    match current {
        Some((expected, false)) if expected == hash(credential) => {}
        Some((expected, true)) if expected == hash(credential) => {
            return Err(ApiError::agent_revoked());
        }
        _ => return Err(ApiError::unauthorized()),
    }

    let source = match batch.source {
        ChildSourceKind::Docker => "docker",
        ChildSourceKind::Proxmox => "proxmox",
    };
    let mut tx = state.write_transaction().await?;
    let shared_scope = if let Some(token) = batch.source_token.as_deref() {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ApiError::bad("Invalid shared source token"));
        }
        let token_hash = hash(token);
        let bound: Option<String> = sqlx::query_scalar(
            "SELECT token_hash FROM shared_source_scopes WHERE source=? AND source_scope=?",
        )
        .bind(source)
        .bind(&batch.source_scope)
        .fetch_optional(&mut *tx)
        .await?;
        match bound {
            Some(expected) if expected != token_hash => {
                return Err(ApiError::bad(
                    "Shared source token does not match this cluster",
                ));
            }
            Some(_) => true,
            None => {
                sqlx::query("INSERT INTO shared_source_scopes(source,source_scope,token_hash,created_at) VALUES(?,?,?,?)")
                    .bind(source).bind(&batch.source_scope).bind(token_hash).bind(now()).execute(&mut *tx).await?;
                true
            }
        }
    } else {
        false
    };
    if shared_scope {
        sqlx::query("INSERT OR IGNORE INTO shared_source_members(source,source_scope,agent_id,joined_at) VALUES(?,?,?,?)")
            .bind(source).bind(&batch.source_scope).bind(&batch.agent_id).bind(now()).execute(&mut *tx).await?;
    }
    let replay: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT parent_agent_id,source_scope,sequence FROM child_batches WHERE batch_id=?",
    )
    .bind(&batch.batch_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((parent, scope, sequence)) = replay {
        if parent == batch.agent_id
            && scope == batch.source_scope
            && sequence == batch.sequence as i64
        {
            tx.rollback().await?;
            return Ok(Json(ChildReportResponse {
                accepted: true,
                duplicate: true,
            }));
        }
        return Err(ApiError::bad("Child inventory batch identity was reused"));
    }
    let head: Option<(i64, i64)> = sqlx::query_as(
        "SELECT sequence,inventory_generation FROM child_batch_heads WHERE parent_agent_id=? AND source=? AND source_scope=?",
    )
    .bind(&batch.agent_id)
    .bind(source)
    .bind(&batch.source_scope)
    .fetch_optional(&mut *tx)
    .await?;
    if head.is_some_and(|(sequence, generation)| {
        batch.sequence <= sequence as u64 || batch.inventory_generation < generation as u64
    }) {
        return Err(ApiError::bad(
            "Child inventory sequence or generation is out of order",
        ));
    }
    let parent_host: String = sqlx::query_scalar("SELECT id FROM hosts WHERE agent_id=?")
        .bind(&batch.agent_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if shared_scope && let Some(old_scope) = batch.legacy_source_scope.as_deref() {
        for legacy in &batch.legacy_identities {
            let kind = match legacy.kind {
                hope_protocol::ChildDeviceKind::DockerContainer => "docker_container",
                hope_protocol::ChildDeviceKind::ProxmoxVm => "proxmox_vm",
                hope_protocol::ChildDeviceKind::ProxmoxLxc => "proxmox_lxc",
            };
            let old_row: Option<(String, String, f64, String)> = sqlx::query_as("SELECT id,body,observed_at,lifecycle FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=? AND kind=?")
                    .bind(&batch.agent_id).bind(source).bind(old_scope).bind(&legacy.key).bind(&legacy.incarnation).bind(kind)
                    .fetch_optional(&mut *tx).await?;
            let Some((old_id, old_body, old_seen, old_lifecycle)) = old_row else {
                continue;
            };
            // Keep metadata-only migration evidence pending until a current observation of
            // the canonical identity proves that the interface still matches. Old local
            // maps can survive VMID reuse, so a tuple alone must never move history.
            sqlx::query("INSERT OR IGNORE INTO pending_legacy_identities(source,parent_agent_id,legacy_source_device_id,source_scope,source_key,canonical_incarnation) VALUES(?,?,?,?,?,?)")
                    .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                    .execute(&mut *tx).await?;

            // The canonical owner may have reported before this remote reporter submitted its
            // legacy map. In that ordering no later guest batch from this reporter may arrive
            // (for example, its collector can be stopped), so reconcile against the already
            // stored shared identity now. The token-bound scope/key/incarnation selects the
            // canonical record; matching kind, fresh timestamps, and an interface MAC prove the
            // legacy row is the same current guest rather than a reused VMID.
            crate::identity::reconcile_shared_identity(
                &mut tx,
                source,
                &batch.source_scope,
                &legacy.key,
                &legacy.canonical_incarnation,
            )
            .await?;
            let canonical_id: Option<String> = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE source=? AND identity_owner='@shared' AND source_scope=? AND source_key=? AND incarnation=?")
                .bind(source).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                .fetch_optional(&mut *tx).await?;
            if old_lifecycle != "archived"
                && let Some(canonical_id) = canonical_id
                && canonical_id != old_id
            {
                let canonical: Option<(String, String, f64, String)> = sqlx::query_as(
                    "SELECT kind,body,observed_at,lifecycle FROM source_devices WHERE id=?",
                )
                .bind(&canonical_id)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((canonical_kind, canonical_body, canonical_seen, canonical_lifecycle)) =
                    canonical
                    && canonical_kind == kind
                    && canonical_lifecycle != "archived"
                    && batch.observed_at >= old_seen
                    && batch.observed_at - old_seen <= 30.0 * 86400.0
                    && batch.observed_at >= canonical_seen
                    && batch.observed_at - canonical_seen <= 30.0 * 86400.0
                    && (canonical_seen - old_seen).abs() <= 30.0 * 86400.0
                {
                    let old: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                    let canonical: hope_protocol::ChildDeviceReport =
                        serde_json::from_str(&canonical_body)?;
                    if same_interface_mac(&old, &canonical) {
                        crate::identity::merge_legacy_source_identity(
                            &mut tx,
                            &canonical_id,
                            &old_id,
                            &batch.agent_id,
                        )
                        .await?;
                        sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                            .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                            .execute(&mut *tx).await?;
                    }
                }
            } else if old_lifecycle != "archived"
                && batch.observed_at >= old_seen
                && batch.observed_at - old_seen <= 30.0 * 86400.0
            {
                // Before any current owner report exists, two authorized cluster members may
                // submit maps for copies of the same old cluster-wide VM inventory. Join those
                // copies only when both point to the exact same token-bound canonical identity,
                // have current rows, and share an interface MAC. The pending-map membership is
                // the cluster proof; VMID/name/IP coincidence alone never reaches this path.
                let old_report: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                let siblings = sqlx::query("SELECT p.parent_agent_id,p.legacy_source_device_id,d.body,d.observed_at,d.lifecycle,d.kind FROM pending_legacy_identities p JOIN source_devices d ON d.id=p.legacy_source_device_id AND d.parent_agent_id=p.parent_agent_id AND d.source=p.source AND d.source_key=p.source_key JOIN shared_source_members m ON m.source=p.source AND m.source_scope=p.source_scope AND m.agent_id=p.parent_agent_id JOIN agents a ON a.id=p.parent_agent_id AND a.revoked=0 WHERE p.source=? AND p.source_scope=? AND p.source_key=? AND p.canonical_incarnation=? AND p.legacy_source_device_id!=? AND p.parent_agent_id!=? ORDER BY d.observed_at DESC,d.id")
                    .bind(source).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                    .bind(&old_id).bind(&batch.agent_id).fetch_all(&mut *tx).await?;
                for sibling in siblings {
                    let sibling_parent: String = sibling.get("parent_agent_id");
                    let sibling_id: String = sibling.get("legacy_source_device_id");
                    let sibling_body: String = sibling.get("body");
                    let sibling_seen: f64 = sibling.get("observed_at");
                    let sibling_lifecycle: String = sibling.get("lifecycle");
                    let sibling_kind: String = sibling.get("kind");
                    if sibling_kind != kind
                        || sibling_lifecycle == "archived"
                        || batch.observed_at < sibling_seen
                        || batch.observed_at - sibling_seen > 30.0 * 86400.0
                        || (sibling_seen - old_seen).abs() > 30.0 * 86400.0
                    {
                        continue;
                    }
                    let sibling_report: hope_protocol::ChildDeviceReport =
                        serde_json::from_str(&sibling_body)?;
                    if !same_interface_mac(&old_report, &sibling_report) {
                        continue;
                    }
                    // Pick a stable survivor independent of which agent's metadata arrives
                    // first; old URLs for the duplicate remain resolvable through the alias.
                    let (survivor, duplicate, duplicate_parent) = if old_id <= sibling_id {
                        (&old_id, &sibling_id, &sibling_parent)
                    } else {
                        (&sibling_id, &old_id, &batch.agent_id)
                    };
                    crate::identity::merge_legacy_source_identity(
                        &mut tx,
                        survivor,
                        duplicate,
                        duplicate_parent,
                    )
                    .await?;
                    sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                        .bind(source).bind(duplicate_parent).bind(duplicate).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                        .execute(&mut *tx).await?;
                }
            }
        }
    }
    sqlx::query("INSERT INTO child_batches(batch_id,parent_agent_id,source,source_scope,sequence,inventory_generation,inventory_id,chunk_index,chunk_count,observed_at,complete,received_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&batch.batch_id).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
        .bind(batch.sequence as i64).bind(batch.inventory_generation as i64)
        .bind(&batch.inventory_id).bind(batch.chunk_index as i64).bind(batch.chunk_count as i64)
        .bind(batch.observed_at).bind(batch.complete).bind(now()).execute(&mut *tx).await?;

    let authoritative_complete = if batch.complete {
        if let Some(inventory_id) = &batch.inventory_id {
            let (count, min_index, max_index, min_count, max_count): (i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
                "SELECT COUNT(*),MIN(chunk_index),MAX(chunk_index),MIN(chunk_count),MAX(chunk_count) FROM child_batches WHERE parent_agent_id=? AND source=? AND source_scope=? AND inventory_id=? AND inventory_generation=?",
            ).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
                .bind(inventory_id).bind(batch.inventory_generation as i64)
                .fetch_one(&mut *tx).await?;
            count == batch.chunk_count as i64
                && min_index == Some(0)
                && max_index == Some(batch.chunk_count as i64 - 1)
                && min_count == Some(batch.chunk_count as i64)
                && max_count == Some(batch.chunk_count as i64)
        } else {
            batch.chunk_count == 1 && batch.chunk_index == 0
        }
    } else {
        false
    };

    let mut seen: HashSet<(String, String)> = batch
        .devices
        .iter()
        .map(|device| {
            (
                device.identity.key.clone(),
                device.identity.incarnation.clone(),
            )
        })
        .collect();
    let mut source_ids = std::collections::HashMap::new();
    let mut fresh_source_ids = HashSet::new();
    for device in &batch.devices {
        let identity = &device.identity;
        if shared_scope {
            crate::identity::reconcile_shared_identity(
                &mut tx,
                source,
                &batch.source_scope,
                &identity.key,
                &identity.incarnation,
            )
            .await?;
        }
        // A scope is only authoritative inside its authenticated reporter until cluster
        // membership has been authorized. This still reconciles node moves reported by one
        // controller while preventing another enrolled agent from claiming the same VMID.
        let identity_owner = if shared_scope {
            "@shared"
        } else {
            batch.agent_id.as_str()
        };
        let mapped_id: Option<String> = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE source=? AND identity_owner=? AND source_scope=? AND source_key=? AND incarnation=?")
            .bind(source).bind(identity_owner).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).fetch_optional(&mut *tx).await?;
        let kind = match identity.kind {
            hope_protocol::ChildDeviceKind::DockerContainer => "docker_container",
            hope_protocol::ChildDeviceKind::ProxmoxVm => "proxmox_vm",
            hope_protocol::ChildDeviceKind::ProxmoxLxc => "proxmox_lxc",
        };
        let mut pending_legacy_id = None;
        if shared_scope {
            let pending = sqlx::query("SELECT legacy_source_device_id,parent_agent_id FROM pending_legacy_identities WHERE source=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                .bind(source).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation)
                .fetch_all(&mut *tx).await?;
            for row in pending {
                let old_id: String = row.get("legacy_source_device_id");
                let old_parent: String = row.get("parent_agent_id");
                let old: Option<(String, f64)> = sqlx::query_as("SELECT body,observed_at FROM source_devices WHERE id=? AND parent_agent_id=? AND source=? AND kind=?")
                    .bind(&old_id).bind(&old_parent).bind(source).bind(kind).fetch_optional(&mut *tx).await?;
                let Some((old_body, old_seen)) = old else {
                    sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                        .bind(source).bind(&old_parent).bind(&old_id).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).execute(&mut *tx).await?;
                    continue;
                };
                let old: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                if batch.observed_at < old_seen
                    || batch.observed_at - old_seen > 30.0 * 86400.0
                    || !same_interface_mac(device, &old)
                {
                    continue;
                }
                if let Some(canonical) = mapped_id.as_deref() {
                    if canonical != old_id {
                        crate::identity::merge_legacy_source_identity(
                            &mut tx,
                            canonical,
                            &old_id,
                            &old_parent,
                        )
                        .await?;
                    }
                } else if let Some(survivor) = pending_legacy_id.as_deref() {
                    if survivor != old_id {
                        crate::identity::merge_legacy_source_identity(
                            &mut tx,
                            survivor,
                            &old_id,
                            &old_parent,
                        )
                        .await?;
                    }
                } else {
                    pending_legacy_id.get_or_insert(old_id.clone());
                }
                sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                    .bind(source).bind(&old_parent).bind(&old_id).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).execute(&mut *tx).await?;
            }
        }
        let legacy_row: Option<(String, String, f64)> = if shared_scope {
            match (batch.legacy_source_scope.as_deref(),device.legacy_incarnation.as_deref()) {
                (Some(old_scope),Some(old_incarnation)) => sqlx::query_as("SELECT id,body,observed_at FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=? AND kind=?")
                    .bind(&batch.agent_id).bind(source).bind(old_scope).bind(&identity.key).bind(old_incarnation).bind(kind)
                    .fetch_optional(&mut *tx).await?,
                _ => None,
            }
        } else {
            None
        };
        let legacy_id = if pending_legacy_id.is_some() {
            pending_legacy_id
        } else if let Some((old_id, old_body, old_seen)) = &legacy_row {
            let old: hope_protocol::ChildDeviceReport = serde_json::from_str(old_body)?;
            if batch.observed_at - *old_seen <= 30.0 * 86400.0 && same_interface_mac(device, &old) {
                Some(old_id.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(canonical), Some(old)) = (mapped_id.as_deref(), legacy_id.as_deref())
            && canonical != old
        {
            crate::identity::merge_legacy_source_identity(&mut tx, canonical, old, &batch.agent_id)
                .await?;
        }
        let existing: Option<(String, f64, i64, String)> = if let Some(mapped_id) = mapped_id {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE id=?")
                .bind(mapped_id).fetch_optional(&mut *tx).await?
        } else if let Some(old_id) = legacy_id {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE id=?")
                .bind(old_id).fetch_optional(&mut *tx).await?
        } else {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=?")
                .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).fetch_optional(&mut *tx).await?
        };
        let id = existing
            .as_ref()
            .map(|(id, _, _, _)| id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let owner_generation = device.ownership_generation.unwrap_or(0);
        let is_newer =
            existing
                .as_ref()
                .is_none_or(|(_, observed_at, prior_generation, prior_parent)| {
                    owner_generation > *prior_generation as u64
                        || (owner_generation == *prior_generation as u64
                            && prior_parent == &batch.agent_id
                            && batch.observed_at >= *observed_at)
                });
        if is_newer {
            fresh_source_ids.insert(id.clone());
        }
        let body = serde_json::to_string(device)?;
        if is_newer && sqlx::query("UPDATE source_devices SET parent_agent_id=?,kind=?,lifecycle=?,name=?,observed_at=?,inventory_generation=?,ownership_generation=?,missing_complete_count=0,missing_since=NULL,missing_inventory_generation=0,body=? WHERE id=?")
            .bind(&batch.agent_id)
            .bind(kind)
            .bind(format!("{:?}", device.lifecycle).to_ascii_lowercase())
            .bind(&device.name).bind(batch.observed_at).bind(batch.inventory_generation as i64).bind(owner_generation as i64)
            .bind(&body).bind(&id).execute(&mut *tx).await?.rows_affected() == 0
        {
            sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,ownership_generation,body) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(&id).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
                .bind(&identity.key).bind(&identity.incarnation)
                .bind(kind)
                .bind(format!("{:?}", device.lifecycle).to_ascii_lowercase())
                .bind(&device.name).bind(batch.observed_at).bind(batch.inventory_generation as i64).bind(owner_generation as i64)
                .bind(&body).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO source_device_identities(source,identity_owner,source_scope,source_key,incarnation,source_device_id) VALUES(?,?,?,?,?,?) ON CONFLICT(source,identity_owner,source_scope,source_key,incarnation) DO UPDATE SET source_device_id=excluded.source_device_id")
            .bind(source).bind(identity_owner).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).bind(&id).execute(&mut *tx).await?;
        sqlx::query("INSERT OR IGNORE INTO source_observations(source_device_id,batch_id,observed_at,inventory_generation,body) VALUES(?,?,?,?,?)")
            .bind(&id).bind(&batch.batch_id).bind(batch.observed_at).bind(batch.inventory_generation as i64)
            .bind(&body).execute(&mut *tx).await?;
        sqlx::query("INSERT OR IGNORE INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,30,?)")
            .bind(&id).bind(&batch.batch_id).bind(batch.observed_at)
            .bind(serde_json::to_string(&device.metrics)?).execute(&mut *tx).await?;
        source_ids.insert(identity.key.as_str(), id);
    }

    for device in &batch.devices {
        let id = &source_ids[device.identity.key.as_str()];
        if !fresh_source_ids.contains(id) {
            continue;
        }
        let parent = device
            .parent_key
            .as_deref()
            .and_then(|key| source_ids.get(key))
            .map(String::as_str)
            .unwrap_or(&parent_host);
        sqlx::query("DELETE FROM device_relationships WHERE source_device_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT OR REPLACE INTO device_relationships(from_device_id,to_device_id,kind,detail,source_device_id,observed_at) VALUES(?,?,'runs_on',NULL,?,?)")
            .bind(id).bind(parent).bind(id).bind(batch.observed_at).execute(&mut *tx).await?;
        for port in &device.published_ports {
            let Some(host_port) = port.host_port else {
                continue;
            };
            let address = port
                .host_ip
                .as_deref()
                .filter(|ip| !ip.is_empty())
                .unwrap_or("*");
            let detail = format!(
                "{}://{}:{} → container:{}",
                port.protocol, address, host_port, port.container_port
            );
            sqlx::query("INSERT OR REPLACE INTO device_relationships(from_device_id,to_device_id,kind,detail,source_device_id,observed_at) VALUES(?,?,'exposes_service',?,?,?)")
                .bind(id).bind(parent).bind(detail).bind(id).bind(batch.observed_at)
                .execute(&mut *tx).await?;
        }
    }

    crate::identity::reconcile_sources(&mut tx, &fresh_source_ids).await?;
    for source_id in &fresh_source_ids {
        State::clear_retired_source(&mut tx, source_id).await?;
    }

    if authoritative_complete {
        if let Some(inventory_id) = &batch.inventory_id {
            let rows = sqlx::query(
                "SELECT DISTINCT d.source_key,d.incarnation FROM source_observations o JOIN child_batches b ON b.batch_id=o.batch_id JOIN source_devices d ON d.id=o.source_device_id WHERE b.parent_agent_id=? AND b.source=? AND b.source_scope=? AND b.inventory_id=? AND b.inventory_generation=?",
            )
            .bind(&batch.agent_id)
            .bind(source)
            .bind(&batch.source_scope)
            .bind(inventory_id)
            .bind(batch.inventory_generation as i64)
            .fetch_all(&mut *tx)
            .await?;
            seen = rows
                .into_iter()
                .map(|row| {
                    (
                        sqlx::Row::get(&row, "source_key"),
                        sqlx::Row::get(&row, "incarnation"),
                    )
                })
                .collect();
        }
        let scan_presence = sqlx::query("SELECT id,body FROM hosts")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .filter_map(|row| {
                let id: String = row.get("id");
                let body: String = row.get("body");
                serde_json::from_str::<Host>(&body)
                    .ok()
                    .map(|host| (id, host.last_seen))
            })
            .collect::<HashMap<_, _>>();
        let rows = sqlx::query("SELECT id,source_key,incarnation,host_id,missing_complete_count,missing_since,missing_inventory_generation FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND lifecycle!='archived'")
            .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).fetch_all(&mut *tx).await?;
        for row in rows {
            let key: String = sqlx::Row::get(&row, "source_key");
            let incarnation: String = sqlx::Row::get(&row, "incarnation");
            if seen.contains(&(key, incarnation)) {
                continue;
            }
            let id: String = sqlx::Row::get(&row, "id");
            let previous: i64 = sqlx::Row::get(&row, "missing_complete_count");
            let associated_host: Option<String> = sqlx::Row::get(&row, "host_id");
            let since: Option<f64> = sqlx::Row::get(&row, "missing_since");
            let last_generation: i64 = sqlx::Row::get(&row, "missing_inventory_generation");
            if batch.inventory_generation <= last_generation as u64 {
                continue;
            }
            let missing_since = since.unwrap_or(batch.observed_at);
            let count = previous.saturating_add(1);
            let present_elsewhere = associated_host
                .as_ref()
                .and_then(|host_id| scan_presence.get(host_id))
                .is_some_and(|last_seen| batch.observed_at - last_seen < 600.0);
            let lifecycle =
                if count >= 3 && batch.observed_at - missing_since >= 300.0 && !present_elsewhere {
                    "archived"
                } else {
                    "stale"
                };
            sqlx::query("UPDATE source_devices SET lifecycle=?,missing_complete_count=?,missing_since=?,missing_inventory_generation=? WHERE id=?")
                .bind(lifecycle).bind(count).bind(missing_since).bind(batch.inventory_generation as i64).bind(id).execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO child_batch_heads(parent_agent_id,source,source_scope,sequence,inventory_generation,observed_at,complete_generation,complete_observed_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(parent_agent_id,source,source_scope) DO UPDATE SET sequence=excluded.sequence,inventory_generation=MAX(child_batch_heads.inventory_generation,excluded.inventory_generation),observed_at=excluded.observed_at,complete_generation=CASE WHEN excluded.complete_generation>0 THEN excluded.complete_generation ELSE child_batch_heads.complete_generation END,complete_observed_at=CASE WHEN excluded.complete_generation>0 THEN excluded.complete_observed_at ELSE child_batch_heads.complete_observed_at END")
        .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).bind(batch.sequence as i64)
        .bind(batch.inventory_generation as i64).bind(batch.observed_at)
        .bind(if authoritative_complete { batch.inventory_generation as i64 } else { 0 })
        .bind(if authoritative_complete { Some(batch.observed_at) } else { None })
        .execute(&mut *tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(ChildReportResponse {
        accepted: true,
        duplicate: false,
    }))
}
