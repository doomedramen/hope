use crate::extras;
use hope_protocol::{
    CollectorStatus, Container, Disk, Gpu, Interface, Inventory, Metrics, NetworkCounter,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use std::{
    ffi::CString,
    mem::MaybeUninit,
    path::{Component, Path, PathBuf},
};
use sysinfo::{Disks, Networks, System};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

#[derive(serde::Deserialize)]
struct DockerContainer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "Ports")]
    ports: String,
}

#[derive(serde::Deserialize)]
struct DockerStats {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "CPUPerc")]
    cpu_percent: String,
    #[serde(rename = "MemUsage")]
    memory_usage: String,
    #[serde(rename = "MemPerc")]
    memory_percent: String,
    #[serde(rename = "NetIO")]
    network_io: String,
    #[serde(rename = "BlockIO")]
    block_io: String,
    #[serde(rename = "PIDs")]
    pids: String,
}

fn docker_percent(value: &str) -> Option<f64> {
    value.trim().strip_suffix('%')?.trim().parse().ok()
}

fn docker_bytes(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let number = value[..split].parse::<f64>().ok()?;
    let unit = value[split..].trim();
    let multiplier = match unit {
        "B" => 1.0,
        "kB" | "KB" => 1_000.0,
        "KiB" => 1_024.0,
        "MB" => 1_000_000.0,
        "MiB" => 1_048_576.0,
        "GB" => 1_000_000_000.0,
        "GiB" => 1_073_741_824.0,
        "TB" => 1_000_000_000_000.0,
        "TiB" => 1_099_511_627_776.0,
        _ => return None,
    };
    Some(number * multiplier)
}

fn docker_pair(value: &str, parse: fn(&str) -> Option<f64>) -> (Option<f64>, Option<f64>) {
    let Some((left, right)) = value.split_once('/') else {
        return (None, None);
    };
    (parse(left), parse(right))
}

fn apply_docker_stats(containers: &mut [Container], output: &str) {
    for stats in output
        .lines()
        .take(128)
        .filter_map(|line| serde_json::from_str::<DockerStats>(line).ok())
    {
        let Some(container) = containers.iter_mut().find(|c| c.name == stats.name) else {
            continue;
        };
        container.cpu_percent = docker_percent(&stats.cpu_percent);
        (container.memory_used_bytes, container.memory_limit_bytes) =
            docker_pair(&stats.memory_usage, docker_bytes);
        container.memory_percent = docker_percent(&stats.memory_percent);
        (container.network_rx_bytes, container.network_tx_bytes) =
            docker_pair(&stats.network_io, docker_bytes);
        (container.block_read_bytes, container.block_write_bytes) =
            docker_pair(&stats.block_io, docker_bytes);
        container.pids = stats.pids.trim().parse().ok();
    }
}

fn parse_docker_output(output: &str) -> Vec<Container> {
    output
        .lines()
        .take(128)
        .filter_map(|line| serde_json::from_str::<DockerContainer>(line).ok())
        .filter(|item| {
            !item.id.is_empty()
                && !item.names.is_empty()
                && !item.image.is_empty()
                && !item.state.is_empty()
        })
        .map(|item| Container {
            id: item.id,
            name: item.names,
            image: item.image,
            state: item.state,
            status: item.status,
            ports: item.ports,
            cpu_percent: None,
            memory_used_bytes: None,
            memory_limit_bytes: None,
            memory_percent: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            block_read_bytes: None,
            block_write_bytes: None,
            pids: None,
        })
        .collect()
}

async fn docker_containers() -> Option<Vec<Container>> {
    let output = command(
        "docker",
        &["ps", "--all", "--no-trunc", "--format", "{{json .}}"],
        2,
    )
    .await?;
    let mut containers = parse_docker_output(&output);
    if containers
        .iter()
        .any(|container| container.state == "running")
        && let Some(output) = command(
            "docker",
            &["stats", "--no-stream", "--format", "{{json .}}"],
            5,
        )
        .await
    {
        apply_docker_stats(&mut containers, &output);
    }
    Some(containers)
}

#[derive(Clone, serde::Serialize)]
pub struct CollectorCost {
    pub collector: &'static str,
    pub milliseconds: f64,
}
pub struct Collector {
    pub costs: Vec<CollectorCost>,
    system: System,
    cpu_probe: CgroupCpuProbe,
    networks: Networks,
    disks: DiskProbe,
    disk_inventory: Vec<Disk>,
    last: Instant,
    previous_network: HashMap<String, (u64, u64, String)>,
    previous_disk: Option<(u64, u64)>,
    gpus: Vec<Gpu>,
    gpu_at: Instant,
    first: bool,
    settings: extras::Settings,
    sensors: extras::Sensors,
    monitors: extras::Monitors,
    previous_devices: HashMap<String, extras::DeviceCounters>,
    previous_pools: HashMap<String, (u64, u64)>,
}

