//! Fixed, read-only Proxmox collection. This command is installed only as a root-owned systemd
//! service; the reporting process reads its bounded, group-readable snapshot from `/run`.
use crate::collect::ChildSnapshot;
use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, stream};
use hope_protocol::{
    ChildDeviceKind, ChildDeviceReport, ChildMetrics, ChildSourceKind, CpuMeasurement,
    DeviceLifecycle, Interface, MemoryMeasurement, SourceIdentity,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;

const SNAPSHOT: &str = "/run/hope-agent/proxmox.json";
const SHARED_STATE: &str = "/etc/pve/priv/hope-agent";
const SHARED_CLUSTER_ID: &str = "/etc/pve/priv/hope-agent/cluster-id";
const SHARED_SOURCE_TOKEN: &str = "/etc/pve/priv/hope-agent/source-token";
const GUEST_IDENTITIES: &str = "/etc/pve/priv/hope-agent/guest-identities.json";
const LEGACY_GUEST_IDENTITIES: &str = "/etc/hope-agent/proxmox-guest-identities.json";
const SHARED_LOCK: &str = "/etc/pve/priv/lock/hope-agent";
const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PVE_OUTPUT: usize = 8 * 1024 * 1024;
const CONFIG_CACHE: &str = "/run/hope-agent/proxmox-config.json";
const RATE_CACHE: &str = "/run/hope-agent/proxmox-rates.json";
const CONFIG_REFRESH_SECONDS: u64 = 300;
const MAX_GUEST_CONFIG_CONCURRENCY: usize = 2;

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    source_scope: String,
    source_token: Option<String>,
    legacy_source_scope: Option<String>,
    #[serde(default)]
    legacy_identities: Vec<hope_protocol::LegacySourceIdentity>,
    #[serde(default)]
    proxmox_guest_presence: Option<Vec<hope_protocol::ProxmoxGuestPresence>>,
    #[serde(default)]
    observed_at: f64,
    devices: Vec<ChildDeviceReport>,
    inventory_complete: bool,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ConfigCache {
    entries: HashMap<String, CachedConfig>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedConfig {
    observed_at: u64,
    value: Value,
    #[serde(default)]
    guest_data: Option<GuestDataCache>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct GuestDataCache {
    observed_at: f64,
    status: Option<hope_protocol::GuestAgentStatus>,
    os: Option<hope_protocol::GuestOsMetadata>,
    filesystems: Vec<hope_protocol::GuestFilesystem>,
    interfaces: Vec<Interface>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct RateCache {
    entries: HashMap<String, MetricBaseline>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MetricBaseline {
    observed_at: f64,
    disk_read_bytes: Option<f64>,
    disk_write_bytes: Option<f64>,
    network_rx_bytes: Option<f64>,
    network_tx_bytes: Option<f64>,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct GuestIdentityMap {
    source_scope: String,
    last_complete_at: f64,
    entries: HashMap<String, GuestIdentity>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct GuestIdentity {
    incarnation: String,
    #[serde(default)]
    owner_node: Option<String>,
    #[serde(default)]
    ownership_generation: u64,
    missing_count: u32,
    missing_since: Option<f64>,
}

/// Read a root-produced snapshot without following symlinks or accepting unbounded files.
pub fn read_snapshot() -> Option<ChildSnapshot> {
    let path = Path::new(SNAPSHOT);
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let file: SnapshotFile = serde_json::from_slice(&bytes).ok()?;
    Uuid::parse_str(&file.source_scope).ok()?;
    let file_observed_at = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|age| age.as_secs_f64())?;
    let observed_at = if file.observed_at.is_finite() && file.observed_at > 0.0 {
        file.observed_at
    } else {
        file_observed_at
    };
    let fresh = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age <= Duration::from_secs(90));
    Some(ChildSnapshot {
        source: ChildSourceKind::Proxmox,
        source_scope: file.source_scope,
        source_token: file.source_token,
        legacy_source_scope: file.legacy_source_scope,
        legacy_identities: file.legacy_identities,
        proxmox_guest_presence: if fresh {
            file.proxmox_guest_presence
        } else {
            None
        },
        observed_at,
        devices: if fresh { file.devices } else { vec![] },
        inventory_complete: fresh && file.inventory_complete,
    })
}

pub fn collector_status() -> Option<hope_protocol::CollectorStatus> {
    pvesh_binary()?;
    let snapshot = read_snapshot();
    let available = snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.inventory_complete);
    let detail = if available {
        None
    } else if snapshot.is_none() {
        Some("Proxmox collection is unavailable; install the system agent to enable its root-owned collector.".into())
    } else {
        Some("The latest Proxmox inventory is stale or incomplete; check pvesh permissions and collector logs.".into())
    };
    Some(hope_protocol::CollectorStatus {
        name: "proxmox_children".into(),
        available,
        detail,
    })
}

/// Collect one current inventory using only fixed `pvesh` reads. No request data is used as an
/// executable or command argument except validated node and numeric guest identifiers.
pub async fn collect() -> Result<()> {
    let (cluster_id, source_token) = cluster_identity()?;
    let mut complete = true;
    let resources = match pvesh(&[
        "get",
        "/cluster/resources",
        "--type",
        "vm",
        "--output-format",
        "json",
    ])
    .await
    {
        Ok(value) => value,
        Err(error) => {
            write_snapshot(&SnapshotFile {
                source_scope: cluster_id,
                source_token: Some(source_token),
                legacy_source_scope: None,
                legacy_identities: vec![],
                proxmox_guest_presence: None,
                observed_at: unix_now(),
                devices: vec![],
                inventory_complete: false,
            })?;
            return Err(error).context("read Proxmox cluster resources");
        }
    };
    let Some(resources) = resources.as_array() else {
        write_snapshot(&SnapshotFile {
            source_scope: cluster_id,
            source_token: Some(source_token),
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            observed_at: unix_now(),
            devices: vec![],
            inventory_complete: false,
        })?;
        bail!("Proxmox resources must be a JSON array");
    };
    if resources.len() > 500 {
        complete = false;
    }
    let local_node = local_node_name();
    let Some(local_node) = local_node else {
        write_snapshot(&SnapshotFile {
            source_scope: cluster_id,
            source_token: Some(source_token),
            legacy_source_scope: None,
            legacy_identities: vec![],
            proxmox_guest_presence: None,
            observed_at: unix_now(),
            devices: vec![],
            inventory_complete: false,
        })?;
        bail!("could not determine the local Proxmox node name");
    };
    // These are independent, node-local bulk listings. Run them concurrently so their two
    // bounded pvesh deadlines do not stack on top of the cluster resources request and the
    // collector's bounded enrichment work.
    let qemu_path = format!("/nodes/{local_node}/qemu");
    let lxc_path = format!("/nodes/{local_node}/lxc");
    let qemu_args = [
        "get",
        qemu_path.as_str(),
        // Proxmox's VM index can include full active-guest status in the same bulk request.
        "--full",
        "1",
        "--output-format",
        "json",
    ];
    let lxc_args = ["get", lxc_path.as_str(), "--output-format", "json"];
    let (qemu, lxc) = tokio::join!(pvesh(&qemu_args), pvesh(&lxc_args));
    let qemu = match qemu {
        Ok(value) => value,
        Err(error) => {
            complete = false;
            eprintln!("Proxmox QEMU inventory is partial: {error:#}");
            Value::Array(vec![])
        }
    };
    let lxc = match lxc {
        Ok(value) => value,
        Err(error) => {
            complete = false;
            eprintln!("Proxmox LXC inventory is partial: {error:#}");
            Value::Array(vec![])
        }
    };
    let qemu = qemu.as_array().cloned().unwrap_or_else(|| {
        complete = false;
        eprintln!("Proxmox QEMU inventory is not a JSON array");
        vec![]
    });
    let lxc = lxc.as_array().cloned().unwrap_or_else(|| {
        complete = false;
        eprintln!("Proxmox LXC inventory is not a JSON array");
        vec![]
    });
    let mut global_members = HashMap::new();
    for resource in resources.iter().take(500) {
        let Some(vmid) = resource.get("vmid").and_then(Value::as_u64) else {
            complete = false;
            continue;
        };
        let kind = resource
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(kind, "qemu" | "lxc") {
            global_members.insert((kind.to_owned(), vmid), resource);
        }
    }
    let global_present = global_members
        .keys()
        .map(|(kind, vmid)| {
            let kind = if kind == "qemu" {
                ChildDeviceKind::ProxmoxVm
            } else {
                ChildDeviceKind::ProxmoxLxc
            };
            format!("{kind:?}:{vmid}")
        })
        .collect::<HashSet<_>>();
    let local_count = qemu.len().saturating_add(lxc.len());
    if local_count > 500 {
        complete = false;
    }
    let mut parsed = Vec::with_capacity(local_count.min(500));
    for (kind, list) in [("qemu", qemu), ("lxc", lxc)] {
        for item in list.iter().take(500) {
            let Some(vmid) = item.get("vmid").and_then(Value::as_u64) else {
                complete = false;
                continue;
            };
            // The node-local endpoint supplies status/counters; the cluster endpoint provides
            // authoritative membership evidence for safe absence and migration handling.
            let Some(member) = global_members.get(&(kind.to_owned(), vmid)) else {
                complete = false;
                continue;
            };
            if member.get("node").and_then(Value::as_str) != Some(local_node.as_str()) {
                // A pre-migration local listing can lag the cluster view. Never let it reclaim
                // ownership or emit a foreign guest under this host's authenticated agent.
                complete = false;
                continue;
            }
            let mut resource = item.clone();
            if let Some(object) = resource.as_object_mut() {
                object.insert("type".into(), Value::String(kind.into()));
                object.insert("node".into(), Value::String(local_node.clone()));
                for field in ["maxcpu", "maxmem", "maxdisk", "tags"] {
                    if !object.contains_key(field)
                        && let Some(value) = member.get(field)
                    {
                        object.insert(field.into(), value.clone());
                    }
                }
            }
            let Some(device) = resource_device(&resource, &cluster_id) else {
                complete = false;
                continue;
            };
            if !device
                .status
                .as_deref()
                .is_some_and(|status| matches!(status, "running" | "stopped" | "paused"))
            {
                complete = false;
            }
            parsed.push(device);
        }
    }
    let observed_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let now = observed_at as u64;
    let mut config_cache = read_config_cache();
    let current_keys = parsed
        .iter()
        .filter_map(config_key)
        .collect::<std::collections::HashSet<_>>();
    config_cache
        .entries
        .retain(|key, _| current_keys.contains(key));
    let cached_configs = config_cache.entries.clone();
    let mut tasks = stream::iter(parsed.into_iter().map(|mut device| {
        let cached = config_key(&device).and_then(|key| cached_configs.get(&key).cloned());
        async move {
            let key = config_key(&device);
            let should_refresh = cached.as_ref().is_none_or(|entry| {
                now.saturating_sub(entry.observed_at) >= CONFIG_REFRESH_SECONDS
            });
            let config = if should_refresh {
                match guest_config(&device).await {
                    Some(value) => Some(CachedConfig {
                        observed_at: now,
                        value,
                        guest_data: cached.as_ref().and_then(|entry| entry.guest_data.clone()),
                    }),
                    None => cached,
                }
            } else {
                cached
            };
            if let Some(config) = config {
                enrich_from_config(&mut device, &config.value);
                apply_cached_guest_data(&mut device, config.guest_data.as_ref());
                return (device, key, Some(config));
            }
            (device, None, None)
        }
    }))
    .buffer_unordered(MAX_GUEST_CONFIG_CONCURRENCY);
    let mut devices = vec![];
    while let Some((device, key, config)) = tasks.next().await {
        if let (Some(key), Some(config)) = (key, config) {
            config_cache.entries.insert(key, config);
        }
        devices.push(device);
    }
    drop(tasks);
    devices.sort_by(|a, b| {
        format!("{:?}", a.identity.kind)
            .cmp(&format!("{:?}", b.identity.kind))
            .then_with(|| a.identity.key.cmp(&b.identity.key))
    });
    refresh_guest_enrichment(&mut devices, &mut config_cache, observed_at).await;
    write_config_cache(&config_cache)?;
    let (legacy_source_scope, legacy_identities, proxmox_guest_presence) = update_guest_identities(
        &mut devices,
        &cluster_id,
        complete,
        observed_at,
        &global_present,
    )?;
    add_metric_rates(&mut devices, observed_at)?;
    write_snapshot(&SnapshotFile {
        source_scope: cluster_id,
        source_token: Some(source_token),
        legacy_source_scope,
        legacy_identities,
        proxmox_guest_presence: Some(proxmox_guest_presence),
        observed_at,
        devices,
        inventory_complete: complete,
    })
}

fn local_node_name() -> Option<String> {
    // Proxmox's pmxcfs exposes the configured node name here. Fall back to the OS hostname for
    // test fixtures and systems where the cluster filesystem has not mounted yet.
    std::fs::read_link("/etc/pve/local")
        .ok()
        .and_then(|path| path.file_name()?.to_str().and_then(safe_node))
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .and_then(|name| safe_node(name.trim()))
        })
}

#[cfg(test)]
fn is_local_guest(device: &ChildDeviceReport, local_node: &str) -> bool {
    device.node.as_deref() == Some(local_node)
}

fn local_owner_config_exists(device: &ChildDeviceReport) -> bool {
    let Some(node) = device.node.as_deref().and_then(safe_node) else {
        return false;
    };
    let Ok(vmid) = device.identity.key.parse::<u64>() else {
        return false;
    };
    let directory = match device.identity.kind {
        ChildDeviceKind::ProxmoxVm => "qemu-server",
        ChildDeviceKind::ProxmoxLxc => "lxc",
        ChildDeviceKind::DockerContainer => return false,
    };
    std::fs::symlink_metadata(format!("/etc/pve/nodes/{node}/{directory}/{vmid}.conf"))
        .is_ok_and(|metadata| metadata.file_type().is_file())
}

fn update_guest_identities(
    devices: &mut Vec<ChildDeviceReport>,
    source_scope: &str,
    complete: bool,
    now: f64,
    global_present: &HashSet<String>,
) -> Result<(
    Option<String>,
    Vec<hope_protocol::LegacySourceIdentity>,
    Vec<hope_protocol::ProxmoxGuestPresence>,
)> {
    let _lock = acquire_shared_lock()?;
    devices.retain(local_owner_config_exists);
    let local_legacy = read_private::<GuestIdentityMap>(LEGACY_GUEST_IDENTITIES);
    let legacy_scope = local_legacy
        .as_ref()
        .filter(|map| map.source_scope != source_scope)
        .map(|map| map.source_scope.clone());
    if let Some(legacy) = &local_legacy {
        for device in devices.iter_mut() {
            let key = format!("{:?}:{}", device.identity.kind, device.identity.key);
            device.legacy_incarnation = legacy
                .entries
                .get(&key)
                .map(|identity| identity.incarnation.clone());
        }
    }
    let mut identities = read_shared::<GuestIdentityMap>(GUEST_IDENTITIES)?
        .filter(|identities| identities.source_scope == source_scope)
        .unwrap_or_else(|| {
            // Upgrade from the old per-node state without changing existing incarnations. The
            // first node to acquire pmxcfs's distributed lock seeds the cluster-wide map.
            let mut legacy = local_legacy.clone().unwrap_or_default();
            legacy.source_scope = source_scope.to_owned();
            legacy
        });
    if assign_guest_identities(
        &mut identities,
        devices,
        source_scope,
        complete,
        now,
        Some(global_present),
    ) {
        write_shared(GUEST_IDENTITIES, &identities)?;
    }
    let legacy_identities = local_legacy
        .as_ref()
        .filter(|_| legacy_scope.is_some())
        .map(|legacy| {
            legacy
                .entries
                .iter()
                .filter_map(|(legacy_key, old)| {
                    let (kind, key) = legacy_key.split_once(':')?;
                    let kind = match kind {
                        "ProxmoxVm" => ChildDeviceKind::ProxmoxVm,
                        "ProxmoxLxc" => ChildDeviceKind::ProxmoxLxc,
                        _ => return None,
                    };
                    let canonical_key = format!("{kind:?}:{key}");
                    let current = identities.entries.get(&canonical_key)?;
                    Some(hope_protocol::LegacySourceIdentity {
                        kind,
                        key: key.to_owned(),
                        incarnation: old.incarnation.clone(),
                        canonical_incarnation: current.incarnation.clone(),
                    })
                })
                .take(500)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut proxmox_guest_presence = global_present
        .iter()
        .filter_map(|key| {
            let (kind, key) = key.split_once(':')?;
            let kind = match kind {
                "ProxmoxVm" => ChildDeviceKind::ProxmoxVm,
                "ProxmoxLxc" => ChildDeviceKind::ProxmoxLxc,
                _ => return None,
            };
            let incarnation = identities
                .entries
                .get(&format!("{kind:?}:{key}"))
                .map(|entry| entry.incarnation.clone());
            Some(hope_protocol::ProxmoxGuestPresence {
                kind,
                key: key.to_owned(),
                incarnation,
            })
        })
        .collect::<Vec<_>>();
    proxmox_guest_presence
        .sort_by(|left, right| (left.kind as u8, &left.key).cmp(&(right.kind as u8, &right.key)));
    Ok((legacy_scope, legacy_identities, proxmox_guest_presence))
}

fn assign_guest_identities(
    identities: &mut GuestIdentityMap,
    devices: &mut [ChildDeviceReport],
    source_scope: &str,
    complete: bool,
    now: f64,
    global_present: Option<&HashSet<String>>,
) -> bool {
    let mut changed = false;
    if identities.source_scope != source_scope {
        *identities = GuestIdentityMap {
            source_scope: source_scope.to_owned(),
            ..GuestIdentityMap::default()
        };
        changed = true;
    }
    let mut present = HashSet::new();
    for device in devices.iter_mut() {
        let key = format!("{:?}:{}", device.identity.kind, device.identity.key);
        present.insert(key.clone());
        let entry = identities.entries.entry(key).or_insert_with(|| {
            changed = true;
            GuestIdentity {
                incarnation: Uuid::new_v4().to_string(),
                owner_node: device.node.clone(),
                ownership_generation: 1,
                missing_count: 0,
                missing_since: None,
            }
        });
        if entry.owner_node != device.node {
            entry.owner_node = device.node.clone();
            entry.ownership_generation = entry.ownership_generation.saturating_add(1).max(1);
            changed = true;
        }
        if entry.missing_count >= 3
            && entry
                .missing_since
                .is_some_and(|since| now - since >= 300.0)
        {
            entry.incarnation = Uuid::new_v4().to_string();
            changed = true;
        }
        if entry.missing_count > 0 || entry.missing_since.is_some() {
            entry.missing_count = 0;
            entry.missing_since = None;
            changed = true;
        }
        device.identity.incarnation = entry.incarnation.clone();
        device.ownership_generation = Some(entry.ownership_generation);
    }
    if complete && (identities.last_complete_at == 0.0 || now - identities.last_complete_at >= 60.0)
    {
        identities.last_complete_at = now;
        for (key, entry) in &mut identities.entries {
            if global_present
                .map(|all| all.contains(key))
                .unwrap_or_else(|| present.contains(key))
            {
                continue;
            }
            entry.missing_count = entry.missing_count.saturating_add(1);
            entry.missing_since.get_or_insert(now);
        }
        identities.entries.retain(|_, entry| {
            entry
                .missing_since
                .is_none_or(|since| now - since < 30.0 * 86400.0)
        });
        changed = true;
    }
    changed
}

fn resource_device(resource: &Value, cluster_id: &str) -> Option<ChildDeviceReport> {
    let kind = match resource.get("type")?.as_str()? {
        "qemu" => ChildDeviceKind::ProxmoxVm,
        "lxc" => ChildDeviceKind::ProxmoxLxc,
        _ => return None,
    };
    let vmid = resource.get("vmid")?.as_u64()?;
    let key = vmid.to_string();
    let state = resource
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let lifecycle = match state {
        "running" => DeviceLifecycle::Running,
        "stopped" | "paused" => DeviceLifecycle::Stopped,
        _ => DeviceLifecycle::Stale,
    };
    let cpu_capacity = resource
        .get("maxcpu")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0);
    let cpu_percent = resource
        .get("cpu")
        .and_then(Value::as_f64)
        .map(|value| value * 100.0);
    let memory_used = resource.get("mem").and_then(Value::as_f64);
    let memory_total = resource.get("maxmem").and_then(Value::as_f64);
    let node = resource
        .get("node")
        .and_then(Value::as_str)
        .and_then(safe_node);
    let name = resource
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{} {vmid}",
                if kind == ChildDeviceKind::ProxmoxVm {
                    "VM"
                } else {
                    "LXC"
                }
            )
        });
    let mut metrics = ChildMetrics::default();
    if lifecycle == DeviceLifecycle::Running {
        metrics.cpu_percent = cpu_percent;
        metrics.cpu_capacity_cores = cpu_capacity;
        metrics.cpu_measurement = Some(CpuMeasurement::HypervisorGuest);
        metrics.memory_used_bytes = memory_used;
        metrics.memory_total_bytes = memory_total;
        metrics.memory_measurement = Some(MemoryMeasurement::HypervisorGuest);
        metrics.disk_used_bytes = resource.get("disk").and_then(Value::as_f64);
        metrics.disk_total_bytes = resource.get("maxdisk").and_then(Value::as_f64);
        metrics.disk_read_bytes = first_number(resource, &["diskread", "disk_read"]);
        metrics.disk_write_bytes = first_number(resource, &["diskwrite", "disk_write"]);
        metrics.uptime_seconds = resource.get("uptime").and_then(Value::as_f64);
        metrics.network_rx_bytes = resource.get("netin").and_then(Value::as_f64);
        metrics.network_tx_bytes = resource.get("netout").and_then(Value::as_f64);
    }
    let tags = resource
        .get("tags")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split([',', ';'])
        .filter(|tag| !tag.trim().is_empty())
        .map(|tag| tag.trim().to_owned())
        .take(64)
        .collect();
    Some(ChildDeviceReport {
        identity: SourceIdentity {
            kind,
            scope: cluster_id.to_owned(),
            key: key.clone(),
            // Replaced by the persistent VMID-incarnation map after the complete inventory read.
            incarnation: format!("vmid-{vmid}"),
        },
        ownership_generation: None,
        legacy_incarnation: None,
        name,
        lifecycle,
        status: Some(state.to_owned()),
        health: None,
        restart_count: None,
        parent_key: None,
        node,
        tags,
        interfaces: vec![],
        cpu_cores: cpu_capacity,
        memory_bytes: memory_total,
        image: None,
        compose_project: None,
        compose_service: None,
        published_ports: vec![],
        guest_agent_status: None,
        guest_os: None,
        guest_filesystems: vec![],
        network_configuration: vec![],
        disk_configuration: vec![],
        metrics,
    })
}

