//! Collectors beyond the core CPU, memory, network and filesystem sample: memory
//! breakdown, load average, temperature sensors, per-device disk throughput, ZFS
//! pools, service units and configured reachability monitors.
//!
//! Every collector is bounded (list sizes, subprocess output, deadlines) and reports
//! `None` or an empty list when its source is unavailable; nothing is estimated.
use hope_protocol::{
    DiskIo, LoadAverage, MonitorResult, ServiceSummary, ServiceUnit, StoragePool, Temperature,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};
use sysinfo::Components;

pub const MAX_MONITORS: usize = 32;
pub const MAX_WATCHED_SERVICES: usize = 64;
const MAX_DEVICES: usize = 64;
const MAX_SENSORS: usize = 64;
const MAX_POOLS: usize = 64;
const MAX_UNITS: usize = 128;
const MONITOR_TIMEOUT: Duration = Duration::from_secs(5);

/// Operator settings read at start: `HOPE_MONITORS` and `HOPE_WATCH_SERVICES` (comma
/// separated) take precedence over `settings.json` in the state directory, which holds
/// `{"monitors": [...], "watch_services": [...]}`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub monitors: Vec<String>,
    #[serde(default)]
    pub watch_services: Vec<String>,
    #[serde(skip)]
    pub ca_pem: Option<String>,
}
impl Settings {
    pub async fn load(dir: &Path) -> Self {
        let mut settings =
            match crate::config::read_bounded(&dir.join("settings.json"), 64 * 1024).await {
                Ok(bytes) => serde_json::from_slice::<Settings>(&bytes).unwrap_or_default(),
                Err(_) => Settings::default(),
            };
        let list = |name: &str| {
            std::env::var(name).ok().map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
        };
        if let Some(monitors) = list("HOPE_MONITORS") {
            settings.monitors = monitors;
        }
        if let Some(services) = list("HOPE_WATCH_SERVICES") {
            settings.watch_services = services;
        }
        settings
            .monitors
            .retain(|target| target.len() <= 255 && parse_monitor(target).is_some());
        let mut seen = HashSet::new();
        settings
            .monitors
            .retain(|target| seen.insert(target.clone()));
        settings.monitors.truncate(MAX_MONITORS);
        settings.watch_services.retain(|unit| {
            !unit.is_empty() && unit.len() <= 256 && !unit.chars().any(char::is_control)
        });
        settings.watch_services.truncate(MAX_WATCHED_SERVICES);
        settings.ca_pem = crate::config::read(dir)
            .await
            .ok()
            .and_then(|config| config.ca_pem);
        settings
    }
}

// ---------- Memory ----------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemoryBreakdown {
    pub cached_bytes: Option<f64>,
    pub zfs_arc_bytes: Option<f64>,
}
pub async fn memory_breakdown(is_container: bool) -> MemoryBreakdown {
    if !cfg!(target_os = "linux") {
        return MemoryBreakdown::default();
    }
    let meminfo = tokio::fs::read_to_string("/proc/meminfo").await.ok();
    // ZFS kstats can expose the host's ARC inside an LXC guest, whereas
    // lxcfs virtualizes meminfo. Never subtract host ARC from guest memory.
    let arc = if is_container {
        None
    } else {
        tokio::fs::read_to_string("/proc/spl/kstat/zfs/arcstats")
            .await
            .ok()
    };
    MemoryBreakdown {
        cached_bytes: meminfo.as_deref().and_then(parse_meminfo_cache),
        zfs_arc_bytes: arc.as_deref().and_then(parse_arc_size),
    }
}
/// Page cache, buffers and reclaimable slab, in bytes.
pub fn parse_meminfo_cache(text: &str) -> Option<f64> {
    let mut values = HashMap::new();
    for line in text.lines() {
        let (key, rest) = line.split_once(':')?;
        let number = rest.split_whitespace().next()?.parse::<u64>().ok()?;
        values.insert(key.trim(), number.saturating_mul(1024));
    }
    let cached = *values.get("Cached")?;
    let buffers = values.get("Buffers").copied().unwrap_or(0);
    let slab = values.get("SReclaimable").copied().unwrap_or(0);
    Some(cached.saturating_add(buffers).saturating_add(slab) as f64)
}
pub fn parse_arc_size(text: &str) -> Option<f64> {
    text.lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            (parts.next()? == "size")
                .then(|| parts.nth(1)?.parse::<u64>().ok())
                .flatten()
        })
        .map(|v| v as f64)
}

