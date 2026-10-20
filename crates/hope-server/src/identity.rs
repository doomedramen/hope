//! Transactional source/host associations. Inventory reads never make identity decisions.
use crate::{
    associations,
    error::{ApiError, ApiResult},
    inventory::StoredHost,
    state::{State, hash, now},
};
use hope_protocol::{AssociationUpdate, ChildDeviceReport, Host, Network};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};

/// Reconcile after observations or settings change, in the same transaction as the write.
/// Existing confirmations and explicit rejections take precedence over automatic candidates.
pub(crate) async fn reconcile(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<bool> {
    reconcile_selected(tx, None).await
}
pub(crate) async fn reconcile_sources(
    tx: &mut Transaction<'_, Sqlite>,
    source_ids: &HashSet<String>,
) -> anyhow::Result<bool> {
    reconcile_selected(tx, Some(source_ids)).await
}
async fn reconcile_selected(
    tx: &mut Transaction<'_, Sqlite>,
    selected: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    let hosts = sqlx::query_scalar::<_, String>("SELECT body FROM hosts ORDER BY id")
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|body| StoredHost::decode(&body).map(StoredHost::into_view))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let networks =
        sqlx::query("SELECT id,name,cidr,enabled,interval_minutes FROM networks WHERE enabled=1")
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|row| Network {
                id: row.get("id"),
                name: row.get("name"),
                cidr: row.get("cidr"),
                enabled: row.get("enabled"),
                interval_minutes: row.get::<i64, _>("interval_minutes") as u32,
            })
            .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT source_device_id,host_id FROM device_associations WHERE status='confirmed'",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut by_source = HashMap::<String, String>::new();
    let mut by_host = HashMap::<String, String>::new();
    for row in rows {
        let source: String = row.get("source_device_id");
        let host: String = row.get("host_id");
        by_source.insert(source.clone(), host.clone());
        by_host.insert(host, source);
    }
    let sources=sqlx::query("SELECT id,body FROM source_devices d WHERE NOT EXISTS(SELECT 1 FROM source_device_aliases a WHERE a.alias_id=d.id) ORDER BY id")
        .fetch_all(&mut **tx).await?;
    for row in sources {
        let id: String = row.get("id");
        if selected.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        let child: ChildDeviceReport = serde_json::from_str(row.get("body"))?;
        // Only inferred suggestions expire. Manual decisions survive missing/changed evidence.
        sqlx::query("DELETE FROM device_associations WHERE source_device_id=? AND status='suggested' AND manual=0")
            .bind(&id).execute(&mut **tx).await?;
        for candidate in associations::candidates(&child, &hosts, &networks) {
            let prior:Option<(String,bool)>=sqlx::query_as("SELECT status,manual FROM device_associations WHERE source_device_id=? AND host_id=?")
                .bind(&id).bind(&candidate.host_id).fetch_optional(&mut **tx).await?;
            if prior
                .as_ref()
                .is_some_and(|(status, manual)| *manual || status == "confirmed")
            {
                continue;
            }
            let confirm = candidate.strong
                && by_source
                    .get(&id)
                    .is_none_or(|host| host == &candidate.host_id)
                && by_host
                    .get(&candidate.host_id)
                    .is_none_or(|source| source == &id);
            let evidence = serde_json::to_string(&candidate.evidence)?;
            sqlx::query("INSERT INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at,manual) VALUES(?,?,?,?,?,?,?,0) ON CONFLICT(source_device_id,host_id) DO UPDATE SET status=excluded.status,reason=excluded.reason,evidence=excluded.evidence,evidence_hash=excluded.evidence_hash,updated_at=excluded.updated_at")
                .bind(&id).bind(&candidate.host_id).bind(if confirm {"confirmed"} else {"suggested"})
                .bind(&candidate.reason).bind(&evidence).bind(hash(&evidence)).bind(now()).execute(&mut **tx).await?;
            if confirm {
                by_source.insert(id.clone(), candidate.host_id.clone());
                by_host.insert(candidate.host_id, id.clone());
            }
        }
    }
    let changed = sqlx::query("UPDATE source_devices SET host_id=(SELECT host_id FROM device_associations WHERE source_device_id=source_devices.id AND status='confirmed') WHERE host_id IS NOT (SELECT host_id FROM device_associations WHERE source_device_id=source_devices.id AND status='confirmed')")
        .execute(&mut **tx).await?.rows_affected() > 0;
    Ok(changed)
}