fn first_number(value: &Value, fields: &[&str]) -> Option<f64> {
    fields
        .iter()
        .find_map(|field| value.get(field).and_then(Value::as_f64))
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn add_metric_rates(devices: &mut [ChildDeviceReport], observed_at: f64) -> Result<()> {
    let mut cache = read_rate_cache();
    let mut next = HashMap::new();
    for device in devices {
        if device.lifecycle != DeviceLifecycle::Running {
            continue;
        }
        let key = format!(
            "{:?}:{}:{}:{}",
            device.identity.kind,
            device.identity.scope,
            device.identity.key,
            device.identity.incarnation
        );
        let current = MetricBaseline {
            observed_at,
            disk_read_bytes: device.metrics.disk_read_bytes,
            disk_write_bytes: device.metrics.disk_write_bytes,
            network_rx_bytes: device.metrics.network_rx_bytes,
            network_tx_bytes: device.metrics.network_tx_bytes,
        };
        if let Some(previous) = cache.entries.get(&key) {
            let elapsed = observed_at - previous.observed_at;
            if (1.0..=180.0).contains(&elapsed) {
                device.metrics.disk_read_bytes_per_second =
                    counter_rate(previous.disk_read_bytes, current.disk_read_bytes, elapsed);
                device.metrics.disk_write_bytes_per_second =
                    counter_rate(previous.disk_write_bytes, current.disk_write_bytes, elapsed);
                device.metrics.network_rx_bytes_per_second =
                    counter_rate(previous.network_rx_bytes, current.network_rx_bytes, elapsed);
                device.metrics.network_tx_bytes_per_second =
                    counter_rate(previous.network_tx_bytes, current.network_tx_bytes, elapsed);
            }
        }
        next.insert(key, current);
    }
    cache.entries = next;
    let bytes = serde_json::to_vec(&cache)?;
    if bytes.len() > MAX_SNAPSHOT_BYTES as usize {
        bail!("Proxmox metric rate cache exceeded its size limit")
    }
    let temporary = format!("{RATE_CACHE}.tmp-{}", std::process::id());
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, RATE_CACHE)?;
    Ok(())
}