struct DiskProbe {
    receiver: Option<tokio::sync::oneshot::Receiver<Vec<Disk>>>,
    collect: std::sync::Arc<dyn Fn() -> Vec<Disk> + Send + Sync>,
}

#[cfg(any(target_os = "linux", test))]
fn decode_mount_field(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(any(target_os = "linux", test))]
fn root_disk_from_mounts(mounts: &str, total_bytes: u64, available_bytes: u64) -> Option<Disk> {
    let (name, filesystem) = mounts.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        let name = fields.next()?;
        let mount = fields.next()?;
        let filesystem = fields.next()?;
        (decode_mount_field(mount) == "/")
            .then(|| (decode_mount_field(name), filesystem.to_string()))
    })?;
    (total_bytes > 0).then_some(Disk {
        name,
        mount: "/".into(),
        filesystem,
        total_bytes: total_bytes as f64,
        available_bytes: available_bytes.min(total_bytes) as f64,
    })
}

#[cfg(any(target_os = "linux", test))]
fn include_root_disk(disks: &mut Vec<Disk>, root: Option<Disk>) {
    if !disks.iter().any(|disk| disk.mount == "/")
        && let Some(root) = root
    {
        disks.push(root);
    }
}

#[cfg(target_os = "linux")]
fn root_disk() -> Option<Disk> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let mount = CString::new("/").ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(mount.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    let block_size = if stats.f_frsize > 0 {
        stats.f_frsize
    } else {
        stats.f_bsize
    } as u128;
    let total = block_size.saturating_mul(stats.f_blocks as u128);
    let available = block_size.saturating_mul(stats.f_bavail as u128);
    root_disk_from_mounts(
        &mounts,
        total.min(u64::MAX as u128) as u64,
        available.min(u64::MAX as u128) as u64,
    )
}

#[derive(Clone, Copy)]
struct CgroupCpuSample {
    usage_usec: u64,
    cores: f64,
}

#[derive(Default)]
struct CgroupCpuProbe {
    previous: Option<CgroupCpuSample>,
}

impl CgroupCpuProbe {
    fn sample(&mut self, elapsed_seconds: f64, fallback_cores: usize) -> Option<f64> {
        let Some(current) = read_cgroup_cpu_sample(fallback_cores) else {
            self.previous = None;
            return None;
        };
        let previous = self.previous.replace(current)?;
        cpu_percent_from_samples(previous, current, elapsed_seconds)
    }
}

#[cfg(target_os = "linux")]
fn read_cgroup_cpu_sample(fallback_cores: usize) -> Option<CgroupCpuSample> {
    let membership = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let _hierarchy = fields.next()?;
        let controllers = fields.next()?;
        let group = fields.next()?;
        if controllers.is_empty() {
            if let Some(sample) = read_v2_cpu_sample(group, fallback_cores) {
                return Some(sample);
            }
        } else if controllers.split(',').any(|name| name == "cpuacct")
            && let Some(sample) = read_v1_cpu_sample(group, fallback_cores)
        {
            return Some(sample);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_cgroup_cpu_sample(_fallback_cores: usize) -> Option<CgroupCpuSample> {
    None
}

#[cfg(target_os = "linux")]
fn safe_cgroup_path(root: &Path, group: &str) -> Option<PathBuf> {
    let relative = Path::new(group).strip_prefix("/").ok()?;
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative))
}

#[cfg(target_os = "linux")]
fn cgroup_ancestors(path: &Path) -> Vec<&Path> {
    let mut paths = Vec::new();
    let mut current = Some(path);
    while let Some(directory) = current {
        paths.push(directory);
        current = directory.parent();
    }
    paths
}

#[cfg(any(target_os = "linux", test))]
fn parse_cpu_list(value: &str) -> Option<usize> {
    let mut count = 0usize;
    for item in value.trim().split(',').filter(|item| !item.is_empty()) {
        let (first, last) = item
            .split_once('-')
            .map_or((item, item), |(first, last)| (first, last));
        let first = first.parse::<usize>().ok()?;
        let last = last.parse::<usize>().ok()?;
        if last < first {
            return None;
        }
        count = count.checked_add(last - first + 1)?;
    }
    (count > 0).then_some(count)
}

#[cfg(target_os = "linux")]
fn v2_cpu_capacity(path: &Path, fallback_cores: usize) -> Option<f64> {
    let mut capacity: Option<f64> = None;
    for directory in cgroup_ancestors(path) {
        if let Ok(value) = std::fs::read_to_string(directory.join("cpu.max"))
            && let Some((quota, period)) = value.split_once(' ')
            && quota != "max"
        {
            let quota = quota.parse::<f64>().ok()?;
            let period = period.trim().parse::<f64>().ok()?;
            if quota > 0.0 && period > 0.0 {
                let cores = quota / period;
                capacity = Some(capacity.map_or(cores, |current| current.min(cores)));
            }
        }
    }
    let cpus = std::fs::read_to_string(path.join("cpuset.cpus.effective"))
        .ok()
        .and_then(|value| parse_cpu_list(&value))
        .or((fallback_cores > 0).then_some(fallback_cores));
    match (capacity, cpus) {
        (Some(quota), Some(cpus)) => Some(quota.min(cpus as f64)),
        (Some(quota), None) => Some(quota),
        (None, Some(cpus)) => Some(cpus as f64),
        (None, None) => None,
    }
}

