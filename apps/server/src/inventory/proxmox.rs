//! Authenticated, host-scoped Proxmox guest inventory. VMIDs are local to a
//! reporting host; no guest-agent ownership is inferred from names or IPs.
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

pub async fn project(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
    host_id: Uuid,
    inventory: &Value,
    snapshot_at: OffsetDateTime,
) -> sqlx::Result<()> {
    let Some(data) = inventory.get("proxmox") else {
        return Ok(());
    };
    if !matches!(data["status"].as_str(), Some("available" | "partial")) {
        return Ok(());
    }
    let Some(at) = data["collected_at_unix_secs"]
        .as_i64()
        .and_then(|at| OffsetDateTime::from_unix_timestamp(at).ok())
    else {
        return Ok(());
    };
    // An old cache must not refresh or remove inventory, even on a replay.
    if !(0..=180).contains(&(snapshot_at - at).whole_seconds()) {
        return Ok(());
    }
    let Some(guests) = data["guests"].as_array() else {
        return Ok(());
    };
    let mut complete =
        data["complete"] == true && data["status"] == "available" && guests.len() <= 128;
    let mut seen = Vec::new();
    for guest in guests.iter().take(128) {
        let Some(vmid) = guest["vmid"]
            .as_u64()
            .filter(|vmid| (100..=999_999_999).contains(vmid))
        else {
            complete = false;
            continue;
        };
        let Some(kind @ ("vm" | "lxc")) = guest["kind"].as_str() else {
            complete = false;
            continue;
        };
        let key = format!("proxmox:{vmid}");
        if seen.contains(&key) {
            complete = false;
            continue;
        }
        seen.push(key.clone());
        let name = guest["name"]
            .as_str()
            .map(|name| name.chars().take(128).collect::<String>());
        let status = match guest["status"].as_str() {
            Some("running") => "running",
            Some("stopped") => "stopped",
            Some("paused") => "paused",
            _ => "unknown",
        };
        let workload_id: Option<Uuid> = sqlx::query_scalar(
            "insert into workloads(workload_type,host_device_id,runtime_id,name,status,source_type,agent_source_key,last_seen,is_current,image_or_template) \
             values ($1,$2,$3,$4,$5,'proxmox',$6,$7,true,$8) \
             on conflict(host_device_id,agent_source_key) where agent_source_key is not null do update \
             set workload_type=excluded.workload_type,name=excluded.name,status=excluded.status,last_seen=excluded.last_seen,is_current=true, \
                 image_or_template=excluded.image_or_template,updated_at=now(),version=workloads.version+1 \
             where workloads.last_seen is null or workloads.last_seen<=excluded.last_seen returning id")
            .bind(kind).bind(host_id).bind(vmid.to_string()).bind(name).bind(status).bind(&key).bind(at)
            .bind(if guest["template"] == true {Some("template")} else {None})
            .fetch_optional(&mut **tx).await?;
        let Some(workload_id) = workload_id else {
            continue;
        };
        let relation = if kind == "vm" {
            "hosts_vm"
        } else {
            "hosts_container"
        };
        // A reused VMID can change guest type; keep one current containment.
        sqlx::query("delete from containment_edges where parent_kind='devices' and parent_id=$1 and child_kind='workloads' and child_id=$2 and relation in ('hosts_vm','hosts_container') and relation<>$3")
            .bind(host_id).bind(workload_id).bind(relation).execute(&mut **tx).await?;
        sqlx::query("insert into containment_edges(parent_kind,parent_id,child_kind,child_id,relation) values ('devices',$1,'workloads',$2,$3) on conflict do nothing")
            .bind(host_id).bind(workload_id).bind(relation).execute(&mut **tx).await?;
    }
    if complete {
        sqlx::query("update workloads set is_current=false,status='not_present',updated_at=now(),version=version+1 \
            where host_device_id=$1 and source_type='proxmox' and is_current and last_seen<=$2 and not(agent_source_key=any($3))")
            .bind(host_id).bind(at).bind(&seen).execute(&mut **tx).await?;
        // Retain the workload and its host FK as history, but remove active
        // containment so dependency reasoning does not treat departed guests
        // as still running on this host.
        sqlx::query("delete from containment_edges where parent_kind='devices' and parent_id=$1 and child_kind='workloads' \
            and relation in ('hosts_vm','hosts_container') and child_id in \
            (select id from workloads where host_device_id=$1 and source_type='proxmox' and not is_current and last_seen<=$2)")
            .bind(host_id).bind(at).execute(&mut **tx).await?;
    }
    super::evidence::record_automatic_tx(
        tx,
        "devices",
        host_id,
        "agent",
        Some(&format!("agent:{agent_id}:proxmox")),
        "proxmox",
        &json!({"node":data["node"],"guests":seen.len(),"complete":complete}),
        0.9,
        false,
    )
    .await?;
    Ok(())
}