pub async fn change(state: &State, source_id: &str, update: AssociationUpdate) -> ApiResult<Host> {
    let _guard = state.0.writes.lock().await;
    let source_id = state.resolve_id(source_id).await?;
    let (host_id, action) = match update {
        AssociationUpdate::Confirm { host_id } => (host_id, "confirmed"),
        AssociationUpdate::Reject { host_id } => (host_id, "rejected"),
        AssociationUpdate::Unlink { host_id } => (host_id, "unlink"),
    };
    let host_id = state.resolve_id(&host_id).await?;
    let mut tx = state.write_transaction().await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_devices WHERE id=?) AND EXISTS(SELECT 1 FROM hosts WHERE id=?)")
        .bind(&source_id).bind(&host_id).fetch_one(&mut *tx).await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let prior: Option<String> = sqlx::query_scalar(
        "SELECT status FROM device_associations WHERE source_device_id=? AND host_id=?",
    )
    .bind(&source_id)
    .bind(&host_id)
    .fetch_optional(&mut *tx)
    .await?;
    if action == "unlink" && prior.as_deref() == Some("rejected") {
        // Reset: the UI labels this action Reset for a rejected or previously unlinked pair.
        sqlx::query("DELETE FROM device_associations WHERE source_device_id=? AND host_id=?")
            .bind(&source_id)
            .bind(&host_id)
            .execute(&mut *tx)
            .await?;
    } else {
        if action == "confirmed" {
            let taken:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM device_associations WHERE host_id=? AND status='confirmed' AND source_device_id!=?)")
                .bind(&host_id).bind(&source_id).fetch_one(&mut *tx).await?;
            if taken {
                return Err(ApiError::bad(
                    "This discovered host is already linked to another source device",
                ));
            }
            sqlx::query("DELETE FROM device_associations WHERE source_device_id=? AND status='confirmed' AND host_id!=?")
                .bind(&source_id).bind(&host_id).execute(&mut *tx).await?;
        }
        let status = if action == "confirmed" {
            "confirmed"
        } else {
            "rejected"
        };
        let reason = match action {
            "confirmed" => "Manually confirmed",
            "unlink" => "Unlinked by administrator",
            _ => "Rejected by administrator",
        };
        sqlx::query("INSERT INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at,manual) VALUES(?,?,?,?,'[]',?,?,1) ON CONFLICT(source_device_id,host_id) DO UPDATE SET status=excluded.status,reason=excluded.reason,updated_at=excluded.updated_at,manual=1")
            .bind(&source_id).bind(&host_id).bind(status).bind(reason).bind(hash("[]")).bind(now()).execute(&mut *tx).await?;
    }
    reconcile(&mut tx).await?;
    tx.commit().await?;
    state.changed();
    state.host(&host_id).await?.ok_or_else(ApiError::not_found)
}