fn counter_rate(previous: Option<f64>, current: Option<f64>, elapsed: f64) -> Option<f64> {
    let previous = previous?;
    let current = current?;
    (current >= previous).then_some((current - previous) / elapsed)
}

fn read_rate_cache() -> RateCache {
    let path = Path::new(RATE_CACHE);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return RateCache::default();
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return RateCache::default();
    }
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

async fn guest_config(device: &ChildDeviceReport) -> Option<Value> {
    let vmid = device.identity.key.parse::<u64>().ok()?;
    let node = device.node.as_deref()?;
    let kind = match device.identity.kind {
        ChildDeviceKind::ProxmoxVm => "qemu-server",
        ChildDeviceKind::ProxmoxLxc => "lxc",
        ChildDeviceKind::DockerContainer => return None,
    };
    let path = format!("/etc/pve/nodes/{node}/{kind}/{vmid}.conf");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > 256 * 1024 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut safe = serde_json::Map::new();
    for line in text.lines().take(2048) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.len() > 8192 {
            continue;
        }
        if matches!(key, "cores" | "memory") {
            if let Ok(number) = value.parse::<u64>() {
                safe.insert(key.to_owned(), Value::from(number));
            }
            continue;
        }
        if key == "tags"
            || key == "agent"
            || is_indexed_config_key(key, &["net", "ipconfig"])
            || is_disk_config_key(key)
        {
            safe.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
    Some(Value::Object(safe))
}

fn config_key(device: &ChildDeviceReport) -> Option<String> {
    let node = device.node.as_deref()?;
    let kind = match device.identity.kind {
        ChildDeviceKind::ProxmoxVm => "qemu",
        ChildDeviceKind::ProxmoxLxc => "lxc",
        ChildDeviceKind::DockerContainer => return None,
    };
    Some(format!(
        "{}:{kind}:{node}:{}",
        device.identity.scope, device.identity.key
    ))
}

fn read_config_cache() -> ConfigCache {
    let path = Path::new(CONFIG_CACHE);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return ConfigCache::default();
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return ConfigCache::default();
    }
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_config_cache(cache: &ConfigCache) -> Result<()> {
    let bytes = serde_json::to_vec(cache)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        bail!("Proxmox configuration cache exceeded the size limit")
    }
    let temporary = format!("{CONFIG_CACHE}.tmp-{}", std::process::id());
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, CONFIG_CACHE)?;
    Ok(())
}