#[cfg(target_os = "linux")]
fn read_v2_cpu_sample(group: &str, fallback_cores: usize) -> Option<CgroupCpuSample> {
    let root = Path::new("/sys/fs/cgroup");
    let path = safe_cgroup_path(root, group)?;
    let usage_path = path.join("cpu.stat");
    if !usage_path.exists() && path != root {
        return None;
    }
    let usage_path = if usage_path.exists() {
        usage_path
    } else {
        root.join("cpu.stat")
    };
    let stats = std::fs::read_to_string(usage_path).ok()?;
    let usage_usec = stats.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (key == "usage_usec")
            .then(|| value.trim().parse().ok())
            .flatten()
    })?;
    let cores = v2_cpu_capacity(&path, fallback_cores)?;
    Some(CgroupCpuSample { usage_usec, cores })
}

#[cfg(target_os = "linux")]
fn v1_cpu_capacity(path: &Path, cpuset: &Path, fallback_cores: usize) -> Option<f64> {
    let mut capacity: Option<f64> = None;
    for directory in cgroup_ancestors(path) {
        if let (Ok(quota), Ok(period)) = (
            std::fs::read_to_string(directory.join("cpu.cfs_quota_us")),
            std::fs::read_to_string(directory.join("cpu.cfs_period_us")),
        ) && let (Ok(quota), Ok(period)) =
            (quota.trim().parse::<f64>(), period.trim().parse::<f64>())
            && quota > 0.0
            && period > 0.0
        {
            let cores = quota / period;
            capacity = Some(capacity.map_or(cores, |current| current.min(cores)));
        }
    }
    let cpus = cgroup_ancestors(cpuset)
        .into_iter()
        .find_map(|directory| {
            std::fs::read_to_string(directory.join("cpuset.cpus"))
                .ok()
                .and_then(|value| parse_cpu_list(&value))
        })
        .or((fallback_cores > 0).then_some(fallback_cores));
    match (capacity, cpus) {
        (Some(quota), Some(cpus)) => Some(quota.min(cpus as f64)),
        (Some(quota), None) => Some(quota),
        (None, Some(cpus)) => Some(cpus as f64),
        (None, None) => None,
    }
}