pub(crate) fn same_interface_mac(
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

/// Apply the old per-node identity map only after the reporter joined the token-bound cluster
/// scope. Current observations and interface MACs must still prove a match before history moves.
pub(crate) async fn reconcile_legacy_source_maps(
    tx: &mut Transaction<'_, Sqlite>,
    source: &str,
    batch: &hope_protocol::ChildReportBatch,
    shared_scope: bool,
) -> ApiResult<()> {
    if shared_scope && let Some(old_scope) = batch.legacy_source_scope.as_deref() {
        for legacy in &batch.legacy_identities {
            let kind = match legacy.kind {
                hope_protocol::ChildDeviceKind::DockerContainer => "docker_container",
                hope_protocol::ChildDeviceKind::ProxmoxVm => "proxmox_vm",
                hope_protocol::ChildDeviceKind::ProxmoxLxc => "proxmox_lxc",
            };
            let old_row: Option<(String, String, f64, String)> = sqlx::query_as("SELECT id,body,observed_at,lifecycle FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=? AND kind=?")
                    .bind(&batch.agent_id).bind(source).bind(old_scope).bind(&legacy.key).bind(&legacy.incarnation).bind(kind)
                    .fetch_optional(&mut **tx).await?;
            let Some((old_id, old_body, old_seen, old_lifecycle)) = old_row else {
                continue;
            };
            // Keep metadata-only migration evidence pending until a current observation of
            // the canonical identity proves that the interface still matches. Old local
            // maps can survive VMID reuse, so a tuple alone must never move history.
            sqlx::query("INSERT OR IGNORE INTO pending_legacy_identities(source,parent_agent_id,legacy_source_device_id,source_scope,source_key,canonical_incarnation) VALUES(?,?,?,?,?,?)")
                    .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                    .execute(&mut **tx).await?;

            // The canonical owner may have reported before this remote reporter submitted its
            // legacy map. In that ordering no later guest batch from this reporter may arrive
            // (for example, its collector can be stopped), so reconcile against the already
            // stored shared identity now. The token-bound scope/key/incarnation selects the
            // canonical record; matching kind, fresh timestamps, and an interface MAC prove the
            // legacy row is the same current guest rather than a reused VMID.
            crate::identity::reconcile_shared_identity(
                tx,
                source,
                &batch.source_scope,
                &legacy.key,
                &legacy.canonical_incarnation,
            )
            .await?;
            let canonical_id: Option<String> = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE source=? AND identity_owner='@shared' AND source_scope=? AND source_key=? AND incarnation=?")
                .bind(source).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                .fetch_optional(&mut **tx).await?;
            if old_lifecycle != "archived"
                && let Some(canonical_id) = canonical_id
                && canonical_id != old_id
            {
                let canonical: Option<(String, String, f64, String)> = sqlx::query_as(
                    "SELECT kind,body,observed_at,lifecycle FROM source_devices WHERE id=?",
                )
                .bind(&canonical_id)
                .fetch_optional(&mut **tx)
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
                            tx,
                            &canonical_id,
                            &old_id,
                            &batch.agent_id,
                        )
                        .await?;
                        sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                            .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                            .execute(&mut **tx).await?;
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
                    .bind(&old_id).bind(&batch.agent_id).fetch_all(&mut **tx).await?;
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
                        tx,
                        survivor,
                        duplicate,
                        duplicate_parent,
                    )
                    .await?;
                    sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                        .bind(source).bind(duplicate_parent).bind(duplicate).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                        .execute(&mut **tx).await?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn merge_source_device(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    survivor: &str,
    duplicate: &str,
) -> ApiResult<()> {
    merge_source_device_inner(tx, survivor, duplicate, None).await
}

pub(crate) async fn merge_legacy_source_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    survivor: &str,
    duplicate: &str,
    reporting_agent: &str,
) -> ApiResult<()> {
    merge_source_device_inner(tx, survivor, duplicate, Some(reporting_agent)).await
}

async fn merge_source_device_inner(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    survivor: &str,
    duplicate: &str,
    legacy_reporter: Option<&str>,
) -> ApiResult<()> {
    let compatible: bool = if let Some(reporter) = legacy_reporter {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_devices a JOIN source_devices b ON a.id=? AND b.id=? WHERE a.source=b.source AND a.kind=b.kind AND a.source_key=b.source_key AND b.parent_agent_id=?)")
            .bind(survivor).bind(duplicate).bind(reporter).fetch_one(&mut **tx).await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_devices a JOIN source_devices b ON a.id=? AND b.id=? WHERE a.source=b.source AND a.kind=b.kind AND ((a.source_scope=b.source_scope AND a.source_key=b.source_key AND a.incarnation=b.incarnation AND (a.parent_agent_id=b.parent_agent_id OR (EXISTS(SELECT 1 FROM shared_source_members m WHERE m.source=a.source AND m.source_scope=a.source_scope AND m.agent_id=a.parent_agent_id) AND EXISTS(SELECT 1 FROM shared_source_members m WHERE m.source=b.source AND m.source_scope=b.source_scope AND m.agent_id=b.parent_agent_id)))) OR (a.source_scope!=b.source_scope AND a.source_key=b.source_key AND abs(a.observed_at-b.observed_at)<604800 AND NOT EXISTS(SELECT 1 FROM shared_source_scopes s WHERE s.source=a.source AND s.source_scope IN (a.source_scope,b.source_scope)))))")
        .bind(survivor).bind(duplicate).fetch_one(&mut **tx).await?
    };
    if !compatible {
        return Err(ApiError::bad(
            "Devices do not share a proven source identity",
        ));
    }
    let exact_identity: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_devices a JOIN source_devices b ON a.source=b.source AND a.source_scope=b.source_scope AND a.source_key=b.source_key AND a.incarnation=b.incarnation AND a.kind=b.kind WHERE a.id=? AND b.id=?)")
        .bind(survivor).bind(duplicate).fetch_one(&mut **tx).await?;
    if !exact_identity {
        let bodies: Vec<String> =
            sqlx::query_scalar("SELECT body FROM source_devices WHERE id IN (?,?)")
                .bind(survivor)
                .bind(duplicate)
                .fetch_all(&mut **tx)
                .await?;
        let reports: Vec<hope_protocol::ChildDeviceReport> = bodies
            .iter()
            .map(|body| serde_json::from_str(body))
            .collect::<Result<_, _>>()?;
        if reports.len() != 2 || !same_interface_mac(&reports[0], &reports[1]) {
            return Err(ApiError::bad(
                "Legacy candidates need a shared interface MAC",
            ));
        }
    }
    sqlx::query("UPDATE source_device_identities SET source_device_id=? WHERE source_device_id=?")
        .bind(survivor)
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    if exact_identity && legacy_reporter.is_none() {
        crate::metrics::merge_source_history(tx, survivor, duplicate).await?;
    }
    merge_associations(tx, survivor, duplicate).await?;
    sqlx::query("UPDATE source_devices SET host_id=COALESCE(host_id,(SELECT host_id FROM source_devices WHERE id=?)) WHERE id=?").bind(duplicate).bind(survivor).execute(&mut **tx).await?;
    sqlx::query("INSERT OR IGNORE INTO device_relationships SELECT CASE WHEN from_device_id=? THEN ? ELSE from_device_id END,CASE WHEN to_device_id=? THEN ? ELSE to_device_id END,kind,detail,?,observed_at FROM device_relationships WHERE source_device_id=? AND kind!='runs_on'")
        .bind(duplicate).bind(survivor).bind(duplicate).bind(survivor).bind(survivor).bind(duplicate).execute(&mut **tx).await?;
    sqlx::query("INSERT OR IGNORE INTO device_relationships SELECT ?,to_device_id,'runs_on',detail,?,observed_at FROM device_relationships WHERE source_device_id=? AND kind='runs_on' AND NOT EXISTS(SELECT 1 FROM device_relationships WHERE source_device_id=? AND kind='runs_on')")
        .bind(survivor).bind(survivor).bind(duplicate).bind(survivor).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM device_relationships WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM device_associations WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO retired_devices(device_id,retired_at) SELECT ?,retired_at FROM retired_devices WHERE device_id=?")
        .bind(survivor)
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM retired_devices WHERE device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO source_device_aliases(alias_id,survivor_id) VALUES(?,?) ON CONFLICT(alias_id) DO UPDATE SET survivor_id=excluded.survivor_id").bind(duplicate).bind(survivor).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO device_id_aliases(alias_id,survivor_id) VALUES(?,?) ON CONFLICT(alias_id) DO UPDATE SET survivor_id=excluded.survivor_id").bind(duplicate).bind(survivor).execute(&mut **tx).await?;
    sqlx::query("UPDATE source_device_aliases SET survivor_id=? WHERE survivor_id=?")
        .bind(survivor)
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE device_id_aliases SET survivor_id=? WHERE survivor_id=?")
        .bind(survivor)
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM source_devices WHERE id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Consolidate old parent-scoped rows only after every contributing reporter has joined the
/// token-bound scope. The exact incarnation must match, so a reused VMID remains separate.
pub(crate) async fn reconcile_shared_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    source: &str,
    scope: &str,
    key: &str,
    incarnation: &str,
) -> ApiResult<()> {
    let rows = sqlx::query("SELECT d.id FROM source_devices d JOIN shared_source_members m ON m.source=d.source AND m.source_scope=d.source_scope AND m.agent_id=d.parent_agent_id WHERE d.source=? AND d.source_scope=? AND d.source_key=? AND d.incarnation=? ORDER BY d.ownership_generation DESC,d.observed_at DESC,d.id")
        .bind(source).bind(scope).bind(key).bind(incarnation).fetch_all(&mut **tx).await?;
    let Some(first) = rows.first() else {
        return Ok(());
    };
    let survivor: String = first.get("id");
    for row in rows.iter().skip(1) {
        let duplicate: String = row.get("id");
        merge_source_device(tx, &survivor, &duplicate).await?;
    }
    sqlx::query("INSERT INTO source_device_identities(source,identity_owner,source_scope,source_key,incarnation,source_device_id) VALUES(?,'@shared',?,?,?,?) ON CONFLICT(source,identity_owner,source_scope,source_key,incarnation) DO UPDATE SET source_device_id=excluded.source_device_id")
        .bind(source).bind(scope).bind(key).bind(incarnation).bind(&survivor).execute(&mut **tx).await?;
    Ok(())
}