fn enrich_from_config(device: &mut ChildDeviceReport, config: &Value) {
    if let Some(cores) = config
        .get("cores")
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0)
    {
        device.cpu_cores = Some(cores);
    }
    if let Some(memory) = config
        .get("memory")
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0)
    {
        device.memory_bytes = Some(memory * 1024.0 * 1024.0);
    }
    if let Some(tags) = config.get("tags").and_then(Value::as_str) {
        device.tags = tags
            .split(';')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .take(64)
            .map(str::to_owned)
            .collect();
    }
    let is_lxc = device.identity.kind == ChildDeviceKind::ProxmoxLxc;
    if device.identity.kind == ChildDeviceKind::ProxmoxVm {
        let enabled = config
            .get("agent")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "1" || value.starts_with("1,"));
        device.guest_agent_status = Some(if enabled {
            hope_protocol::GuestAgentStatus::Unavailable
        } else {
            hope_protocol::GuestAgentStatus::Disabled
        });
    }
    for (key, value) in config.as_object().into_iter().flatten() {
        if (key.starts_with("net") || key.starts_with("ipconfig"))
            && let Some(interface) =
                parse_interface(key, value.as_str().unwrap_or_default(), is_lxc)
        {
            if let Some(existing) = device
                .interfaces
                .iter_mut()
                .find(|item| item.name == interface.name)
            {
                existing.mac = interface.mac.or(existing.mac.take());
                existing.gateway = interface.gateway.or(existing.gateway.take());
                for address in interface.addresses {
                    if !existing.addresses.contains(&address) {
                        existing.addresses.push(address);
                    }
                }
            } else {
                device.interfaces.push(interface);
            }
        }
        if key.starts_with("net")
            && let Some(network) =
                parse_network_configuration(key, value.as_str().unwrap_or_default())
            && !device
                .network_configuration
                .iter()
                .any(|existing| existing.name == network.name)
        {
            device.network_configuration.push(network);
        }
        if is_disk_config_key(key)
            && let Some(disk) = parse_disk_configuration(key, value.as_str().unwrap_or_default())
        {
            device.disk_configuration.push(disk);
        }
    }
    device.interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    device
        .network_configuration
        .sort_by(|a, b| a.name.cmp(&b.name));
    device
        .disk_configuration
        .sort_by(|a, b| a.name.cmp(&b.name));
}