#[cfg(target_os = "linux")]
fn read_v1_cpu_sample(group: &str, fallback_cores: usize) -> Option<CgroupCpuSample> {
    let cpu_paths = [
        Path::new("/sys/fs/cgroup/cpu"),
        Path::new("/sys/fs/cgroup/cpu,cpuacct"),
    ];
    let cpu_path = cpu_paths
        .iter()
        .filter_map(|root| safe_cgroup_path(root, group))
        .find(|path| path.exists())?;
    let cpuacct_path = safe_cgroup_path(Path::new("/sys/fs/cgroup/cpuacct"), group);
    let usage_path = [
        cpuacct_path.map(|path| path.join("cpuacct.usage")),
        Some(cpu_path.join("cpuacct.usage")),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate.exists())?;
    let usage_usec = std::fs::read_to_string(usage_path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        / 1_000;
    let cores = v1_cpu_capacity(
        &cpu_path,
        &Path::new("/sys/fs/cgroup/cpuset").join(group.trim_start_matches('/')),
        fallback_cores,
    )?;
    Some(CgroupCpuSample { usage_usec, cores })
}

fn cpu_percent_from_samples(
    previous: CgroupCpuSample,
    current: CgroupCpuSample,
    elapsed_seconds: f64,
) -> Option<f64> {
    if !elapsed_seconds.is_finite()
        || elapsed_seconds <= 0.0
        || current.usage_usec < previous.usage_usec
        || current.cores <= 0.0
    {
        return None;
    }
    Some(
        ((current.usage_usec - previous.usage_usec) as f64
            / (elapsed_seconds * 1_000_000.0 * current.cores)
            * 100.0)
            .clamp(0.0, 100.0),
    )
}
impl DiskProbe {
    fn new() -> Self {
        Self {
            receiver: None,
            collect: std::sync::Arc::new(|| {
                // A single detached worker may wait on a remote filesystem. It cannot
                // accumulate further workers or prevent process shutdown.
                let disks = Disks::new_with_refreshed_list();
                disks
                    .iter()
                    .take(128)
                    .map(|disk| Disk {
                        name: disk.name().to_string_lossy().into(),
                        mount: disk.mount_point().to_string_lossy().into(),
                        filesystem: disk.file_system().to_string_lossy().into(),
                        total_bytes: disk.total_space() as f64,
                        available_bytes: disk.available_space() as f64,
                    })
                    .collect()
            }),
        }
    }
    async fn sample(&mut self, deadline: Duration) -> Option<Vec<Disk>> {
        if self.receiver.is_none() {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let collect = self.collect.clone();
            std::thread::Builder::new()
                .name("hope-filesystems".into())
                .spawn(move || {
                    let _ = sender.send(collect());
                })
                .ok()?;
            self.receiver = Some(receiver);
        }
        match timeout(deadline, self.receiver.as_mut()?).await {
            Ok(Ok(disks)) => {
                self.receiver = None;
                Some(disks)
            }
            Ok(Err(_)) => {
                self.receiver = None;
                None
            }
            Err(_) => None,
        }
    }
}

pub fn rate(previous: u64, current: u64, seconds: f64) -> Option<f64> {
    if !seconds.is_finite() || seconds <= 0.0 || current < previous {
        None
    } else {
        Some((current - previous) as f64 / seconds)
    }
}
impl Collector {
    pub fn new(settings: extras::Settings) -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self {
            monitors: extras::Monitors::new(&settings),
            settings,
            sensors: extras::Sensors::new(),
            previous_devices: HashMap::new(),
            previous_pools: HashMap::new(),
            costs: Vec::with_capacity(20),
            system,
            cpu_probe: CgroupCpuProbe::default(),
            networks: Networks::new_with_refreshed_list(),
            disks: DiskProbe::new(),
            disk_inventory: vec![],
            last: Instant::now(),
            previous_network: HashMap::new(),
            previous_disk: None,
            gpus: vec![],
            gpu_at: Instant::now() - Duration::from_secs(3601),
            first: true,
        }
    }
    fn cost(&mut self, collector: &'static str, started: Instant) {
        self.costs.push(CollectorCost {
            collector,
            milliseconds: started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    pub async fn sample(&mut self) -> (Inventory, Metrics, Vec<CollectorStatus>) {
        let elapsed = self.last.elapsed().as_secs_f64();
        self.last = Instant::now();
        self.costs.clear();
        let started = Instant::now();
        self.system.refresh_cpu_usage();
        let cpu_cores = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or_else(|_| self.system.cpus().len())
            .max(1);
        self.cost("cpu", started);
        let started = Instant::now();
        self.system.refresh_memory();
        self.cost("memory", started);
        let started = Instant::now();
        self.networks.refresh(true);
        self.cost("network_refresh", started);
        let started = Instant::now();
        let disk_sample = self.disks.sample(Duration::from_millis(500)).await;
        #[cfg(target_os = "linux")]
        let disk_sample = disk_sample.map(|mut disks| {
            include_root_disk(&mut disks, root_disk());
            disks
        });
        self.cost("filesystems", started);
        if let Some(disks) = &disk_sample {
            self.disk_inventory = disks.clone();
        }
        let started = Instant::now();
        let containers = docker_containers().await;
        self.cost("docker_containers", started);
        let started = Instant::now();
        let gateway = default_gateway().await;
        self.cost("gateway", started);
        let started = Instant::now();
        let mut receive = 0.0;
        let mut transmit = 0.0;
        let mut has_rate = true;
        let mut selected = 0;
        let mut network_interfaces = vec![];
        let mut current_network = HashMap::new();
        for (name, network) in self
            .networks
            .iter()
            .filter(|(name, _)| *name != "lo" && *name != "lo0")
            .take(128)
        {
            let counters = (network.total_received(), network.total_transmitted());
            let mac = network.mac_address().to_string();
            let previous = self.previous_network.get(name).filter(|old| old.2 == mac);
            let rx = previous.and_then(|old| rate(old.0, counters.0, elapsed));
            let tx = previous.and_then(|old| rate(old.1, counters.1, elapsed));
            // A default-route interface avoids counting the same traffic again on
            // bridges, tunnel adapters, and their physical underlay. All counters
            // remain available individually, with the total's scope explicit.
            let included = gateway
                .as_ref()
                .map(|(interface, _)| interface == name)
                .unwrap_or_else(|| {
                    ![
                        "veth", "docker", "br-", "bridge", "virbr", "tap", "tun", "utun", "wg",
                        "bond",
                    ]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                });
            if included {
                selected += 1;
                if let (Some(rx), Some(tx)) = (rx, tx) {
                    receive += rx;
                    transmit += tx;
                } else {
                    has_rate = false;
                }
            }
            network_interfaces.push(NetworkCounter {
                name: name.clone(),
                received_bytes: counters.0 as f64,
                transmitted_bytes: counters.1 as f64,
                rx_bytes_per_second: rx,
                tx_bytes_per_second: tx,
                included_in_total: included,
            });
            current_network.insert(name.clone(), (counters.0, counters.1, mac));
        }
        has_rate = has_rate && selected > 0;
        network_interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        self.previous_network = current_network;
        self.cost("network_counters", started);
        let disks = disk_sample.clone().unwrap_or_default();
        let started = Instant::now();
        let io = disk_counters().await;
        self.cost("disk_io", started);
        let disk_rate = self
            .previous_disk
            .zip(io)
            .map(|(old, new)| (rate(old.0, new.0, elapsed), rate(old.1, new.1, elapsed)));
        self.previous_disk = io;
        if self.gpu_at.elapsed() > Duration::from_secs(3600) {
            let started = Instant::now();
            self.gpus = gpu_inventory().await;
            self.cost("gpu_inventory", started);
            self.gpu_at = Instant::now();
        }
        let mut gpus = self.gpus.clone();
        if cfg!(target_os = "linux") {
            let started = Instant::now();
            gpu_metrics(&mut gpus).await;
            self.cost("gpu_metrics", started);
        }
        let gpu_metrics_available = gpus.iter().any(|g| g.utilization_percent.is_some());
        let started = Instant::now();
        let breakdown = extras::memory_breakdown().await;
        let load_average = extras::load_average();
        self.cost("memory_breakdown", started);
        let started = Instant::now();
        let temperatures = self.sensors.sample();
        self.cost("temperatures", started);
        let started = Instant::now();
        let root = extras::root().await;
        let devices = extras::device_counters().await;
        let disk_io = extras::device_rates(&self.previous_devices, &devices, elapsed, &root);
        self.previous_devices = devices;
        self.cost("disk_devices", started);
        let started = Instant::now();
        let capacities = extras::pool_capacities().await;
        let pools = match &capacities {
            Some(capacities) => {
                let counters = extras::pool_counters(capacities).await;
                let pools =
                    extras::pools(capacities, &self.previous_pools, &counters, elapsed, &root);
                self.previous_pools = counters;
                pools
            }
            None => {
                self.previous_pools.clear();
                vec![]
            }
        };
        self.cost("storage_pools", started);
        let started = Instant::now();
        let services = extras::services(&self.settings.watch_services).await;
        self.cost("services", started);
        let started = Instant::now();
        let monitors = if self.monitors.configured() > 0 {
            self.monitors.sample().await
        } else {
            vec![]
        };
        self.cost("monitors", started);
        let mut collectors = vec![
            status(
                "cpu",
                !self.first && !self.system.cpus().is_empty(),
                Some("CPU utilization needs a previous sample"),
            ),
            status("memory", true, None),
            status("network", true, None),
            status(
                "storage",
                disk_sample.is_some(),
                Some(
                    "Filesystem collector is unavailable or still waiting; last inventory is retained",
                ),
            ),
            status(
                "disk_io",
                io.is_some(),
                Some(
                    "Host disk I/O counters are not available on this platform or with these permissions",
                ),
            ),
            status(
                "gpu_inventory",
                !gpus.is_empty(),
                Some("No supported GPU was found; hardware tools or permissions may be missing"),
            ),
            status(
                "gpu_metrics",
                gpu_metrics_available,
                Some("GPU driver metrics are unavailable; inventory remains visible"),
            ),
            status(
                "memory_breakdown",
                breakdown.cached_bytes.is_some(),
                Some("Cache and buffer sizes are only available from /proc/meminfo on Linux"),
            ),
            status(
                "load_average",
                load_average.is_some(),
                Some("Load averages are unavailable on this platform"),
            ),
            status(
                "temperatures",
                !temperatures.is_empty(),
                Some("No temperature sensor is readable (hwmon on Linux, SMC on macOS)"),
            ),
            status(
                "disk_devices",
                !disk_io.is_empty(),
                Some("Per-device throughput needs /proc/diskstats (Linux)"),
            ),
            status(
                "storage_pools",
                capacities.is_some(),
                Some("No ZFS pools: zpool is missing or not permitted for this user"),
            ),
            status(
                "services",
                services.is_some(),
                Some("Service units need systemd (systemctl list-units)"),
            ),
            status(
                "docker_containers",
                containers.is_some(),
                Some("Docker CLI is unavailable or the daemon cannot be reached"),
            ),
        ];
        if self.monitors.configured() > 0 {
            collectors.push(status(
                "monitors",
                monitors.len() == self.monitors.configured(),
                Some("Some configured monitors could not be checked"),
            ));
        }
        let started = Instant::now();
        let interfaces = interfaces(&self.networks, gateway);
        let inventory = Inventory {
            hostname: System::host_name().unwrap_or_else(|| "Unnamed host".into()),
            os: format!(
                "{} {}",
                System::name().unwrap_or_default(),
                System::os_version().unwrap_or_default()
            ),
            architecture: std::env::consts::ARCH.into(),
            cpu_model: self
                .system
                .cpus()
                .first()
                .map(|c| c.brand().to_string())
                .unwrap_or_else(|| "Unknown CPU".into()),
            cpu_cores: cpu_cores as u32,
            memory_total_bytes: self.system.total_memory() as f64,
            interfaces,
            disks: self.disk_inventory.clone(),
            gpus: gpus.clone(),
            collectors: collectors.clone(),
        };
        let metrics = Metrics {
            cpu_percent: (!self.first && !self.system.cpus().is_empty()).then(|| {
                self.cpu_probe
                    .sample(elapsed, cpu_cores)
                    .unwrap_or_else(|| self.system.global_cpu_usage() as f64)
            }),
            memory_used_bytes: Some(self.system.used_memory() as f64),
            memory_total_bytes: Some(self.system.total_memory() as f64),
            memory_cached_bytes: breakdown.cached_bytes,
            memory_zfs_arc_bytes: breakdown.zfs_arc_bytes,
            swap_used_bytes: (self.system.total_swap() > 0).then(|| self.system.used_swap() as f64),
            swap_total_bytes: (self.system.total_swap() > 0)
                .then(|| self.system.total_swap() as f64),
            load_average,
            network_rx_bytes_per_second: has_rate.then_some(receive),
            network_tx_bytes_per_second: has_rate.then_some(transmit),
            network_interfaces,
            disk_read_bytes_per_second: disk_rate.and_then(|r| r.0),
            disk_write_bytes_per_second: disk_rate.and_then(|r| r.1),
            disks,
            disk_io,
            pools,
            gpus,
            temperatures,
            service_summary: services.as_ref().map(|s| s.summary),
            services: services.map(|s| s.units).unwrap_or_default(),
            monitors,
            containers: containers.unwrap_or_default(),
        };
        self.cost("inventory", started);
        self.first = false;
        (inventory, metrics, collectors)
    }
}
fn status(name: &str, available: bool, detail: Option<&str>) -> CollectorStatus {
    CollectorStatus {
        name: name.into(),
        available,
        detail: if available {
            None
        } else {
            detail.map(String::from)
        },
    }
}

pub fn stable_inventory(mut inventory: Inventory) -> Inventory {
    for disk in &mut inventory.disks {
        disk.available_bytes = 0.0;
    }
    for gpu in &mut inventory.gpus {
        gpu.utilization_percent = None;
        gpu.memory_used_bytes = None;
        gpu.temperature_celsius = None;
    }
    inventory
        .disks
        .sort_by(|a, b| (&a.name, &a.mount).cmp(&(&b.name, &b.mount)));
    inventory
        .gpus
        .sort_by(|a, b| (&a.id, &a.vendor, &a.name).cmp(&(&b.id, &b.vendor, &b.name)));
    for interface in &mut inventory.interfaces {
        interface.addresses.sort();
    }
    inventory.interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    inventory.collectors.sort_by(|a, b| a.name.cmp(&b.name));
    inventory
}

fn interfaces(networks: &Networks, gateway: Option<(String, String)>) -> Vec<Interface> {
    let addresses = if_addrs::get_if_addrs().unwrap_or_default();
    let mut interfaces: Vec<_> = networks
        .iter()
        .filter(|(name, _)| *name != "lo" && *name != "lo0")
        .take(128)
        .map(|(name, data)| Interface {
            name: name.clone(),
            mac: (!data.mac_address().is_unspecified()).then(|| data.mac_address().to_string()),
            addresses: addresses
                .iter()
                .filter(|a| a.name == *name)
                .map(|a| a.ip().to_string())
                .collect(),
            gateway: gateway
                .as_ref()
                .filter(|(interface, _)| interface == name)
                .map(|(_, ip)| ip.clone()),
        })
        .collect();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}
async fn default_gateway() -> Option<(String, String)> {
    if cfg!(target_os = "linux") {
        let text = tokio::fs::read_to_string("/proc/net/route").await.ok()?;
        for line in text.lines().skip(1) {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() > 3 && parts[1] == "00000000" {
                let value = u32::from_str_radix(parts[2], 16).ok()?;
                return Some((
                    parts[0].into(),
                    std::net::Ipv4Addr::from(value.to_le_bytes()).to_string(),
                ));
            }
        }
        None
    } else {
        let output = command("/sbin/route", &["-n", "get", "default"], 2).await?;
        let field = |key: &str| {
            output
                .lines()
                .find_map(|l| l.trim().strip_prefix(key).map(|s| s.trim().to_string()))
        };
        Some((field("interface:")?, field("gateway:")?))
    }
}
async fn disk_counters() -> Option<(u64, u64)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let data = tokio::fs::read_to_string("/proc/diskstats").await.ok()?;
    let mut read = 0u64;
    let mut write = 0u64;
    let mut found = false;
    for line in data.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        if p.len() < 14 {
            continue;
        }
        if p[2].starts_with("loop")
            || p[2].starts_with("ram")
            || p[2].starts_with("dm-")
            || p[2].starts_with("md")
            || p[2].starts_with("zram")
        {
            continue;
        }
        if tokio::fs::try_exists(format!("/sys/block/{}", p[2]))
            .await
            .unwrap_or(false)
        {
            read = read.saturating_add(p[5].parse::<u64>().ok()?.saturating_mul(512));
            write = write.saturating_add(p[9].parse::<u64>().ok()?.saturating_mul(512));
            found = true;
        }
    }
    found.then_some((read, write))
}
pub(crate) async fn command(binary: &str, args: &[&str], seconds: u64) -> Option<String> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let result = timeout(Duration::from_secs(seconds), async {
        let mut bytes = vec![];
        child
            .stdout
            .take()?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 1024 * 1024 {
            return None;
        }
        if !child.wait().await.ok()?.success() {
            return None;
        }
        String::from_utf8(bytes).ok()
    })
    .await;
    match result {
        Ok(Some(output)) => Some(output),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    }
}