/// Transfer associations before installing the old source's URL alias. Delete first so the
/// unique host index cannot discard the duplicate's confirmed association during the transfer.
pub(crate) async fn merge_associations(
    tx: &mut Transaction<'_, Sqlite>,
    survivor: &str,
    duplicate: &str,
) -> anyhow::Result<()> {
    let rows=sqlx::query("SELECT host_id,status,reason,evidence,evidence_hash,updated_at,manual FROM device_associations WHERE source_device_id=? ORDER BY manual DESC,updated_at DESC")
        .bind(duplicate).fetch_all(&mut **tx).await?;
    sqlx::query("DELETE FROM device_associations WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    let mut confirmed: Option<String> = sqlx::query_scalar(
        "SELECT host_id FROM device_associations WHERE source_device_id=? AND status='confirmed'",
    )
    .bind(survivor)
    .fetch_optional(&mut **tx)
    .await?;
    for row in rows {
        let host: String = row.get("host_id");
        let mut status: String = row.get("status");
        if status == "confirmed" && confirmed.as_ref().is_some_and(|current| current != &host) {
            status = "suggested".into();
        }
        let inserted = sqlx::query("INSERT OR IGNORE INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at,manual) VALUES(?,?,?,?,?,?,?,?)")
            .bind(survivor).bind(&host).bind(&status).bind(row.get::<String,_>("reason"))
            .bind(row.get::<String,_>("evidence")).bind(row.get::<String,_>("evidence_hash"))
            .bind(row.get::<f64,_>("updated_at")).bind(row.get::<bool,_>("manual"))
            .execute(&mut **tx).await?.rows_affected() == 1;
        if inserted && status == "confirmed" {
            confirmed.get_or_insert(host);
        }
    }
    sqlx::query("INSERT OR IGNORE INTO source_device_settings(source_device_id,body) SELECT ?,body FROM source_device_settings WHERE source_device_id=?")
        .bind(survivor).bind(duplicate).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM source_device_settings WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE source_devices SET host_id=(SELECT host_id FROM device_associations WHERE source_device_id=source_devices.id AND status='confirmed') WHERE id IN (?,?)")
        .bind(survivor).bind(duplicate).execute(&mut **tx).await?;
    Ok(())
}