fn is_indexed_config_key(key: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        key.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

fn is_disk_config_key(key: &str) -> bool {
    key == "rootfs"
        || is_indexed_config_key(
            key,
            &[
                "scsi", "virtio", "sata", "ide", "efidisk", "tpmstate", "unused", "mp",
            ],
        )
}

fn parse_disk_configuration(
    key: &str,
    value: &str,
) -> Option<hope_protocol::GuestDiskConfiguration> {
    let mut storage = None;
    let mut volume = None;
    let mut size_bytes = None;
    let first = value.split(',').next()?.trim();
    if let Some((storage_id, volume_id)) = first.split_once(':')
        && safe_storage_id(storage_id)
        && !volume_id.is_empty()
        && volume_id.len() <= 255
    {
        storage = Some(storage_id.to_owned());
        volume = Some(volume_id.to_owned());
    }
    for part in value.split(',') {
        if let Some(size) = part.trim().strip_prefix("size=") {
            size_bytes = parse_pve_size(size);
        }
    }
    Some(hope_protocol::GuestDiskConfiguration {
        name: key.to_owned(),
        storage,
        volume,
        size_bytes,
    })
}

fn safe_storage_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn parse_pve_size(value: &str) -> Option<f64> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix(['K', 'k']) {
        (number, 1024.0)
    } else if let Some(number) = value.strip_suffix(['M', 'm']) {
        (number, 1024.0_f64.powi(2))
    } else if let Some(number) = value.strip_suffix(['G', 'g']) {
        (number, 1024.0_f64.powi(3))
    } else if let Some(number) = value.strip_suffix(['T', 't']) {
        (number, 1024.0_f64.powi(4))
    } else {
        return value.parse::<f64>().ok().filter(|number| *number >= 0.0);
    };
    number
        .parse::<f64>()
        .ok()
        .map(|number| number * multiplier)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn apply_cached_guest_data(device: &mut ChildDeviceReport, cache: Option<&GuestDataCache>) {
    let Some(cache) = cache else {
        return;
    };
    if let Some(status) = cache.status {
        device.guest_agent_status = Some(status);
    }
    device.guest_os = cache.os.clone();
    device.guest_filesystems = cache.filesystems.clone();
    for interface in cache.interfaces.iter().cloned() {
        merge_interface(&mut device.interfaces, interface);
    }
}

const SLOW_ENRICHMENT_REFRESH_SECONDS: f64 = 300.0;
const MAX_QGA_GUESTS_PER_RUN: usize = 1;
const MAX_LXC_INTERFACE_GUESTS_PER_RUN: usize = 2;

async fn refresh_guest_enrichment(
    devices: &mut [ChildDeviceReport],
    config_cache: &mut ConfigCache,
    now: f64,
) {
    for (kind, maximum) in [
        (ChildDeviceKind::ProxmoxVm, MAX_QGA_GUESTS_PER_RUN),
        (
            ChildDeviceKind::ProxmoxLxc,
            MAX_LXC_INTERFACE_GUESTS_PER_RUN,
        ),
    ] {
        let candidates = enrichment_candidates(devices, config_cache, kind, now, maximum);
        if candidates.is_empty() {
            continue;
        }
        let cursor_path = if kind == ChildDeviceKind::ProxmoxVm {
            "/run/hope-agent/proxmox-qga-cursor"
        } else {
            "/run/hope-agent/proxmox-lxc-interface-cursor"
        };
        let mut cursor = 0usize;
        for index in &candidates {
            if let Ok(value) = devices[*index].identity.key.parse::<usize>() {
                cursor = value.saturating_add(1);
            }
        }
        let temporary = format!("{cursor_path}.tmp-{}", std::process::id());
        if std::fs::write(&temporary, cursor.to_string()).is_ok() {
            let _ = std::fs::rename(temporary, cursor_path);
        }
        for index in candidates {
            let device = &devices[index];
            let Some(key) = config_key(device) else {
                continue;
            };
            let guest_data = if kind == ChildDeviceKind::ProxmoxVm {
                collect_guest_agent_data(device, now).await
            } else {
                collect_lxc_interfaces(device, now).await
            };
            let cache = guest_data.unwrap_or_else(|| GuestDataCache {
                observed_at: now,
                status: (kind == ChildDeviceKind::ProxmoxVm)
                    .then_some(hope_protocol::GuestAgentStatus::Unavailable),
                os: None,
                filesystems: vec![],
                interfaces: vec![],
            });
            if let Some(entry) = config_cache.entries.get_mut(&key) {
                entry.guest_data = Some(cache.clone());
            }
            apply_cached_guest_data(&mut devices[index], Some(&cache));
        }
    }
}

fn enrichment_candidates(
    devices: &[ChildDeviceReport],
    cache: &ConfigCache,
    kind: ChildDeviceKind,
    now: f64,
    maximum: usize,
) -> Vec<usize> {
    if devices.is_empty() {
        return vec![];
    }
    let cursor_path = if kind == ChildDeviceKind::ProxmoxVm {
        "/run/hope-agent/proxmox-qga-cursor"
    } else {
        "/run/hope-agent/proxmox-lxc-interface-cursor"
    };
    let cursor = std::fs::read_to_string(cursor_path)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_default() as usize;
    let start = devices
        .iter()
        .position(|device| {
            device
                .identity
                .key
                .parse::<usize>()
                .is_ok_and(|id| id >= cursor)
        })
        .unwrap_or(0);
    (0..devices.len())
        .map(|offset| (start + offset) % devices.len())
        .filter(|index| {
            let device = &devices[*index];
            if device.identity.kind != kind || device.lifecycle != DeviceLifecycle::Running {
                return false;
            }
            if kind == ChildDeviceKind::ProxmoxVm
                && device.guest_agent_status != Some(hope_protocol::GuestAgentStatus::Unavailable)
            {
                return false;
            }
            let Some(key) = config_key(device) else {
                return false;
            };
            cache
                .entries
                .get(&key)
                .and_then(|entry| entry.guest_data.as_ref())
                .is_none_or(|data| now - data.observed_at >= SLOW_ENRICHMENT_REFRESH_SECONDS)
        })
        .take(maximum)
        .collect()
}

async fn collect_guest_agent_data(device: &ChildDeviceReport, now: f64) -> Option<GuestDataCache> {
    let node = device.node.as_deref()?;
    let vmid = device.identity.key.parse::<u64>().ok()?;
    let base = format!("/nodes/{node}/qemu/{vmid}/agent");
    let os_path = format!("{base}/get-osinfo");
    let hostname_path = format!("{base}/get-host-name");
    let filesystems_path = format!("{base}/get-fsinfo");
    let network_path = format!("{base}/network-get-interfaces");
    let (os, hostname, filesystems, network) = tokio::join!(
        pvesh_get(&os_path, Duration::from_secs(2)),
        pvesh_get(&hostname_path, Duration::from_secs(2)),
        pvesh_get(&filesystems_path, Duration::from_secs(2)),
        pvesh_get(&network_path, Duration::from_secs(2)),
    );
    let os = os.ok().and_then(|value| parse_guest_os(&value, now));
    let hostname = hostname.ok().and_then(|value| {
        let result = value.get("result").unwrap_or(&value);
        result
            .get("host-name")
            .or_else(|| result.get("hostname"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 255)
            .map(str::to_owned)
    });
    let filesystems = filesystems
        .ok()
        .map(|value| parse_guest_filesystems(&value, now))
        .unwrap_or_default();
    let interfaces = network
        .ok()
        .map(|value| parse_guest_network_interfaces(&value))
        .unwrap_or_default();
    let mut os = os;
    if let Some(os) = &mut os {
        os.hostname = hostname.clone().or_else(|| os.hostname.take());
    } else if hostname.is_some() {
        os = Some(hope_protocol::GuestOsMetadata {
            hostname,
            name: None,
            kernel_release: None,
            observed_at: now,
        });
    }
    let available = os.is_some() || !filesystems.is_empty() || !interfaces.is_empty();
    Some(GuestDataCache {
        observed_at: now,
        status: Some(if available {
            hope_protocol::GuestAgentStatus::Available
        } else {
            hope_protocol::GuestAgentStatus::Unavailable
        }),
        os,
        filesystems,
        interfaces,
    })
}

async fn collect_lxc_interfaces(device: &ChildDeviceReport, now: f64) -> Option<GuestDataCache> {
    let node = device.node.as_deref()?;
    let vmid = device.identity.key.parse::<u64>().ok()?;
    let value = pvesh_get(
        &format!("/nodes/{node}/lxc/{vmid}/interfaces"),
        Duration::from_secs(2),
    )
    .await
    .ok()?;
    Some(GuestDataCache {
        observed_at: now,
        status: None,
        os: None,
        filesystems: vec![],
        interfaces: parse_guest_network_interfaces(&value),
    })
}

async fn pvesh_get(path: &str, deadline: Duration) -> Result<Value> {
    pvesh_timeout(&["get", path, "--output-format", "json"], deadline).await
}

fn parse_guest_os(value: &Value, observed_at: f64) -> Option<hope_protocol::GuestOsMetadata> {
    let value = value.get("result").unwrap_or(value);
    let field = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| value.get(*name).and_then(Value::as_str))
            .filter(|value| !value.is_empty() && value.len() <= 255)
            .map(str::to_owned)
    };
    let metadata = hope_protocol::GuestOsMetadata {
        hostname: field(&["host-name", "hostname"]),
        name: field(&["pretty-name", "name", "id"]),
        kernel_release: field(&["kernel-release", "kernel_release"]),
        observed_at,
    };
    (metadata.hostname.is_some() || metadata.name.is_some() || metadata.kernel_release.is_some())
        .then_some(metadata)
}