async fn gpu_inventory() -> Vec<Gpu> {
    let mut gpus = vec![];
    if cfg!(target_os = "macos") {
        if let Some(output) = command(
            "/usr/sbin/system_profiler",
            &["SPDisplaysDataType", "-json", "-detailLevel", "mini"],
            5,
        )
        .await
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&output)
            && let Some(devices) = value["SPDisplaysDataType"].as_array()
        {
            for device in devices.iter().take(32) {
                gpus.push(Gpu {
                    id: device["sppci_slot_name"]
                        .as_str()
                        .map(|slot| format!("macos-slot:{slot}")),
                    name: device["sppci_model"].as_str().unwrap_or("GPU").into(),
                    vendor: device["spdisplays_vendor"]
                        .as_str()
                        .unwrap_or("Unknown")
                        .into(),
                    utilization_percent: None,
                    memory_used_bytes: None,
                    memory_total_bytes: None,
                    temperature_celsius: None,
                });
            }
        }
    } else if let Ok(mut entries) = tokio::fs::read_dir("/sys/class/drm").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || !name[4..].chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let path = entry.path().join("device");
            let vendor = tokio::fs::read_to_string(path.join("vendor"))
                .await
                .unwrap_or_default();
            let device = tokio::fs::read_to_string(path.join("device"))
                .await
                .unwrap_or_default();
            let vendor_name = match vendor.trim() {
                "0x10de" => "NVIDIA",
                "0x1002" => "AMD",
                "0x8086" => "Intel",
                _ => "Unknown",
            };
            let canonical = tokio::fs::canonicalize(&path).await.unwrap_or(path.clone());
            let id = format!(
                "sysfs:{:x}",
                Sha256::digest(canonical.to_string_lossy().as_bytes())
            );
            gpus.push(Gpu {
                id: Some(id),
                name: format!("{vendor_name} {} ({name})", device.trim()),
                vendor: vendor_name.into(),
                utilization_percent: None,
                memory_used_bytes: None,
                memory_total_bytes: None,
                temperature_celsius: None,
            });
            if gpus.len() >= 32 {
                break;
            }
        }
    }
    gpus.sort_by(|a, b| (&a.id, &a.name).cmp(&(&b.id, &b.name)));
    gpus
}
async fn gpu_metrics(gpus: &mut Vec<Gpu>) {
    if let Some(output) = command(
        "nvidia-smi",
        &[
            "--query-gpu=uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
        2,
    )
    .await
    {
        gpus.retain(|g| g.vendor != "NVIDIA");
        gpus.extend(parse_nvidia(&output));
    }
    for gpu in gpus.iter_mut().filter(|g| g.vendor == "AMD") {
        let Some(card) = gpu.name.split('(').nth(1).and_then(|s| s.strip_suffix(')')) else {
            continue;
        };
        let base = format!("/sys/class/drm/{card}/device");
        gpu.utilization_percent = read_number(&format!("{base}/gpu_busy_percent")).await;
        gpu.memory_used_bytes = read_number(&format!("{base}/mem_info_vram_used")).await;
        gpu.memory_total_bytes = read_number(&format!("{base}/mem_info_vram_total")).await;
        if let Ok(mut entries) = tokio::fs::read_dir(format!("{base}/hwmon")).await
            && let Ok(Some(entry)) = entries.next_entry().await
        {
            gpu.temperature_celsius =
                read_number(&entry.path().join("temp1_input").to_string_lossy())
                    .await
                    .map(|v| v / 1000.0);
        }
    }
}
fn parse_nvidia(output: &str) -> Vec<Gpu> {
    output
        .lines()
        .take(32)
        .filter_map(|line| {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            if fields.len() != 6 {
                return None;
            }
            let number = |field: &str| field.parse::<f64>().ok().filter(|value| value.is_finite());
            Some(Gpu {
                id: fields[0]
                    .starts_with("GPU-")
                    .then(|| format!("nvidia:{}", fields[0])),
                name: fields[1].into(),
                vendor: "NVIDIA".into(),
                utilization_percent: number(fields[2]).filter(|v| (0.0..=100.0).contains(v)),
                memory_used_bytes: number(fields[3])
                    .filter(|v| *v >= 0.0)
                    .map(|v| v * 1048576.0),
                memory_total_bytes: number(fields[4])
                    .filter(|v| *v >= 0.0)
                    .map(|v| v * 1048576.0),
                temperature_celsius: number(fields[5]),
            })
        })
        .collect()
}

async fn read_number(path: &str) -> Option<f64> {
    tokio::fs::read_to_string(path)
        .await
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lxc_cpu_usage_is_normalized_to_its_allowed_cpus() {
        let previous = CgroupCpuSample {
            usage_usec: 1_000_000,
            cores: 3.0,
        };
        let current = CgroupCpuSample {
            usage_usec: 1_141_600,
            cores: 3.0,
        };
        assert_eq!(cpu_percent_from_samples(previous, current, 1.0), Some(4.72));
        assert_eq!(parse_cpu_list("0-2"), Some(3));
        assert_eq!(parse_cpu_list("0-1,4"), Some(3));
    }

    #[test]
    fn root_filesystem_can_be_recovered_from_a_complete_mount_list() {
        let mounts = "shared /mnt/shared nfs4 rw 0 0\n/dev/mapper/vg\\040root / ext4 rw 0 0\n";
        let root = root_disk_from_mounts(mounts, 30 * 1024_u64.pow(3), 13 * 1024_u64.pow(3))
            .expect("root mount should be found");
        assert_eq!(root.name, "/dev/mapper/vg root");
        assert_eq!(root.filesystem, "ext4");
        assert_eq!(root.mount, "/");

        let shared = Disk {
            name: "shared".into(),
            mount: "/mnt/shared".into(),
            filesystem: "nfs4".into(),
            total_bytes: 100.0,
            available_bytes: 96.0,
        };
        let mut disks = vec![shared];
        include_root_disk(&mut disks, Some(root));
        assert!(disks.iter().any(|disk| disk.mount == "/"));
    }

    #[test]
    fn docker_output_keeps_container_fields_and_ignores_bad_lines() {
        let containers = parse_docker_output(
            r#"{"ID":"abc123","Names":"web","Image":"nginx:latest","State":"running","Status":"Up 2 hours","Ports":"0.0.0.0:8080->80/tcp"}
not json
{"ID":"def456","Names":"db","Image":"postgres:16","State":"exited","Status":"Exited (0)","Ports":""}"#,
        );
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "web");
        assert_eq!(containers[0].image, "nginx:latest");
        assert_eq!(containers[0].ports, "0.0.0.0:8080->80/tcp");
        assert_eq!(containers[1].state, "exited");
        assert_eq!(containers[0].cpu_percent, None);
    }

    #[test]
    fn docker_stats_attach_current_usage_to_running_containers() {
        let mut containers = parse_docker_output(
            r#"{"ID":"abc123","Names":"web","Image":"nginx:latest","State":"running","Status":"Up 2 hours","Ports":""}
{"ID":"def456","Names":"db","Image":"postgres:16","State":"exited","Status":"Exited (0)","Ports":""}"#,
        );
        apply_docker_stats(
            &mut containers,
            r#"{"Name":"web","CPUPerc":"0.50%","MemUsage":"12.5MiB / 1GiB","MemPerc":"1.22%","NetIO":"1.5kB / 2MiB","BlockIO":"3MB / 4KiB","PIDs":"7"}"#,
        );
        assert_eq!(containers[0].cpu_percent, Some(0.5));
        assert_eq!(containers[0].memory_used_bytes, Some(12.5 * 1_048_576.0));
        assert_eq!(containers[0].memory_limit_bytes, Some(1_073_741_824.0));
        assert_eq!(containers[0].network_rx_bytes, Some(1_500.0));
        assert_eq!(containers[0].block_write_bytes, Some(4_096.0));
        assert_eq!(containers[0].pids, Some(7));
        assert_eq!(containers[1].cpu_percent, None);
    }
    #[tokio::test]
    async fn hanging_filesystems_do_not_accumulate_workers() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        let mut probe = DiskProbe {
            receiver: None,
            collect: Arc::new(move || {
                called.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                vec![]
            }),
        };
        for _ in 0..5 {
            assert!(probe.sample(Duration::from_millis(2)).await.is_none());
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(probe.sample(Duration::from_secs(1)).await.is_some());
    }
    #[tokio::test]
    async fn commands_are_killed_on_deadline_or_excess_output() {
        let start = Instant::now();
        assert!(command("sh", &["-c", "exec sleep 10"], 0).await.is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(
            command("sh", &["-c", "exec yes oversized-output"], 1)
                .await
                .is_none()
        );
    }
    #[test]
    fn duplicate_gpu_models_have_distinct_driver_ids_and_missing_values() {
        let gpus = parse_nvidia(
            "GPU-first, Same model, 10, 100, 1000, 50\nGPU-second, Same model, N/A, N/A, 1000, N/A",
        );
        assert_eq!(gpus.len(), 2);
        assert_ne!(gpus[0].id, gpus[1].id);
        assert!(gpus[1].utilization_percent.is_none());
        assert!(gpus[1].memory_used_bytes.is_none());
    }
    #[test]
    fn counter_reset_is_unavailable() {
        assert_eq!(rate(100, 50, 30.0), None);
        assert_eq!(rate(100, 400, 30.0), Some(10.0));
        assert_eq!(rate(0, 1, 0.0), None);
    }
}
