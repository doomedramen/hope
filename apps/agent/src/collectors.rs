//! Bounded, read-only Linux inventory collectors.
//!
//! Collectors use proc/sysfs and direct subprocess calls. No command is
//! executed through a shell. Missing files, permissions, and missing tools
//! become an unavailable or partial collector result instead of aborting the
//! whole snapshot.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
#[cfg(target_os = "linux")]
use std::fs;
use std::fs::File;
use std::io::Read;
use std::net::Ipv4Addr;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::{
    Capability, CollectorSnapshot, CollectorStatus, InventorySnapshot, M5_SCHEMA_VERSION,
    MAX_COLLECTOR_PAYLOAD_BYTES, MAX_COLLECTORS, MAX_OBSERVATIONS, MAX_SNAPSHOT_BYTES, Observation,
    ObservationBatch, ObservationState, default_agent_capabilities,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

pub const MAX_ITEMS: usize = 128;
pub const MAX_TEXT_BYTES: usize = 512;
pub const MAX_FILE_BYTES: usize = 128 * 1024;
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 32 * 1024;
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    value.chars().take(max_bytes).collect()
}

fn read_bounded(path: impl AsRef<Path>, max_bytes: usize) -> Result<String, String> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!("{} exceeds {max_bytes} bytes", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_trimmed(path: impl AsRef<Path>, max_bytes: usize) -> Result<String, String> {
    Ok(bounded_text(
        read_bounded(path, max_bytes)?.trim(),
        max_bytes,
    ))
}

fn unavailable(capability: Capability, error: impl Into<String>) -> CollectorSnapshot {
    CollectorSnapshot {
        capability,
        status: CollectorStatus::Unavailable,
        payload: None,
        error: Some(bounded_text(&error.into(), MAX_TEXT_BYTES)),
    }
}

fn available(capability: Capability, payload: Value) -> CollectorSnapshot {
    let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
    if payload_bytes.len() > MAX_COLLECTOR_PAYLOAD_BYTES {
        return CollectorSnapshot {
            capability,
            status: CollectorStatus::Partial,
            payload: Some(json!({"truncated": true})),
            error: Some(format!(
                "collector payload exceeded {MAX_COLLECTOR_PAYLOAD_BYTES} bytes"
            )),
        };
    }
    CollectorSnapshot {
        capability,
        status: CollectorStatus::Available,
        payload: Some(payload),
        error: None,
    }
}

fn partial(capability: Capability, payload: Value, warnings: &[String]) -> CollectorSnapshot {
    let mut result = available(capability, payload);
    if !warnings.is_empty() {
        result.status = CollectorStatus::Partial;
        result.error = Some(bounded_text(&warnings.join("; "), MAX_TEXT_BYTES));
    }
    result
}

fn result_from(capability: Capability, result: Result<Value, String>) -> CollectorSnapshot {
    match result {
        Ok(payload) => available(capability, payload),
        Err(error) => unavailable(capability, error),
    }
}

/// Return compiled M5 capabilities. Runtime-unavailable collectors remain
/// visible in the snapshot with an explicit status.
pub fn capabilities() -> Vec<Capability> {
    default_agent_capabilities()
}

/// Collect all M5 sections. Each section has an independent failure boundary.
pub async fn collect_snapshot(agent_id: Uuid) -> InventorySnapshot {
    let mut collectors = Vec::with_capacity(MAX_COLLECTORS);
    collectors.push(result_from(Capability::Host, collect_host()));
    collectors.push(collect_network().await.map_or_else(
        |error| unavailable(Capability::Network, error),
        |(payload, warnings)| {
            if warnings.is_empty() {
                available(Capability::Network, payload)
            } else {
                partial(Capability::Network, payload, &warnings)
            }
        },
    ));
    collectors.push(collect_filesystems().await.map_or_else(
        |error| unavailable(Capability::Filesystem, error),
        |(payload, warnings)| {
            if warnings.is_empty() {
                available(Capability::Filesystem, payload)
            } else {
                partial(Capability::Filesystem, payload, &warnings)
            }
        },
    ));
    collectors.push(result_from(Capability::Systemd, collect_systemd().await));
    collectors.push(result_from(Capability::Packages, collect_packages().await));
    collectors.push(result_from(Capability::Processes, collect_processes()));
    collectors.push(result_from(Capability::Sockets, collect_sockets()));
    collectors.push(result_from(Capability::Docker, collect_docker().await));

    let complete = collectors
        .iter()
        .all(|collector| collector.status == CollectorStatus::Available);
    let mut inventory = Map::new();
    inventory.insert("schema_version".into(), json!(M5_SCHEMA_VERSION));
    for collector in &collectors {
        let key = collector.capability.as_str().to_string();
        inventory.insert(
            key,
            collector
                .payload
                .clone()
                .unwrap_or_else(|| json!({"available": false})),
        );
    }
    inventory.insert(
        "collector_status".into(),
        Value::Array(
            collectors
                .iter()
                .map(|collector| {
                    json!({
                        "capability": collector.capability,
                        "status": collector.status,
                        "error": collector.error,
                    })
                })
                .collect(),
        ),
    );

    let mut snapshot = InventorySnapshot {
        agent_id,
        sequence: 0,
        collected_at_unix_secs: now_unix_secs(),
        inventory: Value::Object(inventory),
        complete,
        snapshot_id: Uuid::new_v4(),
        schema_version: M5_SCHEMA_VERSION,
    };

    // Keep all section names and statuses if aggregate size reaches the
    // envelope bound. Collector-level data is still available on the host.
    if serde_json::to_vec(&snapshot)
        .map(|bytes| bytes.len() > MAX_SNAPSHOT_BYTES)
        .unwrap_or(true)
    {
        snapshot.complete = false;
        if let Some(object) = snapshot.inventory.as_object_mut() {
            for capability in [
                Capability::Host,
                Capability::Network,
                Capability::Filesystem,
                Capability::Systemd,
                Capability::Packages,
                Capability::Processes,
                Capability::Sockets,
                Capability::Docker,
            ] {
                if object
                    .get(capability.as_str())
                    .and_then(|value| serde_json::to_vec(value).ok())
                    .is_some_and(|bytes| bytes.len() > MAX_COLLECTOR_PAYLOAD_BYTES / 2)
                {
                    object.insert(
                        capability.as_str().into(),
                        json!({"truncated": true, "reason": "snapshot size bound"}),
                    );
                }
            }
        }
        if serde_json::to_vec(&snapshot)
            .map(|bytes| bytes.len() > MAX_SNAPSHOT_BYTES)
            .unwrap_or(true)
            && let Some(object) = snapshot.inventory.as_object_mut()
        {
            for capability in [
                Capability::Host,
                Capability::Network,
                Capability::Filesystem,
                Capability::Systemd,
                Capability::Packages,
                Capability::Processes,
                Capability::Sockets,
                Capability::Docker,
            ] {
                object.insert(
                    capability.as_str().into(),
                    json!({"truncated": true, "reason": "snapshot size bound"}),
                );
            }
        }
    }

    snapshot
}

/// Convert collector status into small, idempotent-friendly observations.
/// The batch ID and each key remain stable when the same snapshot is retried.
pub fn observations_from_snapshot(snapshot: &InventorySnapshot) -> ObservationBatch {
    let mut observations = Vec::new();
    if let Some(statuses) = snapshot
        .inventory
        .get("collector_status")
        .and_then(Value::as_array)
    {
        for status in statuses.iter().take(MAX_OBSERVATIONS) {
            let Some(capability) = status.get("capability").and_then(Value::as_str) else {
                continue;
            };
            let status_name = status
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable");
            let state = match status_name {
                "available" => ObservationState::Ok,
                "partial" => ObservationState::Degraded,
                _ => ObservationState::Unavailable,
            };
            observations.push(Observation {
                idempotency_key: format!("{}/collector/{capability}", snapshot.snapshot_id),
                key: format!("collector.{capability}"),
                source: "agent".into(),
                observed_at_unix_secs: snapshot.collected_at_unix_secs,
                state,
                value: json!({"status": status_name}),
            });
        }
    }
    ObservationBatch {
        schema_version: M5_SCHEMA_VERSION,
        batch_id: snapshot.snapshot_id,
        agent_id: snapshot.agent_id,
        collected_at_unix_secs: snapshot.collected_at_unix_secs,
        observations,
    }
}

fn collect_host() -> Result<Value, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("host collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let os_release = read_bounded("/etc/os-release", MAX_FILE_BYTES)
            .map(|value| parse_os_release(&value))
            .unwrap_or_default();
        let kernel = read_trimmed("/proc/sys/kernel/osrelease", MAX_TEXT_BYTES)
            .unwrap_or_else(|_| "unknown".into());
        let hostname = read_trimmed("/etc/hostname", MAX_TEXT_BYTES)
            .or_else(|_| std::env::var("HOSTNAME").map_err(|err| err.to_string()))
            .unwrap_or_else(|_| "unknown".into());
        let machine_id_hash = hash_identifier(read_trimmed("/etc/machine-id", MAX_TEXT_BYTES).ok());
        let boot_id_hash =
            hash_identifier(read_trimmed("/proc/sys/kernel/random/boot_id", MAX_TEXT_BYTES).ok());
        let uptime_secs = read_trimmed("/proc/uptime", MAX_TEXT_BYTES)
            .ok()
            .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
            .map(|value| value.max(0.0) as u64);
        let load = read_trimmed("/proc/loadavg", MAX_TEXT_BYTES)
            .ok()
            .and_then(|value| parse_load_average(&value));
        let cpu = read_bounded("/proc/cpuinfo", MAX_FILE_BYTES)
            .ok()
            .map(|value| parse_cpuinfo(&value));
        let memory = read_bounded("/proc/meminfo", MAX_FILE_BYTES)
            .ok()
            .map(|value| parse_meminfo(&value));

        Ok(json!({
            "hostname": bounded_text(&hostname, MAX_TEXT_BYTES),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "distribution": os_release.get("PRETTY_NAME").cloned(),
            "distribution_id": os_release.get("ID").cloned(),
            "kernel": kernel,
            "machine_id_sha256": machine_id_hash,
            "boot_id_sha256": boot_id_hash,
            "uptime_secs": uptime_secs,
            "load": load,
            "cpu": cpu,
            "memory": memory,
        }))
    }
}