fn parse_guest_filesystems(value: &Value, observed_at: f64) -> Vec<hope_protocol::GuestFilesystem> {
    let Some(items) = value
        .get("result")
        .or(Some(value))
        .and_then(Value::as_array)
    else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let mountpoint = item
                .get("mountpoint")
                .or_else(|| item.get("mount-point"))
                .and_then(Value::as_str)?;
            let total_bytes = first_number(item, &["total-bytes", "total_bytes"])?;
            let used_bytes = first_number(item, &["used-bytes", "used_bytes"])?;
            (total_bytes.is_finite() && used_bytes.is_finite() && used_bytes <= total_bytes)
                .then_some(hope_protocol::GuestFilesystem {
                    mountpoint: mountpoint.to_owned(),
                    filesystem_type: item
                        .get("type")
                        .or_else(|| item.get("filesystem-type"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    total_bytes,
                    used_bytes,
                    observed_at,
                })
        })
        .take(128)
        .collect()
}

fn parse_guest_network_interfaces(value: &Value) -> Vec<Interface> {
    let Some(items) = value
        .get("result")
        .or(Some(value))
        .and_then(Value::as_array)
    else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_owned();
            let mac = item
                .get("hardware-address")
                .or_else(|| item.get("hwaddr"))
                .and_then(Value::as_str)
                .filter(|value| valid_mac(value))
                .map(str::to_ascii_lowercase);
            let mut addresses = vec![];
            if let Some(addrs) = item
                .get("ip-addresses")
                .or_else(|| item.get("ip_addresses"))
                .and_then(Value::as_array)
            {
                for address in addrs {
                    if let Some(address) = address
                        .get("ip-address")
                        .or_else(|| address.get("inet"))
                        .and_then(Value::as_str)
                        .filter(|value| value.parse::<std::net::IpAddr>().is_ok())
                    {
                        addresses.push(address.to_owned());
                    }
                }
            }
            for field in ["inet", "inet6"] {
                if let Some(address) = item
                    .get(field)
                    .and_then(Value::as_str)
                    .and_then(|value| value.split('/').next())
                    .filter(|value| value.parse::<std::net::IpAddr>().is_ok())
                    && !addresses.contains(&address.to_owned())
                {
                    addresses.push(address.to_owned());
                }
            }
            Some(Interface {
                name,
                mac,
                addresses,
                gateway: None,
            })
        })
        .take(32)
        .collect()
}

fn merge_interface(interfaces: &mut Vec<Interface>, incoming: Interface) {
    if let Some(existing) = interfaces
        .iter_mut()
        .find(|item| item.name == incoming.name)
    {
        existing.mac = incoming.mac.or(existing.mac.take());
        existing.gateway = incoming.gateway.or(existing.gateway.take());
        for address in incoming.addresses {
            if !existing.addresses.contains(&address) {
                existing.addresses.push(address);
            }
        }
    } else {
        interfaces.push(incoming);
    }
}

fn parse_network_configuration(
    key: &str,
    value: &str,
) -> Option<hope_protocol::GuestNetworkConfiguration> {
    let index = key.strip_prefix("net")?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parts = value
        .split(',')
        .filter_map(|part| part.split_once('='))
        .collect::<HashMap<_, _>>();
    let model = ["virtio", "e1000", "rtl8139", "vmxnet3"]
        .into_iter()
        .find(|key| parts.contains_key(key));
    let mac = model
        .and_then(|model| parts.get(model).copied())
        .filter(|value| valid_mac(value))
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            parts
                .get("hwaddr")
                .copied()
                .filter(|value| valid_mac(value))
                .map(str::to_ascii_lowercase)
        });
    let vlan_tag = parts
        .get("tag")
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|tag| (1..=4094).contains(tag));
    Some(hope_protocol::GuestNetworkConfiguration {
        name: parts
            .get("name")
            .copied()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("net{index}")),
        bridge: parts.get("bridge").copied().map(str::to_owned),
        vlan_tag,
        model: model.map(str::to_owned),
        mac,
    })
}

fn parse_interface(key: &str, value: &str, is_lxc: bool) -> Option<Interface> {
    let index = key.chars().filter(char::is_ascii_digit).collect::<String>();
    let name = if is_lxc && key.starts_with("net") {
        value
            .split(',')
            .find_map(|part| part.strip_prefix("name="))?
            .to_owned()
    } else {
        format!("net{index}")
    };
    let mut interface = Interface {
        name,
        mac: None,
        addresses: vec![],
        gateway: None,
    };
    for part in value.split(',') {
        if let Some((key, value)) = part.split_once('=')
            && (key == "hwaddr"
                || key == "virtio"
                || key == "e1000"
                || key == "rtl8139"
                || key == "vmxnet3")
            && valid_mac(value)
        {
            interface.mac = Some(value.to_ascii_lowercase());
        }
        if let Some(value) = part
            .strip_prefix("ip=")
            .or_else(|| part.strip_prefix("ip6="))
            && let Some(address) = value
                .split('/')
                .next()
                .filter(|value| *value != "dhcp" && *value != "auto" && !value.is_empty())
            && address.parse::<std::net::IpAddr>().is_ok()
        {
            interface.addresses.push(address.to_owned());
        }
        if let Some(gateway) = part
            .strip_prefix("gw=")
            .or_else(|| part.strip_prefix("gw6="))
            .filter(|gateway| gateway.parse::<std::net::IpAddr>().is_ok())
        {
            interface.gateway = Some(gateway.to_owned());
        }
    }
    Some(interface)
}