// ---------- Load average ----------

pub fn load_average() -> Option<LoadAverage> {
    let load = sysinfo::System::load_average();
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    (finite(load.one) && finite(load.five) && finite(load.fifteen) && !cfg!(windows)).then_some(
        LoadAverage {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        },
    )
}

// ---------- Temperatures ----------

pub struct Sensors {
    components: Components,
}
impl Sensors {
    pub fn new() -> Self {
        Self {
            components: Components::new(),
        }
    }
    pub fn sample(&mut self) -> Vec<Temperature> {
        self.components.refresh(true);
        let mut seen = HashSet::new();
        let mut result: Vec<Temperature> = self
            .components
            .iter()
            .filter_map(|component| {
                let celsius = component.temperature()? as f64;
                if !celsius.is_finite() || !(-100.0..=300.0).contains(&celsius) {
                    return None;
                }
                let label = clean(component.label(), 128);
                let sensor = clean(component.id().unwrap_or(component.label()), 128);
                if sensor.is_empty() || !seen.insert(sensor.clone()) {
                    return None;
                }
                Some(Temperature {
                    sensor,
                    label: if label.is_empty() {
                        "Sensor".into()
                    } else {
                        label
                    },
                    celsius,
                })
            })
            .collect();
        result.sort_by(|a, b| a.sensor.cmp(&b.sensor));
        result.truncate(MAX_SENSORS);
        result
    }
}
pub(crate) fn clean(text: &str, maximum: usize) -> String {
    let mut value: String = text.chars().filter(|c| !c.is_control()).collect();
    value = value.trim().to_string();
    if value.len() > maximum {
        let mut end = maximum;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

// ---------- Root filesystem ----------

/// What backs "/": a whole block device name (for example "nvme0n1") or a ZFS pool name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Root {
    pub device: Option<String>,
    pub pool: Option<String>,
}
pub async fn root() -> Root {
    if !cfg!(target_os = "linux") {
        return Root::default();
    }
    let Ok(mounts) = tokio::fs::read_to_string("/proc/mounts").await else {
        return Root::default();
    };
    let Some((source, fstype)) = parse_root_mount(&mounts) else {
        return Root::default();
    };
    if fstype == "zfs" {
        return Root {
            device: None,
            pool: Some(source.split('/').next().unwrap_or(&source).to_string()),
        };
    }
    let resolved = tokio::fs::canonicalize(&source).await.ok();
    let name = resolved
        .as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string());
    let Some(name) = name else {
        return Root::default();
    };
    Root {
        device: whole_disk(&name).await,
        pool: None,
    }
}
pub fn parse_root_mount(mounts: &str) -> Option<(String, String)> {
    mounts.lines().rev().find_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        (fields.len() >= 3 && fields[1] == "/")
            .then(|| (fields[0].to_string(), fields[2].to_string()))
    })
}
/// The whole disk a partition belongs to, or the device itself when it is not a partition.
async fn whole_disk(name: &str) -> Option<String> {
    if tokio::fs::try_exists(format!("/sys/block/{name}"))
        .await
        .unwrap_or(false)
    {
        return Some(name.to_string());
    }
    // A partition's sysfs path is /sys/block/<disk>/<partition>.
    let mut entries = tokio::fs::read_dir("/sys/block").await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if tokio::fs::try_exists(entry.path().join(name))
            .await
            .unwrap_or(false)
        {
            return Some(entry.file_name().to_string_lossy().to_string());
        }
    }
    None
}