/// Batch both summaries and guest lists for merge members, preserving original
/// host ownership so undo-merge does not need to rewrite guest records.
pub async fn attach(pool: &PgPool, devices: &mut [Value], detail: bool) -> sqlx::Result<()> {
    let ids: Vec<Uuid> = devices
        .iter()
        .filter_map(|d| d["id"].as_str()?.parse().ok())
        .collect();
    let members = "with recursive members(root,id) as (select id,id from unnest($1::uuid[]) id union select m.root,d.id from members m join devices d on d.canonical_of=m.id) ";
    let rows: Vec<(Uuid, Uuid, Value, OffsetDateTime, bool)> = sqlx::query_as(&format!(
        "{members} select m.root,m.id,c.inventory->'proxmox',c.collected_at, \
         a.revoked_at is not null or coalesce(a.last_heartbeat_at,a.last_seen,a.created_at)<now()-make_interval(secs=>a.heartbeat_timeout_seconds::double precision) \
         from members m join agents a on a.device_id=m.id join agent_inventory_current c on c.agent_id=a.id \
         where jsonb_typeof(c.inventory->'proxmox')='object' order by c.collected_at desc,a.id"))
        .bind(&ids).fetch_all(pool).await?;
    let now = OffsetDateTime::now_utc();
    let mut summaries = std::collections::HashMap::<Uuid, Value>::new();
    let mut members_seen = std::collections::HashSet::new();
    for (root, member, data, snapshot_at, offline) in rows {
        if !members_seen.insert((root, member)) {
            continue;
        }
        let at = data["collected_at_unix_secs"]
            .as_i64()
            .and_then(|at| OffsetDateTime::from_unix_timestamp(at).ok());
        let fresh = at.is_some_and(|at| (0..=1200).contains(&(now - at).whole_seconds()))
            && (now - snapshot_at).whole_seconds() <= 1200
            && !offline;
        let status = match data["status"].as_str() {
            Some("available" | "partial") if !fresh => "stale",
            Some("available") => "available",
            Some("partial") => "partial",
            _ => "unavailable",
        };
        let summary = json!({"node":data["node"].as_str().map(|s| s.chars().take(255).collect::<String>()),"status":status,"error":data["error"].as_str().map(|s| s.chars().take(512).collect::<String>()),
            "collected_at":at.and_then(|at| at.format(&time::format_description::well_known::Rfc3339).ok()),"vm_count":0,"lxc_count":0,"guests":[]});
        let severity = |status: &str| match status {
            "available" => 0,
            "partial" => 1,
            "stale" => 2,
            _ => 3,
        };
        if summaries.get(&root).is_none_or(|current| {
            severity(status) > severity(current["status"].as_str().unwrap_or("unavailable"))
        }) {
            summaries.insert(root, summary);
        }
    }
    let guests: Vec<(Uuid, Value)> = sqlx::query_as(&format!(
        "{members} select m.root,jsonb_build_object('id',w.id,'host_device_id',m.root,'vmid',w.runtime_id,'kind',w.workload_type, \
         'name',w.name,'status',case when w.last_seen<now()-interval '20 minutes' and w.is_current then 'stale' else w.status end, \
         'last_seen',w.last_seen,'is_current',w.is_current,'template',w.image_or_template='template') \
         from members m join workloads w on w.host_device_id=m.id where w.source_type='proxmox' \
         order by m.root,w.is_current desc,w.runtime_id,w.id"))
        .bind(&ids).fetch_all(pool).await?;
    for (root, mut guest) in guests {
        let summary = summaries.entry(root).or_insert_with(
            || json!({"status":"unavailable","node":null,"vm_count":0,"lxc_count":0,"guests":[]}),
        );
        if guest["is_current"] == true {
            let key = if guest["kind"] == "vm" {
                "vm_count"
            } else {
                "lxc_count"
            };
            summary[key] = json!(summary[key].as_u64().unwrap_or(0) + 1);
            if summary["status"] != "available" {
                guest["status"] = json!("unknown");
            }
            if summary.get("states").is_none() {
                summary["states"] = json!({});
            }
            let state = guest["status"].as_str().unwrap_or("unknown");
            summary["states"][state] = json!(summary["states"][state].as_u64().unwrap_or(0) + 1);
        }
        if detail {
            summary["guests"].as_array_mut().unwrap().push(guest);
        }
    }
    for device in devices {
        if let Some(id) = device["id"].as_str().and_then(|id| id.parse::<Uuid>().ok())
            && let Some(summary) = summaries.remove(&id)
        {
            device["proxmox"] = summary;
        }
    }
    Ok(())
}