fn valid_mac(value: &str) -> bool {
    let parts = value.split(':').collect::<Vec<_>>();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && u8::from_str_radix(part, 16).is_ok())
}

fn safe_node(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
    .then(|| value.to_owned())
}

fn cluster_identity() -> Result<(String, String)> {
    let _lock = acquire_shared_lock()?;
    std::fs::create_dir_all(SHARED_STATE).context("create private shared pmxcfs state")?;
    let cluster_id = match std::fs::read_to_string(SHARED_CLUSTER_ID) {
        Ok(value) => {
            let id = value.trim();
            Uuid::parse_str(id).context("invalid shared Proxmox cluster identity")?;
            id.to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = Uuid::new_v4().to_string();
            write_shared_text(SHARED_CLUSTER_ID, &id)?;
            id
        }
        Err(error) => return Err(error).context("read shared Proxmox cluster identity"),
    };
    let source_token = match std::fs::read_to_string(SHARED_SOURCE_TOKEN) {
        Ok(value) => {
            let token = value.trim();
            if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("invalid shared Proxmox source token")
            }
            token.to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = Uuid::new_v4().simple().to_string() + &Uuid::new_v4().simple().to_string();
            write_shared_text(SHARED_SOURCE_TOKEN, &token)?;
            token
        }
        Err(error) => return Err(error).context("read shared Proxmox source token"),
    };
    Ok((cluster_id, source_token))
}

struct SharedLock;

impl Drop for SharedLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(SHARED_LOCK);
    }
}

fn acquire_shared_lock() -> Result<SharedLock> {
    use std::time::Instant;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::create_dir(SHARED_LOCK) {
            Ok(()) => return Ok(SharedLock),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // pmxcfs documents mkdir plus utime(0, 0) as its cluster-wide lock protocol.
                // This is deliberately not a local flock: agents on other PVE nodes share it.
                #[cfg(unix)]
                {
                    use std::ffi::CString;
                    let path = CString::new(SHARED_LOCK).expect("static lock path");
                    let times = libc::utimbuf {
                        actime: 0,
                        modtime: 0,
                    };
                    // SAFETY: path is NUL-terminated and times points to a valid utimbuf.
                    unsafe { libc::utime(path.as_ptr(), &times) };
                }
                if Instant::now() >= deadline {
                    bail!("timed out acquiring the shared Proxmox pmxcfs lock")
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error).context("acquire shared Proxmox pmxcfs lock"),
        }
    }
}

fn read_shared<T: serde::de::DeserializeOwned>(path: &str) -> Result<Option<T>> {
    read_json_file(path, MAX_PVE_OUTPUT)
}

fn read_private<T: serde::de::DeserializeOwned>(path: &str) -> Option<T> {
    read_json_file(path, MAX_PVE_OUTPUT).ok().flatten()
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &str, limit: usize) -> Result<Option<T>> {
    let path = Path::new(path);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || metadata.len() as usize > limit {
        bail!("shared Proxmox state has an unexpected file type or size")
    }
    let bytes = std::fs::read(path)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn write_shared<T: serde::Serialize>(path: &str, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write_shared_bytes(path, &bytes)
}

fn write_shared_text(path: &str, value: &str) -> Result<()> {
    write_shared_bytes(path, value.as_bytes())
}

fn write_shared_bytes(path: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_PVE_OUTPUT {
        bail!("shared Proxmox state exceeded its size limit")
    }
    let path = Path::new(path);
    if std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| !metadata.file_type().is_file())
    {
        bail!("shared Proxmox state path is not a regular file")
    }
    // pmxcfs O_EXCL/O_TRUNC creation is not atomic. Write a private temporary file while holding
    // the documented cluster lock, then rename the completed file into place.
    use std::io::Write;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).context("publish shared Proxmox state")?;
    Ok(())
}

fn write_snapshot(snapshot: &SnapshotFile) -> Result<()> {
    let directory = Path::new(SNAPSHOT).parent().context("snapshot directory")?;
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o750))?;
        set_collector_group(directory)?;
    }
    let temporary = format!("{SNAPSHOT}.tmp-{}", std::process::id());
    std::fs::write(&temporary, serde_json::to_vec(snapshot)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o640))?;
        set_collector_group(Path::new(&temporary))?;
    }
    std::fs::rename(temporary, SNAPSHOT)?;
    Ok(())
}

#[cfg(unix)]
fn set_collector_group(path: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let gid = std::fs::read_to_string("/etc/group")?
        .lines()
        .find(|line| line.starts_with("hope-agent:"))
        .and_then(|line| line.split(':').nth(2)?.parse::<libc::gid_t>().ok())
        .context("dedicated hope-agent group is missing")?;
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: path is a valid NUL-terminated path and the collector service runs as root.
    if unsafe { libc::chown(path.as_ptr(), 0, gid) } != 0 {
        return Err(std::io::Error::last_os_error()).context("set Proxmox snapshot group");
    }
    Ok(())
}

async fn pvesh(args: &[&str]) -> Result<Value> {
    pvesh_timeout(args, Duration::from_secs(6)).await
}