// ---------- Per-device disk throughput ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceCounters {
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub busy_ms: u64,
}
pub async fn device_counters() -> HashMap<String, DeviceCounters> {
    if !cfg!(target_os = "linux") {
        return HashMap::new();
    }
    let Ok(text) = tokio::fs::read_to_string("/proc/diskstats").await else {
        return HashMap::new();
    };
    let mut result = HashMap::new();
    for (name, counters) in parse_diskstats(&text) {
        if result.len() >= MAX_DEVICES {
            break;
        }
        if tokio::fs::try_exists(format!("/sys/block/{name}"))
            .await
            .unwrap_or(false)
        {
            result.insert(name, counters);
        }
    }
    result
}
pub fn parse_diskstats(text: &str) -> Vec<(String, DeviceCounters)> {
    text.lines()
        .filter_map(|line| {
            let p: Vec<_> = line.split_whitespace().collect();
            if p.len() < 14 {
                return None;
            }
            let name = p[2];
            if ["loop", "ram", "zram", "sr", "fd"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                return None;
            }
            Some((
                name.to_string(),
                DeviceCounters {
                    read_bytes: p[5].parse::<u64>().ok()?.saturating_mul(512),
                    write_bytes: p[9].parse::<u64>().ok()?.saturating_mul(512),
                    busy_ms: p[12].parse::<u64>().ok()?,
                },
            ))
        })
        .collect()
}
pub fn device_rates(
    previous: &HashMap<String, DeviceCounters>,
    current: &HashMap<String, DeviceCounters>,
    elapsed: f64,
    root: &Root,
) -> Vec<DiskIo> {
    let mut result: Vec<DiskIo> = current
        .iter()
        .map(|(name, now)| {
            let old = previous.get(name);
            let busy = old
                .and_then(|old| crate::collect::rate(old.busy_ms, now.busy_ms, elapsed))
                .map(|ms_per_second| (ms_per_second / 10.0).min(100.0));
            DiskIo {
                device: name.clone(),
                read_bytes_per_second: old
                    .and_then(|old| crate::collect::rate(old.read_bytes, now.read_bytes, elapsed)),
                write_bytes_per_second: old.and_then(|old| {
                    crate::collect::rate(old.write_bytes, now.write_bytes, elapsed)
                }),
                busy_percent: busy,
                root: root.device.as_deref() == Some(name),
            }
        })
        .collect();
    result.sort_by(|a, b| a.device.cmp(&b.device));
    result
}

// ---------- ZFS pools ----------