fn hash_identifier(value: Option<String>) -> Option<String> {
    let value = value?;
    let mut hasher = Sha256::new();
    hasher.update(value.trim().as_bytes());
    Some(hex::encode(hasher.finalize()))
}

pub fn parse_os_release(input: &str) -> BTreeMap<String, String> {
    input
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if !key
                .chars()
                .all(|character| character.is_ascii_uppercase() || character == '_')
            {
                return None;
            }
            let value = value.trim().trim_matches(['"', '\'']);
            Some((
                bounded_text(key, MAX_TEXT_BYTES),
                bounded_text(value, MAX_TEXT_BYTES),
            ))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

fn parse_load_average(input: &str) -> Option<LoadAverage> {
    let mut values = input
        .split_whitespace()
        .take(3)
        .filter_map(|value| value.parse().ok());
    Some(LoadAverage {
        one: values.next()?,
        five: values.next()?,
        fifteen: values.next()?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CpuInfo {
    pub model: Option<String>,
    pub logical_cpus: usize,
}

fn parse_cpuinfo(input: &str) -> CpuInfo {
    let mut model = None;
    let mut logical_cpus: usize = 0;
    for line in input.lines().take(MAX_ITEMS * 8) {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "model name"
        {
            model.get_or_insert(bounded_text(value.trim(), MAX_TEXT_BYTES));
        } else if line.starts_with("processor") && line.contains(':') {
            logical_cpus = logical_cpus.saturating_add(1);
        }
    }
    if logical_cpus == 0 {
        logical_cpus = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
    }
    CpuInfo {
        model,
        logical_cpus,
    }
}

pub fn parse_meminfo(input: &str) -> BTreeMap<String, u64> {
    input
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let mut parts = rest.split_whitespace();
            let value = parts.next()?.parse::<u64>().ok()?;
            let multiplier = match parts.next() {
                Some("kB") => 1024,
                Some("MB") => 1024 * 1024,
                _ => 1,
            };
            Some((
                bounded_text(key.trim(), MAX_TEXT_BYTES),
                value.saturating_mul(multiplier),
            ))
        })
        .collect()
}

async fn collect_network() -> Result<(Value, Vec<String>), String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("network collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let mut names = fs::read_dir("/sys/class/net")
            .map_err(|err| format!("/sys/class/net: {err}"))?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .take(MAX_ITEMS)
            .collect::<Vec<_>>();
        names.sort();

        let address_info = command_output(
            "ip",
            &["-j", "address", "show"],
            COMMAND_TIMEOUT,
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .await
        .ok()
        .map(|value| parse_ip_json_addresses(&value))
        .unwrap_or_default();

        let mut interfaces = Vec::with_capacity(names.len());
        for name in names {
            let base = Path::new("/sys/class/net").join(&name);
            let mut object = Map::new();
            object.insert("name".into(), json!(bounded_text(&name, MAX_TEXT_BYTES)));
            object.insert(
                "mac".into(),
                json!(read_trimmed(base.join("address"), MAX_TEXT_BYTES).ok()),
            );
            object.insert(
                "state".into(),
                json!(read_trimmed(base.join("operstate"), MAX_TEXT_BYTES).ok()),
            );
            object.insert(
                "mtu".into(),
                json!(
                    read_trimmed(base.join("mtu"), MAX_TEXT_BYTES)
                        .ok()
                        .and_then(|value| value.parse::<u32>().ok())
                ),
            );
            object.insert(
                "addresses".into(),
                Value::Array(address_info.get(&name).cloned().unwrap_or_default()),
            );
            interfaces.push(Value::Object(object));
        }

        let mut warnings = Vec::new();
        if interfaces.iter().all(|interface| {
            interface
                .get("addresses")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        }) {
            warnings.push("ip address data unavailable".into());
        }
        let routes = read_bounded("/proc/net/route", MAX_FILE_BYTES)
            .map(|value| parse_proc_routes(&value))
            .unwrap_or_default();
        if routes.is_empty() {
            warnings.push("route data unavailable".into());
        }
        Ok((
            json!({"interfaces": interfaces, "routes": routes}),
            warnings,
        ))
    }
}

pub fn parse_ip_json_addresses(input: &str) -> BTreeMap<String, Vec<Value>> {
    let Ok(Value::Array(interfaces)) = serde_json::from_str(input) else {
        return BTreeMap::new();
    };
    let mut output = BTreeMap::new();
    for interface in interfaces.into_iter().take(MAX_ITEMS) {
        let Some(name) = interface.get("ifname").and_then(Value::as_str) else {
            continue;
        };
        let addresses = interface
            .get("addr_info")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .take(MAX_ITEMS)
                    .filter_map(|value| {
                        let family = value.get("family")?.as_str()?;
                        let local = value.get("local")?.as_str()?;
                        Some(json!({
                            "family": bounded_text(family, MAX_TEXT_BYTES),
                            "address": bounded_text(local, MAX_TEXT_BYTES),
                            "prefix_len": value.get("prefixlen").and_then(Value::as_u64),
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        output.insert(bounded_text(name, MAX_TEXT_BYTES), addresses);
    }
    output
}

pub fn parse_proc_routes(input: &str) -> Vec<Value> {
    input
        .lines()
        .skip(1)
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 8 {
                return None;
            }
            Some(json!({
                "interface": bounded_text(fields[0], MAX_TEXT_BYTES),
                "destination": parse_proc_ipv4(fields[1]),
                "gateway": parse_proc_ipv4(fields[2]),
                "flags": fields[3],
                "metric": fields[6].parse::<u32>().ok(),
                "mask": parse_proc_ipv4(fields[7]),
            }))
        })
        .collect()
}

fn parse_proc_ipv4(value: &str) -> Option<String> {
    let value = u32::from_str_radix(value, 16).ok()?;
    Some(Ipv4Addr::from(value.to_le_bytes()).to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilesystemUsage {
    pub source: String,
    pub mount_point: String,
    pub capacity_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub use_percent: Option<u8>,
}

async fn collect_filesystems() -> Result<(Value, Vec<String>), String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("filesystem collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let mounts = read_bounded("/proc/self/mountinfo", MAX_FILE_BYTES)
            .map(|value| parse_mountinfo(&value))
            .unwrap_or_default();
        let mut warnings = Vec::new();
        let usage = match command_output(
            "df",
            &["-P", "-k"],
            COMMAND_TIMEOUT,
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .await
        {
            Ok(output) => parse_df_output(&output),
            Err(error) => {
                warnings.push(format!("df unavailable: {error}"));
                Vec::new()
            }
        };
        if mounts.is_empty() && usage.is_empty() {
            return Err("mount and filesystem usage data unavailable".into());
        }
        let mut filesystems = Vec::new();
        for mount in mounts.into_iter().take(MAX_ITEMS) {
            let mount_point = mount
                .get("mount_point")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let usage = usage.iter().find(|entry| entry.mount_point == mount_point);
            let mut object = mount.as_object().cloned().unwrap_or_default();
            if let Some(usage) = usage {
                object.insert("capacity_bytes".into(), json!(usage.capacity_bytes));
                object.insert("used_bytes".into(), json!(usage.used_bytes));
                object.insert("available_bytes".into(), json!(usage.available_bytes));
                object.insert("use_percent".into(), json!(usage.use_percent));
            }
            filesystems.push(Value::Object(object));
        }
        for entry in usage.iter().take(MAX_ITEMS) {
            if !filesystems.iter().any(|mount| {
                mount.get("mount_point").and_then(Value::as_str) == Some(entry.mount_point.as_str())
            }) {
                filesystems.push(serde_json::to_value(entry).unwrap_or(Value::Null));
            }
        }
        Ok((json!({"filesystems": filesystems}), warnings))
    }
}

fn parse_mountinfo(input: &str) -> Vec<Value> {
    input
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let before = before.split_whitespace().collect::<Vec<_>>();
            let after = after.split_whitespace().collect::<Vec<_>>();
            if before.len() < 6 || after.len() < 2 {
                return None;
            }
            Some(json!({
                "mount_point": unescape_mount_field(before[4]),
                "options": bounded_text(before[5], MAX_TEXT_BYTES),
                "filesystem": bounded_text(after[0], MAX_TEXT_BYTES),
                "source": unescape_mount_field(after[1]),
            }))
        })
        .collect()
}

fn unescape_mount_field(value: &str) -> String {
    let mut output = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if index + 4 <= bytes.len()
            && bytes[index] == b'\\'
            && let Ok(decoded) = u8::from_str_radix(&value[index + 1..index + 4], 8)
        {
            output.push(decoded as char);
            index += 4;
            continue;
        }
        output.push(bytes[index] as char);
        index += 1;
    }
    bounded_text(&output, MAX_TEXT_BYTES)
}

pub fn parse_df_output(input: &str) -> Vec<FilesystemUsage> {
    input
        .lines()
        .skip(1)
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 6 {
                return None;
            }
            let mount_index = fields.len() - 1;
            let percent_index = fields.len() - 2;
            let blocks_index = fields.len() - 5;
            Some(FilesystemUsage {
                source: bounded_text(&fields[..blocks_index].join(" "), MAX_TEXT_BYTES),
                mount_point: bounded_text(fields[mount_index], MAX_TEXT_BYTES),
                capacity_bytes: fields[blocks_index]
                    .parse::<u64>()
                    .ok()
                    .map(|value| value.saturating_mul(1024)),
                used_bytes: fields[blocks_index + 1]
                    .parse::<u64>()
                    .ok()
                    .map(|value| value.saturating_mul(1024)),
                available_bytes: fields[blocks_index + 2]
                    .parse::<u64>()
                    .ok()
                    .map(|value| value.saturating_mul(1024)),
                use_percent: fields[percent_index]
                    .strip_suffix('%')
                    .and_then(|value| value.parse::<u8>().ok()),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemdUnit {
    pub unit: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

async fn collect_systemd() -> Result<Value, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("systemd collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let output = command_output(
            "systemctl",
            &[
                "list-units",
                "--type=service",
                "--all",
                "--no-legend",
                "--no-pager",
                "--plain",
            ],
            COMMAND_TIMEOUT,
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .await?;
        let units = parse_systemd_units(&output);
        if units.is_empty() {
            return Err("systemd returned no service units".into());
        }
        Ok(json!({"units": units}))
    }
}

pub fn parse_systemd_units(input: &str) -> Vec<SystemdUnit> {
    input
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4 {
                return None;
            }
            Some(SystemdUnit {
                unit: bounded_text(fields[0], MAX_TEXT_BYTES),
                load: bounded_text(fields[1], MAX_TEXT_BYTES),
                active: bounded_text(fields[2], MAX_TEXT_BYTES),
                sub: bounded_text(fields[3], MAX_TEXT_BYTES),
                description: bounded_text(&fields[4..].join(" "), MAX_TEXT_BYTES),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
}

async fn collect_packages() -> Result<Value, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("package collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let commands: [(&str, &[&str], &str); 3] = [
            (
                "dpkg-query",
                &["-W", "-f=${binary:Package}\\t${Version}\\n"],
                "dpkg",
            ),
            (
                "rpm",
                &["-qa", "--qf", "%{NAME}\\t%{VERSION}-%{RELEASE}\\n"],
                "rpm",
            ),
            ("apk", &["info", "-v"], "apk"),
        ];
        let mut selected = None;
        for (program, args, manager) in commands {
            if let Ok(output) =
                command_output(program, args, COMMAND_TIMEOUT, MAX_COMMAND_OUTPUT_BYTES).await
            {
                selected = Some((manager, parse_package_lines(&output)));
                break;
            }
        }
        let Some((manager, packages)) = selected else {
            return Err("no supported package manager available".into());
        };

        let pending = command_output(
            "apt",
            &["list", "--upgradable"],
            COMMAND_TIMEOUT,
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .await
        .map(|output| parse_pending_packages(&output))
        .unwrap_or_default();
        let reboot_required = Path::new("/var/run/reboot-required").exists()
            || Path::new("/run/reboot-required").exists();
        Ok(json!({
            "manager": manager,
            "packages": packages,
            "pending_updates": pending,
            "reboot_required": reboot_required,
        }))
    }
}

pub fn parse_package_lines(input: &str) -> Vec<PackageRecord> {
    input
        .lines()
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Listing...") {
                return None;
            }
            let (name, version) = line.split_once('\t').or_else(|| {
                let mut fields = line.split_whitespace();
                Some((fields.next()?, fields.next()?))
            })?;
            Some(PackageRecord {
                name: bounded_text(name, MAX_TEXT_BYTES),
                version: bounded_text(version, MAX_TEXT_BYTES),
            })
        })
        .collect()
}

fn parse_pending_packages(input: &str) -> Vec<String> {
    input
        .lines()
        .filter(|line| !line.starts_with("WARNING") && !line.starts_with("Listing"))
        .take(MAX_ITEMS)
        .map(|line| bounded_text(line.trim(), MAX_TEXT_BYTES))
        .filter(|line| !line.is_empty())
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub name: String,
    pub state: Option<String>,
    pub rss_bytes: Option<u64>,
    pub command: Option<String>,
}

fn collect_processes() -> Result<Value, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("process collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let mut pids = fs::read_dir("/proc")
            .map_err(|err| format!("/proc: {err}"))?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .collect::<Vec<_>>();
        pids.sort_unstable();
        pids.truncate(MAX_ITEMS);
        let mut processes = Vec::new();
        for pid in pids {
            let base = Path::new("/proc").join(pid.to_string());
            let name = read_trimmed(base.join("comm"), MAX_TEXT_BYTES).ok();
            let stat = read_bounded(base.join("stat"), MAX_TEXT_BYTES).ok();
            let Some(name) = name else { continue };
            let state = stat.as_deref().and_then(parse_proc_stat_state);
            let rss_bytes = read_bounded(base.join("status"), MAX_FILE_BYTES)
                .ok()
                .and_then(|value| parse_status_rss(&value));
            let command = read_bounded(base.join("cmdline"), MAX_TEXT_BYTES)
                .ok()
                .map(|value| bounded_text(&value.replace('\0', " "), MAX_TEXT_BYTES));
            processes.push(ProcessRecord {
                pid,
                name,
                state,
                rss_bytes,
                command,
            });
        }
        if processes.is_empty() {
            return Err("no readable process entries".into());
        }
        Ok(json!({"processes": processes}))
    }
}

pub fn parse_proc_stat_state(input: &str) -> Option<String> {
    let close = input.rfind(") ")?;
    input[close + 2..]
        .split_whitespace()
        .next()
        .map(|value| bounded_text(value, MAX_TEXT_BYTES))
}

fn parse_status_rss(input: &str) -> Option<u64> {
    input.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?.split_whitespace().next()?;
        value.parse::<u64>().ok()?.checked_mul(1024)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SocketRecord {
    pub protocol: String,
    pub local_address: String,
    pub local_port: u16,
    pub state: String,
    pub inode: u64,
    pub pid: Option<u32>,
    pub process_name: Option<String>,
}

fn collect_sockets() -> Result<Value, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Err("socket collector supports Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        let owners = socket_owners();
        let mut sockets = Vec::new();
        for (path, protocol) in [
            ("/proc/net/tcp", "tcp"),
            ("/proc/net/tcp6", "tcp6"),
            ("/proc/net/udp", "udp"),
            ("/proc/net/udp6", "udp6"),
        ] {
            let content = match read_bounded(path, MAX_FILE_BYTES) {
                Ok(content) => content,
                Err(_) => continue,
            };
            for mut socket in parse_socket_table(&content, protocol) {
                if let Some((pid, name)) = owners.get(&socket.inode) {
                    socket.pid = Some(*pid);
                    socket.process_name = Some(name.clone());
                }
                sockets.push(socket);
                if sockets.len() >= MAX_ITEMS {
                    break;
                }
            }
            if sockets.len() >= MAX_ITEMS {
                break;
            }
        }
        if sockets.is_empty() {
            return Err("no readable listening socket entries".into());
        }
        Ok(json!({"sockets": sockets}))
    }
}

pub fn parse_socket_table(input: &str, protocol: &str) -> Vec<SocketRecord> {
    input
        .lines()
        .skip(1)
        .take(MAX_ITEMS)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 11 {
                return None;
            }
            let state = fields[3];
            if protocol.starts_with("tcp") && state != "0A" {
                return None;
            }
            let (address, port) = parse_socket_endpoint(fields[1])?;
            Some(SocketRecord {
                protocol: protocol.into(),
                local_address: address,
                local_port: port,
                state: bounded_text(state, MAX_TEXT_BYTES),
                inode: fields[10].parse().ok()?,
                pid: None,
                process_name: None,
            })
        })
        .collect()
}

fn parse_socket_endpoint(value: &str) -> Option<(String, u16)> {
    let (address, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let address = if address.len() == 8 {
        let value = u32::from_str_radix(address, 16).ok()?;
        Ipv4Addr::from(value.to_le_bytes()).to_string()
    } else {
        bounded_text(address, MAX_TEXT_BYTES)
    };
    Some((address, port))
}

#[cfg(target_os = "linux")]
fn socket_owners() -> HashMap<u64, (u32, String)> {
    let mut owners = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return owners;
    };
    let mut pids = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect::<Vec<_>>();
    pids.sort_unstable();
    for pid in pids.into_iter().take(MAX_ITEMS) {
        let name = read_trimmed(
            Path::new("/proc").join(pid.to_string()).join("comm"),
            MAX_TEXT_BYTES,
        )
        .unwrap_or_else(|_| "unknown".into());
        let Ok(fds) = fs::read_dir(Path::new("/proc").join(pid.to_string()).join("fd")) else {
            continue;
        };
        for fd in fds.take(MAX_ITEMS).filter_map(Result::ok) {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            let Some(inode) = target
                .strip_prefix("socket:[")
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            owners.entry(inode).or_insert((pid, name.clone()));
        }
    }
    owners
}

#[cfg(not(target_os = "linux"))]
fn socket_owners() -> HashMap<u64, (u32, String)> {
    HashMap::new()
}

async fn collect_docker() -> Result<Value, String> {
    let engine = command_output(
        "docker",
        &["version", "--format", "{{json .Server}}"],
        COMMAND_TIMEOUT,
        MAX_COMMAND_OUTPUT_BYTES,
    )
    .await
    .ok()
    .and_then(|output| parse_first_json(&output));

    let containers = command_output(
        "docker",
        &["ps", "-a", "--no-trunc", "--format", "{{json .}}"],
        COMMAND_TIMEOUT,
        MAX_COMMAND_OUTPUT_BYTES,
    )
    .await
    .ok()
    .map(|output| parse_docker_containers(&output))
    .unwrap_or_default();
    let images = command_output(
        "docker",
        &["images", "--no-trunc", "--format", "{{json .}}"],
        COMMAND_TIMEOUT,
        MAX_COMMAND_OUTPUT_BYTES,
    )
    .await
    .ok()
    .map(|output| parse_json_lines(&output, MAX_ITEMS))
    .unwrap_or_default();
    let networks = command_output(
        "docker",
        &["network", "ls", "--format", "{{json .}}"],
        COMMAND_TIMEOUT,
        MAX_COMMAND_OUTPUT_BYTES,
    )
    .await
    .ok()
    .map(|output| parse_json_lines(&output, MAX_ITEMS))
    .unwrap_or_default();

    if engine.is_none() && containers.is_empty() && images.is_empty() && networks.is_empty() {
        return Err("Docker engine or CLI unavailable".into());
    }
    Ok(json!({
        "engine": engine,
        "containers": containers,
        "images": images,
        "networks": networks,
    }))
}

pub fn parse_json_lines(input: &str, max_items: usize) -> Vec<Value> {
    input
        .lines()
        .take(max_items)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.is_object())
        .collect()
}

fn parse_first_json(input: &str) -> Option<Value> {
    serde_json::from_str::<Value>(input.trim())
        .ok()
        .or_else(|| parse_json_lines(input, 1).into_iter().next())
}

fn parse_docker_containers(input: &str) -> Vec<Value> {
    parse_json_lines(input, MAX_ITEMS)
        .into_iter()
        .map(|value| {
            let id = value.get("ID").and_then(Value::as_str).unwrap_or_default();
            let ports = value.get("Ports").and_then(Value::as_str).unwrap_or_default();
            json!({
                "id": bounded_text(id, MAX_TEXT_BYTES),
                "name": bounded_text(value.get("Names").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "image": bounded_text(value.get("Image").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "state": bounded_text(value.get("State").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "status": bounded_text(value.get("Status").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "labels": bounded_text(value.get("Labels").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "networks": bounded_text(value.get("Networks").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT_BYTES),
                "published_ports": parse_published_ports(ports),
            })
        })
        .collect()
}

pub fn parse_published_ports(input: &str) -> Vec<Value> {
    input
        .split(',')
        .take(MAX_ITEMS)
        .filter_map(|entry| {
            let (host_part, container_part) = entry.trim().split_once("->")?;
            let (container_port, protocol) = container_part
                .split_once('/')
                .unwrap_or((container_part, "tcp"));
            let (host, host_port) = host_part.rsplit_once(':')?;
            Some(json!({
                "host": bounded_text(host.trim_matches(['[', ']']).trim(), MAX_TEXT_BYTES),
                "host_port": host_port.parse::<u16>().ok(),
                "container_port": container_port.parse::<u16>().ok(),
                "protocol": bounded_text(protocol, MAX_TEXT_BYTES),
            }))
        })
        .collect()
}

async fn command_output(
    program: &str,
    args: &[&str],
    command_timeout: Duration,
    max_bytes: usize,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program}: stdout unavailable"))?;
    let result = timeout(command_timeout, async {
        let mut bytes = Vec::new();
        stdout
            .take((max_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|err| format!("{program}: read stdout: {err}"))?;
        if bytes.len() > max_bytes {
            return Err(format!("{program} output exceeds {max_bytes} bytes"));
        }
        let status = child
            .wait()
            .await
            .map_err(|err| format!("{program}: wait: {err}"))?;
        if !status.success() {
            return Err(format!(
                "{program} exited with status {}",
                status
                    .code()
                    .map_or_else(|| "signal".into(), |code| code.to_string())
            ));
        }
        Ok::<_, String>(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(format!(
                "{program} timed out after {}s",
                command_timeout.as_secs()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_keep_representative_linux_fields() {
        let os = parse_os_release("ID=debian\nPRETTY_NAME=\"Debian GNU/Linux\"\n");
        assert_eq!(os.get("ID"), Some(&"debian".to_string()));
        assert_eq!(os.get("PRETTY_NAME"), Some(&"Debian GNU/Linux".to_string()));

        let route = parse_proc_routes(
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\neth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n",
        );
        assert_eq!(route[0]["destination"], "0.0.0.0");
        assert_eq!(route[0]["gateway"], "192.168.1.1");

        let df = parse_df_output(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 1000 250 750 25% /\n",
        );
        assert_eq!(df[0].mount_point, "/");
        assert_eq!(df[0].used_bytes, Some(250 * 1024));

        let units = parse_systemd_units(
            "ssh.service loaded active running OpenSSH server\nmissing.service not-found inactive dead Missing\n",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].unit, "ssh.service");

        let packages = parse_package_lines("curl\t8.0\nopenssl\t3.0\n");
        assert_eq!(packages[1].version, "3.0");
    }

    #[test]
    fn parsers_bound_items_and_text() {
        let lines = (0..(MAX_ITEMS + 20))
            .map(|index| format!("pkg{index}\t1"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_package_lines(&lines).len(), MAX_ITEMS);

        let long = "x".repeat(MAX_TEXT_BYTES * 2);
        let parsed = parse_os_release(&format!("ID={long}\n"));
        assert_eq!(parsed["ID"].len(), MAX_TEXT_BYTES);

        let json_lines = (0..(MAX_ITEMS + 10))
            .map(|index| format!(r#"{{"ID":"{index}"}}"#))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_json_lines(&json_lines, MAX_ITEMS).len(), MAX_ITEMS);
    }

    #[test]
    fn parses_sockets_and_docker_ports() {
        let sockets = parse_socket_table(
            "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 0 12345 1 2 3\n",
            "tcp",
        );
        assert_eq!(sockets[0].local_address, "127.0.0.1");
        assert_eq!(sockets[0].local_port, 8080);
        assert_eq!(sockets[0].inode, 12345);

        let ports = parse_published_ports("0.0.0.0:8080->80/tcp, :::8443->443/tcp");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0]["host_port"], 8080);
        assert_eq!(ports[1]["container_port"], 443);

        let containers = parse_docker_containers(
            r#"{"ID":"abc","Names":"web","Image":"nginx:latest","State":"running","Status":"Up","Ports":"0.0.0.0:8080->80/tcp","Labels":"app=web","Networks":"bridge"}"#,
        );
        assert_eq!(containers[0]["name"], "web");
        assert_eq!(containers[0]["published_ports"][0]["container_port"], 80);
    }

    #[test]
    fn observations_use_snapshot_id_for_retries() {
        let snapshot = InventorySnapshot {
            agent_id: Uuid::new_v4(),
            sequence: 4,
            collected_at_unix_secs: 10,
            inventory: json!({
                "collector_status": [{"capability":"host","status":"available"}]
            }),
            complete: true,
            snapshot_id: Uuid::new_v4(),
            schema_version: M5_SCHEMA_VERSION,
        };
        let first = observations_from_snapshot(&snapshot);
        let second = observations_from_snapshot(&snapshot);
        assert_eq!(first, second);
        assert_eq!(first.observations.len(), 1);
        assert!(first.validate().is_ok());
    }

    #[test]
    fn snapshot_status_count_stays_bounded() {
        let agent_id = Uuid::new_v4();
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let snapshot = runtime.block_on(collect_snapshot(agent_id));
        assert!(snapshot.inventory.is_object());
        assert!(
            snapshot.inventory["collector_status"]
                .as_array()
                .is_some_and(|statuses| statuses.len() <= MAX_COLLECTORS)
        );
        assert!(serde_json::to_vec(&snapshot).unwrap().len() <= MAX_SNAPSHOT_BYTES);
    }
}