async fn pvesh_timeout(args: &[&str], deadline: Duration) -> Result<Value> {
    let binary = pvesh_binary().context("pvesh binary was not found in a fixed system path")?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        // Keep pvesh's own actionable errors in the collector service journal. The argument
        // set is fixed by this module and contains no credentials.
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("start fixed pvesh operation")?;
    let pid = child.id();
    let output = timeout(deadline, async {
        let mut bytes = Vec::new();
        let Some(stdout) = child.stdout.take() else {
            bail!("pvesh stdout unavailable")
        };
        stdout
            .take((MAX_PVE_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > MAX_PVE_OUTPUT {
            bail!("pvesh response exceeded the size limit")
        }
        let status = child.wait().await?;
        if !status.success() {
            bail!("pvesh exited with {status}")
        }
        serde_json::from_slice(&bytes).context("pvesh returned invalid JSON")
    })
    .await;
    match output {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            terminate_group(pid);
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            terminate_group(pid);
            let _ = child.wait().await;
            bail!("pvesh operation timed out")
        }
    }
}

fn pvesh_binary() -> Option<&'static str> {
    ["/usr/bin/pvesh", "/usr/sbin/pvesh"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

#[cfg(unix)]
fn terminate_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: the subprocess is spawned in a process group whose ID is its process ID.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn terminate_group(_pid: Option<u32>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pve_inventory_keeps_vm_and_lxc_measurements_and_configured_addresses_distinct() {
        let cluster = Uuid::new_v4().to_string();
        let vm = resource_device(
            &serde_json::json!({"type":"qemu","vmid":201,"name":"media","node":"pve-a","status":"running","cpu":0.25,"maxcpu":4,"mem":512,"maxmem":4096,"disk":1024,"maxdisk":8192,"netin":10,"netout":20,"tags":"infra;media"}),
            &cluster,
        )
        .unwrap();
        assert_eq!(vm.identity.kind, ChildDeviceKind::ProxmoxVm);
        assert_eq!(vm.metrics.cpu_percent, Some(25.0));
        assert_eq!(vm.metrics.cpu_capacity_cores, Some(4.0));
        assert_eq!(
            vm.metrics.memory_measurement,
            Some(MemoryMeasurement::HypervisorGuest)
        );

        let mut lxc = resource_device(
            &serde_json::json!({"type":"lxc","vmid":202,"node":"pve-a","status":"stopped","maxcpu":2,"maxmem":1024}),
            &cluster,
        )
        .unwrap();
        enrich_from_config(
            &mut lxc,
            &serde_json::json!({"cores":2,"memory":2048,"rootfs":"local-lvm:vm-202-disk-0,size=8G","net0":"name=eth0,bridge=vmbr0,hwaddr=02:00:00:00:00:02,ip=192.168.1.22/24,ip6=2001:db8::22/64,gw=192.168.1.1"}),
        );
        assert_eq!(lxc.lifecycle, DeviceLifecycle::Stopped);
        assert_eq!(
            lxc.metrics.cpu_percent, None,
            "stopped guests do not report zero usage"
        );
        assert_eq!(lxc.memory_bytes, Some(2048.0 * 1024.0 * 1024.0));
        assert_eq!(lxc.interfaces.len(), 1);
        assert_eq!(lxc.interfaces[0].mac.as_deref(), Some("02:00:00:00:00:02"));
        assert_eq!(
            lxc.interfaces[0].addresses,
            ["192.168.1.22", "2001:db8::22"]
        );
        let nic = &lxc.network_configuration[0];
        assert_eq!(nic.name, "eth0");
        assert_eq!(nic.bridge.as_deref(), Some("vmbr0"));
        assert_eq!(nic.mac.as_deref(), Some("02:00:00:00:00:02"));
        assert_eq!(lxc.disk_configuration[0].name, "rootfs");
        assert_eq!(
            lxc.disk_configuration[0].size_bytes,
            Some(8.0 * 1024.0 * 1024.0 * 1024.0)
        );
    }

    #[test]
    fn guests_with_unknown_states_remain_visible_without_authoritative_collection() {
        let cluster = Uuid::new_v4().to_string();
        let unknown = serde_json::json!({"type":"lxc","vmid":10,"status":"migrating","node":"pve"});
        let device = resource_device(&unknown, &cluster).unwrap();
        assert_eq!(device.identity.kind, ChildDeviceKind::ProxmoxLxc);
        assert_eq!(device.lifecycle, DeviceLifecycle::Stale);
        assert_eq!(device.status.as_deref(), Some("migrating"));
        let missing_status = serde_json::json!({"type":"lxc","vmid":11,"node":"pve"});
        let device = resource_device(&missing_status, &cluster).unwrap();
        assert_eq!(device.lifecycle, DeviceLifecycle::Stale);
        assert_eq!(device.status.as_deref(), Some("unknown"));
        assert!(safe_node("../../etc").is_none());
        assert!(safe_node("pve-1.local").is_some());
    }

    #[test]
    fn cluster_inventory_is_scoped_to_the_local_node() {
        let scope = Uuid::new_v4().to_string();
        let local = resource_device(
            &serde_json::json!({"type":"lxc","vmid":101,"node":"catacomb3","status":"running"}),
            &scope,
        )
        .unwrap();
        let remote = resource_device(
            &serde_json::json!({"type":"lxc","vmid":102,"node":"catacomb","status":"running"}),
            &scope,
        )
        .unwrap();
        assert!(is_local_guest(&local, "catacomb3"));
        assert!(!is_local_guest(&remote, "catacomb3"));
    }

    #[test]
    fn guest_incarnations_survive_migrations_and_change_after_confirmed_absence() {
        let scope = Uuid::new_v4().to_string();
        let mut map = GuestIdentityMap {
            source_scope: scope.clone(),
            ..Default::default()
        };
        let mut device = resource_device(
            &serde_json::json!({"type":"qemu","vmid":100,"status":"running","node":"pve-a"}),
            &scope,
        )
        .unwrap();
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut device),
            &scope,
            true,
            1000.0,
            None,
        );
        let original = device.identity.incarnation.clone();
        device.node = Some("pve-b".into());
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut device),
            &scope,
            false,
            1030.0,
            None,
        );
        assert_eq!(device.identity.incarnation, original);

        assign_guest_identities(&mut map, &mut [], &scope, false, 1050.0, None);
        assert_eq!(map.entries.values().next().unwrap().missing_count, 0);
        for at in [1060.0, 1120.0, 1180.0, 1240.0, 1300.0, 1360.0] {
            assign_guest_identities(&mut map, &mut [], &scope, true, at, None);
        }
        assert!(map.entries.values().next().unwrap().missing_count >= 3);
        let mut recreated = resource_device(
            &serde_json::json!({"type":"qemu","vmid":100,"status":"running","node":"pve-b"}),
            &scope,
        )
        .unwrap();
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut recreated),
            &scope,
            false,
            1361.0,
            None,
        );
        assert_ne!(recreated.identity.incarnation, original);
    }

    #[test]
    fn ownership_generation_only_advances_when_the_observed_owner_changes() {
        let scope = Uuid::new_v4().to_string();
        let mut map = GuestIdentityMap {
            source_scope: scope.clone(),
            ..Default::default()
        };
        let mut guest = resource_device(
            &serde_json::json!({"type":"qemu","vmid":100,"status":"running","node":"pve-a"}),
            &scope,
        )
        .unwrap();
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut guest),
            &scope,
            false,
            1_000.0,
            None,
        );
        assert_eq!(guest.ownership_generation, Some(1));
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut guest),
            &scope,
            false,
            1_030.0,
            None,
        );
        assert_eq!(guest.ownership_generation, Some(1));
        guest.node = Some("pve-b".into());
        assign_guest_identities(
            &mut map,
            std::slice::from_mut(&mut guest),
            &scope,
            false,
            1_060.0,
            None,
        );
        assert_eq!(guest.ownership_generation, Some(2));
    }

    #[test]
    fn shared_presence_prevents_a_node_local_absence_from_aging_an_identity() {
        let scope = Uuid::new_v4().to_string();
        let key = "ProxmoxVm:100".to_owned();
        let mut map = GuestIdentityMap {
            source_scope: scope.clone(),
            ..Default::default()
        };
        map.entries.insert(
            key.clone(),
            GuestIdentity {
                incarnation: Uuid::new_v4().to_string(),
                owner_node: Some("pve-b".into()),
                ownership_generation: 4,
                missing_count: 0,
                missing_since: None,
            },
        );
        let globally_present = HashSet::from([key]);
        for at in [1_000.0, 1_060.0, 1_120.0, 1_180.0] {
            assign_guest_identities(&mut map, &mut [], &scope, true, at, Some(&globally_present));
        }
        let entry = map.entries.values().next().unwrap();
        assert_eq!(entry.missing_count, 0);
        assert_eq!(entry.missing_since, None);
        assert_eq!(entry.ownership_generation, 4);
    }

    #[test]
    fn qga_fixtures_keep_enrichment_times_and_missing_filesystems_absent() {
        let os = parse_guest_os(&serde_json::json!({"result":{"pretty-name":"Debian GNU/Linux","kernel-release":"6.1"}}), 1234.5).unwrap();
        assert_eq!(os.name.as_deref(), Some("Debian GNU/Linux"));
        assert_eq!(os.observed_at, 1234.5);
        let filesystems = parse_guest_filesystems(
            &serde_json::json!({"result":[
                {"mountpoint":"/","type":"ext4","total-bytes":1000,"used-bytes":600},
                {"mountpoint":"/bad","total-bytes":100,"used-bytes":200}
            ]}),
            1235.0,
        );
        assert_eq!(filesystems.len(), 1);
        assert_eq!(filesystems[0].filesystem_type.as_deref(), Some("ext4"));
        assert_eq!(filesystems[0].observed_at, 1235.0);
        assert!(
            parse_guest_filesystems(&serde_json::json!({"error":"not supported"}), 1236.0)
                .is_empty()
        );
    }
}