#[derive(Clone, Debug, PartialEq)]
pub struct PoolCapacity {
    pub name: String,
    pub size_bytes: f64,
    pub allocated_bytes: f64,
    pub health: String,
}
/// `zpool list` without the tool, a pool or permission yields `None` (collector unavailable);
/// a host with the tool and no pools yields an empty list.
pub async fn pool_capacities() -> Option<Vec<PoolCapacity>> {
    let output = crate::collect::command(
        "zpool",
        &["list", "-Hp", "-o", "name,size,allocated,free,health"],
        3,
    )
    .await?;
    Some(parse_zpool_list(&output))
}
pub fn parse_zpool_list(output: &str) -> Vec<PoolCapacity> {
    output
        .lines()
        .take(MAX_POOLS)
        .filter_map(|line| {
            let f: Vec<_> = line.split('\t').collect();
            if f.len() < 5 {
                return None;
            }
            let size = f[1]
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)?;
            let allocated = f[2]
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)?;
            Some(PoolCapacity {
                name: clean(f[0], 128),
                size_bytes: size,
                allocated_bytes: allocated.min(size),
                health: clean(f[4], 32),
            })
        })
        .collect()
}
/// Cumulative bytes read and written per pool, summed over its datasets' kstats.
pub async fn pool_counters(pools: &[PoolCapacity]) -> HashMap<String, (u64, u64)> {
    let mut result = HashMap::new();
    if !cfg!(target_os = "linux") {
        return result;
    }
    for pool in pools {
        let Ok(mut entries) =
            tokio::fs::read_dir(format!("/proc/spl/kstat/zfs/{}", pool.name)).await
        else {
            continue;
        };
        let mut read = 0u64;
        let mut written = 0u64;
        let mut found = false;
        let mut objsets = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.file_name().to_string_lossy().starts_with("objset-") {
                continue;
            }
            objsets += 1;
            if objsets > 4096 {
                break;
            }
            if let Ok(text) = tokio::fs::read_to_string(entry.path()).await
                && let Some((r, w)) = parse_objset(&text)
            {
                read = read.saturating_add(r);
                written = written.saturating_add(w);
                found = true;
            }
        }
        if found {
            result.insert(pool.name.clone(), (read, written));
        }
    }
    result
}
pub fn parse_objset(text: &str) -> Option<(u64, u64)> {
    let field = |key: &str| {
        text.lines().find_map(|line| {
            let mut parts = line.split_whitespace();
            (parts.next()? == key)
                .then(|| parts.nth(1)?.parse::<u64>().ok())
                .flatten()
        })
    };
    Some((field("nread")?, field("nwritten")?))
}
pub fn pools(
    capacities: &[PoolCapacity],
    previous: &HashMap<String, (u64, u64)>,
    current: &HashMap<String, (u64, u64)>,
    elapsed: f64,
    root: &Root,
) -> Vec<StoragePool> {
    capacities
        .iter()
        .map(|pool| {
            let old = previous.get(&pool.name);
            let now = current.get(&pool.name);
            StoragePool {
                name: pool.name.clone(),
                kind: "zfs".into(),
                size_bytes: pool.size_bytes,
                allocated_bytes: pool.allocated_bytes,
                health: pool.health.clone(),
                read_bytes_per_second: old
                    .zip(now)
                    .and_then(|(o, n)| crate::collect::rate(o.0, n.0, elapsed)),
                write_bytes_per_second: old
                    .zip(now)
                    .and_then(|(o, n)| crate::collect::rate(o.1, n.1, elapsed)),
                root: root.pool.as_deref() == Some(&pool.name),
            }
        })
        .collect()
}

// ---------- Service units ----------

pub struct ServiceReport {
    pub units: Vec<ServiceUnit>,
    pub summary: ServiceSummary,
}
/// systemd only: failed units plus the watched list. `None` without a usable service manager.
pub async fn services(watched: &[String]) -> Option<ServiceReport> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let output = crate::collect::command(
        "systemctl",
        &[
            "list-units",
            "--type=service",
            "--all",
            "--plain",
            "--no-legend",
            "--no-pager",
            "--full",
        ],
        3,
    )
    .await?;
    Some(parse_units(&output, watched))
}
pub fn parse_units(output: &str, watched: &[String]) -> ServiceReport {
    let is_watched = |name: &str| {
        watched.iter().any(|w| {
            w == name || w.trim_end_matches(".service") == name.trim_end_matches(".service")
        })
    };
    let mut summary = ServiceSummary {
        active: 0,
        failed: 0,
        total: 0,
    };
    let mut units = vec![];
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(load), Some(active), Some(sub)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if load == "not-found" {
            continue;
        }
        summary.total = summary.total.saturating_add(1);
        if active == "active" {
            summary.active = summary.active.saturating_add(1);
        }
        if active == "failed" {
            summary.failed = summary.failed.saturating_add(1);
        }
        let watched = is_watched(name);
        if (active == "failed" || watched) && units.len() < MAX_UNITS {
            units.push(ServiceUnit {
                name: clean(name, 256),
                state: clean(active, 32),
                sub_state: clean(sub, 32),
                description: clean(&parts.collect::<Vec<_>>().join(" "), 256),
                watched,
            });
        }
    }
    units.sort_by(|a, b| (!a.watched, &a.name).cmp(&(!b.watched, &b.name)));
    ServiceReport { units, summary }
}

