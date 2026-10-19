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
    collections::HashMap,
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;

const SNAPSHOT: &str = "/run/hope-agent/proxmox.json";
const CLUSTER_ID: &str = "/etc/hope-agent/proxmox-cluster-id";
const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PVE_OUTPUT: usize = 8 * 1024 * 1024;
const CONFIG_CACHE: &str = "/run/hope-agent/proxmox-config.json";
const CONFIG_REFRESH_SECONDS: u64 = 300;

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    source_scope: String,
    devices: Vec<ChildDeviceReport>,
    inventory_complete: bool,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ConfigCache {
    entries: HashMap<String, CachedConfig>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CachedConfig {
    observed_at: u64,
    value: Value,
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
    let fresh = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age <= Duration::from_secs(90));
    Some(ChildSnapshot {
        source: ChildSourceKind::Proxmox,
        source_scope: file.source_scope,
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
    let cluster_id = cluster_id()?;
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
                devices: vec![],
                inventory_complete: false,
            })?;
            return Err(error).context("read Proxmox cluster resources");
        }
    };
    let Some(resources) = resources.as_array() else {
        write_snapshot(&SnapshotFile {
            source_scope: cluster_id,
            devices: vec![],
            inventory_complete: false,
        })?;
        bail!("Proxmox resources must be a JSON array");
    };
    if resources.len() > 500 {
        complete = false;
    }
    let local_node = std::fs::read_to_string("/etc/hostname")
        .ok()
        .and_then(|name| safe_node(name.trim()));
    let mut parsed = Vec::with_capacity(resources.len().min(500));
    for resource in resources.iter().take(500) {
        let Some(device) = resource_device(resource, &cluster_id) else {
            complete = false;
            continue;
        };
        parsed.push(device);
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
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
        let local_node = local_node.clone();
        let cached = config_key(&device).and_then(|key| cached_configs.get(&key).cloned());
        async move {
            if local_node.as_deref() == device.node.as_deref() {
                let key = config_key(&device);
                let should_refresh = cached.as_ref().is_none_or(|entry| {
                    now.saturating_sub(entry.observed_at) >= CONFIG_REFRESH_SECONDS
                });
                let config = if should_refresh {
                    match guest_config(&device).await {
                        Some(value) => Some(CachedConfig {
                            observed_at: now,
                            value,
                        }),
                        None => cached,
                    }
                } else {
                    cached
                };
                if let Some(config) = config {
                    enrich_from_config(&mut device, &config.value);
                    return (device, key, Some(config));
                }
            }
            (device, None, None)
        }
    }))
    .buffer_unordered(8);
    let mut devices = vec![];
    while let Some((device, key, config)) = tasks.next().await {
        if let (Some(key), Some(config)) = (key, config) {
            config_cache.entries.insert(key, config);
        }
        devices.push(device);
    }
    drop(tasks);
    write_config_cache(&config_cache)?;
    devices.sort_by(|a, b| a.identity.key.cmp(&b.identity.key));
    write_snapshot(&SnapshotFile {
        source_scope: cluster_id,
        devices,
        inventory_complete: complete,
    })
}

fn resource_device(resource: &Value, cluster_id: &str) -> Option<ChildDeviceReport> {
    let kind = match resource.get("type")?.as_str()? {
        "qemu" => ChildDeviceKind::ProxmoxVm,
        "lxc" => ChildDeviceKind::ProxmoxLxc,
        _ => return None,
    };
    let vmid = resource.get("vmid")?.as_u64()?;
    let key = vmid.to_string();
    let state = resource.get("status")?.as_str()?;
    let lifecycle = match state {
        "running" => DeviceLifecycle::Running,
        "stopped" | "paused" => DeviceLifecycle::Stopped,
        _ => return None,
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
            // PVE's VMID is stable through migrations. A guest-specific incarnation is supplied
            // by `vmgenid` when available in configuration; VMID-only systems retain a safe ID.
            incarnation: format!("vmid-{vmid}"),
        },
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
        metrics,
    })
}

async fn guest_config(device: &ChildDeviceReport) -> Option<Value> {
    let vmid = device.identity.key.parse::<u64>().ok()?;
    let node = device.node.as_deref()?;
    let kind = match device.identity.kind {
        ChildDeviceKind::ProxmoxVm => "qemu",
        ChildDeviceKind::ProxmoxLxc => "lxc",
        ChildDeviceKind::DockerContainer => return None,
    };
    let config = pvesh_timeout(
        &[
            "get",
            &format!("/nodes/{node}/{kind}/{vmid}/config"),
            "--output-format",
            "json",
        ],
        Duration::from_secs(3),
    )
    .await
    .ok()?;
    let safe = config
        .as_object()?
        .iter()
        .filter(|(key, value)| {
            if matches!(key.as_str(), "cores" | "memory") {
                return true;
            }
            let max = if *key == "tags" { 8192 } else { 2048 };
            value.as_str().is_some_and(|value| value.len() <= max)
        })
        .filter(|(key, _)| {
            matches!(key.as_str(), "cores" | "memory" | "tags")
                || ["net", "ipconfig"].into_iter().any(|prefix| {
                    key.strip_prefix(prefix).is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                    })
                })
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
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
    }
    device.interfaces.sort_by(|a, b| a.name.cmp(&b.name));
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

fn cluster_id() -> Result<String> {
    if let Ok(value) = std::fs::read_to_string(CLUSTER_ID) {
        let id = value.trim();
        Uuid::parse_str(id).context("invalid saved Proxmox cluster identity")?;
        return Ok(id.to_owned());
    }
    let id = Uuid::new_v4().to_string();
    std::fs::create_dir_all("/etc/hope-agent")?;
    let temporary = format!("{CLUSTER_ID}.tmp-{}", std::process::id());
    std::fs::write(&temporary, format!("{id}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o640))?;
    }
    std::fs::rename(temporary, CLUSTER_ID)?;
    Ok(id)
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
    pvesh_timeout(args, Duration::from_secs(10)).await
}

async fn pvesh_timeout(args: &[&str], deadline: Duration) -> Result<Value> {
    let binary = pvesh_binary().context("pvesh binary was not found in a fixed system path")?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
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
            &serde_json::json!({"cores":2,"memory":2048,"net0":"name=eth0,bridge=vmbr0,hwaddr=02:00:00:00:00:02,ip=192.168.1.22/24,ip6=2001:db8::22/64,gw=192.168.1.1"}),
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
    }

    #[test]
    fn unknown_guest_states_and_untrusted_node_names_are_not_guessed() {
        let cluster = Uuid::new_v4().to_string();
        let unknown = serde_json::json!({"type":"lxc","vmid":10,"status":"migrating","node":"pve"});
        assert!(resource_device(&unknown, &cluster).is_none());
        assert!(safe_node("../../etc").is_none());
        assert!(safe_node("pve-1.local").is_some());
    }
}
