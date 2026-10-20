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