// ---------- Monitors ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Monitor {
    Tcp { host: String, port: u16 },
    Http { url: String, secure: bool },
}
pub fn parse_monitor(target: &str) -> Option<Monitor> {
    let target = target.trim();
    if let Some(rest) = target.strip_prefix("tcp://") {
        let (host, port) = rest.rsplit_once(':')?;
        let host = host.trim_matches(|c| c == '[' || c == ']');
        let port = port.parse::<u16>().ok().filter(|p| *p > 0)?;
        return (!host.is_empty() && !host.chars().any(|c| c.is_whitespace() || c == '/')).then(
            || Monitor::Tcp {
                host: host.to_string(),
                port,
            },
        );
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        let url = reqwest::Url::parse(target).ok()?;
        if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
            return None;
        }
        return Some(Monitor::Http {
            url: url.to_string(),
            secure: url.scheme() == "https",
        });
    }
    None
}
pub struct Monitors {
    targets: Vec<(String, Monitor)>,
    client: Option<reqwest::Client>,
}
impl Monitors {
    pub fn new(settings: &Settings) -> Self {
        let targets: Vec<_> = settings
            .monitors
            .iter()
            .filter_map(|t| Some((t.clone(), parse_monitor(t)?)))
            .collect();
        let client = targets
            .iter()
            .any(|(_, m)| matches!(m, Monitor::Http { .. }))
            .then(|| {
                let mut builder = reqwest::Client::builder()
                    .timeout(MONITOR_TIMEOUT)
                    .connect_timeout(MONITOR_TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent(format!("hope-agent-monitor/{}", crate::VERSION));
                if let Some(pem) = &settings.ca_pem
                    && let Ok(certificates) = reqwest::Certificate::from_pem_bundle(pem.as_bytes())
                {
                    for certificate in certificates {
                        builder = builder.add_root_certificate(certificate);
                    }
                }
                builder.build().ok()
            })
            .flatten();
        Self { targets, client }
    }
    pub fn configured(&self) -> usize {
        self.targets.len()
    }
    /// Every check runs concurrently under one deadline; a slow target cannot delay sampling.
    pub async fn sample(&self) -> Vec<MonitorResult> {
        let mut set = tokio::task::JoinSet::new();
        for (index, (target, monitor)) in self.targets.iter().enumerate() {
            let target = target.clone();
            let monitor = monitor.clone();
            let client = self.client.clone();
            set.spawn(async move { (index, check(target, monitor, client).await) });
        }
        let mut results = vec![];
        while let Some(Ok((index, result))) = set.join_next().await {
            results.push((index, result));
        }
        results.sort_by_key(|(index, _)| *index);
        results.into_iter().map(|(_, result)| result).collect()
    }
}
async fn check(target: String, monitor: Monitor, client: Option<reqwest::Client>) -> MonitorResult {
    let started = Instant::now();
    let latency = |started: Instant| Some(started.elapsed().as_secs_f64() * 1000.0);
    match monitor {
        Monitor::Tcp { host, port } => {
            let attempt = tokio::time::timeout(
                MONITOR_TIMEOUT,
                tokio::net::TcpStream::connect((host.as_str(), port)),
            )
            .await;
            match attempt {
                Ok(Ok(_)) => MonitorResult {
                    target,
                    kind: "tcp".into(),
                    up: true,
                    latency_ms: latency(started),
                    status_code: None,
                    detail: None,
                },
                Ok(Err(error)) => MonitorResult {
                    target,
                    kind: "tcp".into(),
                    up: false,
                    latency_ms: None,
                    status_code: None,
                    detail: Some(clean(&error.kind().to_string(), 128)),
                },
                Err(_) => MonitorResult {
                    target,
                    kind: "tcp".into(),
                    up: false,
                    latency_ms: None,
                    status_code: None,
                    detail: Some("timed out".into()),
                },
            }
        }
        Monitor::Http { url, secure } => {
            let kind = if secure { "https" } else { "http" };
            let Some(client) = client else {
                return MonitorResult {
                    target,
                    kind: kind.into(),
                    up: false,
                    latency_ms: None,
                    status_code: None,
                    detail: Some("HTTP client unavailable".into()),
                };
            };
            match client.get(&url).send().await {
                Ok(response) => {
                    let status = response.status();
                    drop(response);
                    MonitorResult {
                        target,
                        kind: kind.into(),
                        up: status.as_u16() < 400,
                        latency_ms: latency(started),
                        status_code: Some(status.as_u16()),
                        detail: (status.as_u16() >= 400)
                            .then(|| format!("HTTP {}", status.as_u16())),
                    }
                }
                Err(error) => {
                    let detail = if error.is_timeout() {
                        "timed out"
                    } else if error.is_connect() {
                        "connection failed"
                    } else if error.is_redirect() {
                        "redirect"
                    } else {
                        "request failed"
                    };
                    MonitorResult {
                        target,
                        kind: kind.into(),
                        up: false,
                        latency_ms: None,
                        status_code: None,
                        detail: Some(detail.into()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_remove_non_adjacent_duplicate_monitors() {
        let directory = tempfile::tempdir().expect("state");
        tokio::fs::write(
            directory.path().join("settings.json"),
            br#"{"monitors":["tcp://127.0.0.1:1","tcp://127.0.0.1:2","tcp://127.0.0.1:1"]}"#,
        )
        .await
        .expect("settings");
        let settings = Settings::load(directory.path()).await;
        assert_eq!(
            settings.monitors,
            ["tcp://127.0.0.1:1", "tcp://127.0.0.1:2"]
        );
    }
    #[test]
    fn meminfo_cache_sums_cached_buffers_and_reclaimable_slab() {
        let text = "MemTotal:       16479424 kB\nBuffers:            1188 kB\nCached:           179412 kB\nSReclaimable:      20000 kB\n";
        assert_eq!(
            parse_meminfo_cache(text),
            Some((179412u64 + 1188 + 20000) as f64 * 1024.0)
        );
        assert_eq!(parse_meminfo_cache("MemTotal: 1 kB\n"), None);
        assert_eq!(
            parse_arc_size("name type data\nsize 4 123456\nc 4 5\n"),
            Some(123456.0)
        );
        assert_eq!(parse_arc_size("c 4 5\n"), None);
    }
    #[test]
    fn root_mount_prefers_the_last_entry_and_recognises_zfs() {
        let mounts =
            "/dev/sda1 / ext4 rw 0 0\nrpool/ROOT/pve-1 / zfs rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n";
        assert_eq!(
            parse_root_mount(mounts),
            Some(("rpool/ROOT/pve-1".into(), "zfs".into()))
        );
        assert_eq!(parse_root_mount("tmpfs /tmp tmpfs rw 0 0\n"), None);
    }
    #[test]
    fn diskstats_rates_skip_pseudo_devices_and_flag_the_root_disk() {
        let before = "   7 0 loop0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n 259 0 nvme0n1 10 0 1000 5 20 0 2000 7 0 100 12 0 0 0 0 0 0\n";
        let after = "   7 0 loop0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n 259 0 nvme0n1 10 0 4000 5 20 0 5000 7 0 400 12 0 0 0 0 0 0\n";
        let previous: HashMap<_, _> = parse_diskstats(before).into_iter().collect();
        let current: HashMap<_, _> = parse_diskstats(after).into_iter().collect();
        assert_eq!(previous.len(), 1);
        let root = Root {
            device: Some("nvme0n1".into()),
            pool: None,
        };
        let rates = device_rates(&previous, &current, 30.0, &root);
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].read_bytes_per_second, Some(3000.0 * 512.0 / 30.0));
        assert_eq!(rates[0].write_bytes_per_second, Some(3000.0 * 512.0 / 30.0));
        assert_eq!(rates[0].busy_percent, Some(1.0));
        assert!(rates[0].root);
        // Without a previous sample every rate is unavailable, never zero.
        let first = device_rates(&HashMap::new(), &current, 30.0, &root);
        assert_eq!(first[0].read_bytes_per_second, None);
        assert_eq!(first[0].busy_percent, None);
    }
    #[test]
    fn zpool_list_and_objset_counters_parse() {
        let pools = parse_zpool_list(
            "rpool\t1000000\t400000\t600000\tONLINE\ntank\t2000\t3000\t0\tDEGRADED\nbroken line\n",
        );
        assert_eq!(pools.len(), 2);
        assert_eq!(pools[0].name, "rpool");
        assert_eq!(pools[0].health, "ONLINE");
        assert_eq!(
            pools[1].allocated_bytes, 2000.0,
            "allocation never exceeds size"
        );
        let objset = "31 1 0x01 7 2160 1 2\nname type data\ndataset_name 7 rpool/ROOT\nwrites 4 10\nnwritten 4 2048\nreads 4 3\nnread 4 4096\n";
        assert_eq!(parse_objset(objset), Some((4096, 2048)));
        let root = Root {
            device: None,
            pool: Some("rpool".into()),
        };
        let previous = HashMap::from([("rpool".to_string(), (0u64, 0u64))]);
        let current = HashMap::from([("rpool".to_string(), (3000u64, 1500u64))]);
        let result = super::pools(&pools, &previous, &current, 30.0, &root);
        assert_eq!(result[0].read_bytes_per_second, Some(100.0));
        assert_eq!(result[0].write_bytes_per_second, Some(50.0));
        assert!(result[0].root);
        assert_eq!(result[1].read_bytes_per_second, None);
        assert!(!result[1].root);
    }
    #[test]
    fn units_report_failed_and_watched_services_with_counts() {
        let output = "cron.service loaded active running Regular background program processing daemon\n\
nginx.service loaded failed failed A high performance web server\n\
ghost.service not-found inactive dead ghost.service\n\
ssh.service loaded active running OpenBSD Secure Shell server\n";
        let report = parse_units(output, &["ssh".into()]);
        assert_eq!(
            (
                report.summary.total,
                report.summary.active,
                report.summary.failed
            ),
            (3, 2, 1)
        );
        assert_eq!(
            report
                .units
                .iter()
                .map(|u| u.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ssh.service", "nginx.service"]
        );
        assert!(report.units[0].watched && !report.units[1].watched);
        assert_eq!(report.units[1].description, "A high performance web server");
    }
    #[test]
    fn monitor_targets_are_validated() {
        assert_eq!(
            parse_monitor("tcp://10.0.0.1:22"),
            Some(Monitor::Tcp {
                host: "10.0.0.1".into(),
                port: 22
            })
        );
        assert_eq!(
            parse_monitor("tcp://[fe80::1]:443"),
            Some(Monitor::Tcp {
                host: "fe80::1".into(),
                port: 443
            })
        );
        assert!(parse_monitor("tcp://host").is_none());
        assert!(parse_monitor("tcp://host:0").is_none());
        assert!(parse_monitor("ftp://host:21").is_none());
        assert!(parse_monitor("https://user:secret@example.com/").is_none());
        assert!(matches!(
            parse_monitor("https://example.com/health"),
            Some(Monitor::Http { secure: true, .. })
        ));
    }
    #[tokio::test]
    async fn monitors_check_tcp_targets_concurrently_and_report_failures() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let port = listener.local_addr().expect("address").port();
        let closed = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe");
            probe.local_addr().expect("address").port()
        };
        let settings = Settings {
            monitors: vec![
                format!("tcp://127.0.0.1:{port}"),
                format!("tcp://127.0.0.1:{closed}"),
            ],
            ..Default::default()
        };
        let monitors = Monitors::new(&settings);
        assert_eq!(monitors.configured(), 2);
        let results = monitors.sample().await;
        assert_eq!(results.len(), 2);
        assert!(results[0].up && results[0].latency_ms.is_some());
        assert!(!results[1].up && results[1].latency_ms.is_none() && results[1].detail.is_some());
    }
}
