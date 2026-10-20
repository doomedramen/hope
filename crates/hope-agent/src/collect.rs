use crate::{cpu::CgroupCpuProbe, extras};
use bollard::{
    Docker,
    query_parameters::{ListContainersOptionsBuilder, StatsOptionsBuilder},
};
use futures_util::{StreamExt, stream};
use hope_protocol::{
    ChildDeviceKind, ChildDeviceReport, ChildMetrics, CollectorStatus, Container, ContainerNetwork,
    ContainerPort, CpuMeasurement, DeviceLifecycle, Disk, Gpu, Interface, Inventory,
    MemoryMeasurement, Metrics, NetworkCounter, SourceIdentity,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime},
};
#[cfg(target_os = "linux")]
use std::{ffi::CString, mem::MaybeUninit};
use sysinfo::{Disks, Networks, System};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

fn clean_option(value: Option<&str>, max: usize) -> Option<String> {
    value
        .map(|value| extras::clean(value, max))
        .filter(|value| !value.is_empty())
}

fn docker_summary(item: bollard::models::ContainerSummary) -> Option<Container> {
    let id = clean_option(item.id.as_deref(), 128)?;
    let name = item
        .names
        .as_ref()?
        .first()
        .map(|name| name.trim_start_matches('/'))?;
    if name.is_empty() {
        return None;
    }
    let labels = item.labels.as_ref();
    let mut networks = item
        .network_settings
        .as_ref()
        .and_then(|settings| settings.networks.as_ref())
        .map(|networks| {
            networks
                .iter()
                .map(|(name, endpoint)| ContainerNetwork {
                    name: extras::clean(name, 128),
                    network_id: clean_option(endpoint.network_id.as_deref(), 128),
                    ipv4: clean_option(endpoint.ip_address.as_deref(), 64),
                    ipv6: clean_option(endpoint.global_ipv6_address.as_deref(), 128),
                    mac: clean_option(endpoint.mac_address.as_deref(), 32),
                    gateway: clean_option(endpoint.gateway.as_deref(), 64),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    networks.sort_by(|a, b| a.name.cmp(&b.name));
    networks.truncate(16);
    let mut published_ports = item
        .ports
        .unwrap_or_default()
        .into_iter()
        .map(|port| ContainerPort {
            host_ip: clean_option(port.ip.as_deref(), 64),
            container_port: port.private_port,
            host_port: port.public_port,
            protocol: port
                .typ
                .map(|kind| kind.to_string())
                .unwrap_or_else(|| "tcp".into()),
        })
        .collect::<Vec<_>>();
    published_ports.sort_by(|a, b| {
        (
            a.host_ip.as_deref(),
            a.host_port,
            a.container_port,
            &a.protocol,
        )
            .cmp(&(
                b.host_ip.as_deref(),
                b.host_port,
                b.container_port,
                &b.protocol,
            ))
    });
    published_ports.truncate(64);
    let state = item
        .state
        .map(|state| format!("{state:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let health = item.health.as_ref().and_then(|health| {
        health
            .status
            .map(|status| format!("{status:?}").to_ascii_lowercase())
    });
    let compose_project = clean_option(
        labels.and_then(|labels| labels.get("com.docker.compose.project").map(String::as_str)),
        128,
    );
    let compose_service = clean_option(
        labels.and_then(|labels| labels.get("com.docker.compose.service").map(String::as_str)),
        128,
    );
    Some(Container {
        id,
        name: extras::clean(name, 256),
        image: extras::clean(
            item.image
                .as_deref()
                .filter(|value| !value.is_empty())
                .or(item.image_id.as_deref())
                .unwrap_or("unknown"),
            512,
        ),
        state,
        status: extras::clean(item.status.as_deref().unwrap_or(""), 256),
        ports: published_ports
            .iter()
            .map(|port| match (&port.host_ip, port.host_port) {
                (Some(ip), Some(host)) => {
                    format!("{ip}:{host}->{}:{}", port.container_port, port.protocol)
                }
                (_, Some(host)) => format!("{host}->{}:{}", port.container_port, port.protocol),
                _ => format!("{}:{}", port.container_port, port.protocol),
            })
            .collect::<Vec<_>>()
            .join(", "),
        created_at: item.created.map(|created| created as f64),
        health,
        compose_project,
        compose_service,
        networks,
        published_ports,
        restart_count: None,
        cpu_percent: None,
        cpu_capacity_cores: None,
        memory_used_bytes: None,
        memory_limit_bytes: None,
        memory_percent: None,
        network_rx_bytes: None,
        network_tx_bytes: None,
        block_read_bytes: None,
        block_write_bytes: None,
        pids: None,
    })
}

static DOCKER_INSPECT_CURSOR: AtomicUsize = AtomicUsize::new(0);
const MAX_DOCKER_INSPECTS_PER_REPORT: usize = 32;
const DOCKER_INSPECT_ROTATION_STEP: usize = 8;

fn docker_inspect_indices(container_count: usize, cursor: usize) -> Vec<usize> {
    let count = container_count.min(MAX_DOCKER_INSPECTS_PER_REPORT);
    if container_count == 0 {
        return vec![];
    }
    (0..count)
        .map(|offset| (cursor.wrapping_add(offset)) % container_count)
        .collect()
}

async fn docker_restart_count(docker: &Docker, id: String) -> (String, Option<u32>) {
    let result = timeout(Duration::from_secs(2), docker.inspect_container(&id, None))
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|response| docker_restart_count_value(response.restart_count));
    (id, result)
}

fn docker_restart_count_value(count: Option<i64>) -> Option<u32> {
    count.and_then(|count| u32::try_from(count).ok())
}

fn docker_cpu_percent(stats: &bollard::models::ContainerStatsResponse) -> Option<f64> {
    let current = stats.cpu_stats.as_ref()?;
    let previous = stats.precpu_stats.as_ref()?;
    let cpu_delta = current
        .cpu_usage
        .as_ref()?
        .total_usage?
        .checked_sub(previous.cpu_usage.as_ref()?.total_usage?)?;
    let system_delta = current
        .system_cpu_usage?
        .checked_sub(previous.system_cpu_usage?)?;
    let cpu_count = current.online_cpus.unwrap_or_else(|| {
        current
            .cpu_usage
            .as_ref()
            .and_then(|usage| usage.percpu_usage.as_ref())
            .map(Vec::len)
            .unwrap_or(0) as u32
    });
    (system_delta > 0 && cpu_count > 0)
        .then(|| cpu_delta as f64 / system_delta as f64 * cpu_count as f64 * 100.0)
        .filter(|value| value.is_finite() && (0.0..=100_000.0).contains(value))
}

fn apply_docker_api_stats(
    container: &mut Container,
    stats: &bollard::models::ContainerStatsResponse,
) {
    container.cpu_percent = docker_cpu_percent(stats);
    container.cpu_capacity_cores = stats
        .cpu_stats
        .as_ref()
        .and_then(|cpu| cpu.online_cpus)
        .map(f64::from);
    if let Some(memory) = stats.memory_stats.as_ref() {
        let inactive = memory
            .stats
            .as_ref()
            .and_then(|values| values.get("inactive_file").or_else(|| values.get("cache")))
            .copied()
            .unwrap_or(0);
        container.memory_used_bytes = memory
            .usage
            .map(|usage| usage.saturating_sub(inactive) as f64);
        container.memory_limit_bytes = memory.limit.map(|limit| limit as f64);
        container.memory_percent = container
            .memory_used_bytes
            .zip(container.memory_limit_bytes)
            .filter(|(_, limit)| *limit > 0.0)
            .map(|(used, limit)| used / limit * 100.0);
    }
    if let Some(networks) = stats.networks.as_ref() {
        let (rx, tx) = networks
            .values()
            .fold((Some(0u64), Some(0u64)), |(rx, tx), net| {
                (
                    rx.zip(net.rx_bytes)
                        .map(|(sum, value)| sum.saturating_add(value)),
                    tx.zip(net.tx_bytes)
                        .map(|(sum, value)| sum.saturating_add(value)),
                )
            });
        container.network_rx_bytes = rx.map(|value| value as f64);
        container.network_tx_bytes = tx.map(|value| value as f64);
    }
    if let Some(entries) = stats
        .blkio_stats
        .as_ref()
        .and_then(|io| io.io_service_bytes_recursive.as_ref())
    {
        let mut read = 0u64;
        let mut write = 0u64;
        let mut has_read = false;
        let mut has_write = false;
        for entry in entries {
            match entry.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("read") => {
                    read = read.saturating_add(entry.value.unwrap_or_default());
                    has_read = true;
                }
                Some("write") => {
                    write = write.saturating_add(entry.value.unwrap_or_default());
                    has_write = true;
                }
                _ => {}
            }
        }
        container.block_read_bytes = has_read.then_some(read as f64);
        container.block_write_bytes = has_write.then_some(write as f64);
    }
    container.pids = stats
        .pids_stats
        .as_ref()
        .and_then(|pids| pids.current)
        .and_then(|pids| u32::try_from(pids).ok());
}

async fn docker_stats(
    docker: &Docker,
    id: String,
) -> (String, Option<bollard::models::ContainerStatsResponse>) {
    let options = StatsOptionsBuilder::default()
        .stream(false)
        .one_shot(true)
        .build();
    let result = timeout(Duration::from_secs(3), async {
        docker.stats(&id, Some(options)).next().await?.ok()
    })
    .await
    .ok()
    .flatten();
    (id, result)
}

async fn docker_containers() -> Option<(String, Vec<Container>, bool)> {
    let docker = Docker::connect_with_local_defaults()
        .ok()?
        .with_timeout(Duration::from_secs(3));
    let scope = timeout(Duration::from_secs(3), docker.info())
        .await
        .ok()?
        .ok()?
        .id?;
    let scope = extras::clean(&scope, 128);
    if scope.is_empty() {
        return None;
    }
    const MAX_DOCKER_CONTAINERS: usize = 128;
    let options = ListContainersOptionsBuilder::default()
        .all(true)
        .limit(MAX_DOCKER_CONTAINERS as i32 + 1)
        .build();
    let listed = timeout(
        Duration::from_secs(3),
        docker.list_containers(Some(options)),
    )
    .await
    .ok()?
    .ok()?;
    let source_count = listed.len();
    let mut containers = listed
        .into_iter()
        .take(MAX_DOCKER_CONTAINERS)
        .filter_map(docker_summary)
        .collect::<Vec<_>>();
    let complete = docker_inventory_complete(source_count, containers.len(), MAX_DOCKER_CONTAINERS);
    let inspect_cursor =
        DOCKER_INSPECT_CURSOR.fetch_add(DOCKER_INSPECT_ROTATION_STEP, Ordering::Relaxed);
    let inspect_indices = docker_inspect_indices(containers.len(), inspect_cursor);
    let inspect_ids = inspect_indices
        .iter()
        .map(|index| containers[*index].id.clone())
        .collect::<Vec<_>>();
    let inspected = timeout(
        Duration::from_secs(5),
        stream::iter(inspect_ids)
            .map(|id| docker_restart_count(&docker, id))
            .buffer_unordered(8)
            .collect::<Vec<_>>(),
    );
    let running = containers
        .iter()
        .filter(|container| container.state == "running")
        .take(64)
        .map(|container| container.id.clone())
        .collect::<Vec<_>>();
    let samples = timeout(
        Duration::from_secs(5),
        stream::iter(running)
            .map(|id| docker_stats(&docker, id))
            .buffer_unordered(8)
            .collect::<Vec<_>>(),
    );
    let (inspected, samples) = tokio::join!(inspected, samples);
    for (id, restart_count) in inspected.unwrap_or_default() {
        if let Some(container) = containers.iter_mut().find(|container| container.id == id) {
            container.restart_count = restart_count;
        }
    }
    let samples = samples.unwrap_or_default();
    for (id, stats) in samples {
        if let (Some(stats), Some(container)) = (
            stats,
            containers.iter_mut().find(|container| container.id == id),
        ) {
            apply_docker_api_stats(container, &stats);
        }
    }
    Some((scope, containers, complete))
}

fn docker_inventory_complete(source_count: usize, parsed_count: usize, limit: usize) -> bool {
    source_count <= limit && parsed_count == source_count
}

fn docker_child(container: &Container, scope: &str) -> ChildDeviceReport {
    let interfaces = container
        .networks
        .iter()
        .map(|network| Interface {
            name: network.name.clone(),
            mac: network.mac.clone(),
            addresses: network
                .ipv4
                .iter()
                .chain(network.ipv6.iter())
                .cloned()
                .collect(),
            gateway: network.gateway.clone(),
        })
        .collect();
    ChildDeviceReport {
        identity: SourceIdentity {
            kind: ChildDeviceKind::DockerContainer,
            scope: scope.into(),
            key: container.id.clone(),
            incarnation: container
                .created_at
                .map(|created| format!("{created:.0}"))
                .unwrap_or_else(|| container.id.clone()),
        },
        ownership_generation: None,
        legacy_incarnation: None,
        name: container.name.clone(),
        lifecycle: if container.state == "running" {
            DeviceLifecycle::Running
        } else {
            DeviceLifecycle::Stopped
        },
        status: Some(container.status.clone()),
        health: container.health.clone(),
        restart_count: container.restart_count,
        parent_key: None,
        node: None,
        tags: vec![],
        interfaces,
        cpu_cores: container.cpu_capacity_cores,
        memory_bytes: container.memory_limit_bytes,
        image: Some(container.image.clone()),
        compose_project: container.compose_project.clone(),
        compose_service: container.compose_service.clone(),
        published_ports: container.published_ports.clone(),
        guest_agent_status: None,
        guest_os: None,
        guest_filesystems: vec![],
        network_configuration: vec![],
        disk_configuration: vec![],
        metrics: ChildMetrics {
            cpu_percent: container.cpu_percent,
            cpu_capacity_cores: container.cpu_capacity_cores,
            cpu_measurement: Some(CpuMeasurement::DockerEngine),
            memory_used_bytes: container.memory_used_bytes,
            memory_total_bytes: container.memory_limit_bytes,
            memory_measurement: Some(MemoryMeasurement::DockerWorkingSet),
            network_rx_bytes: container.network_rx_bytes,
            network_tx_bytes: container.network_tx_bytes,
            ..Default::default()
        },
    }
}

#[derive(Clone)]
pub struct ChildSnapshot {
    pub source: hope_protocol::ChildSourceKind,
    pub source_scope: String,
    pub source_token: Option<String>,
    pub legacy_source_scope: Option<String>,
    pub legacy_identities: Vec<hope_protocol::LegacySourceIdentity>,
    pub observed_at: f64,
    pub devices: Vec<ChildDeviceReport>,
    pub inventory_complete: bool,
}

#[derive(Clone, serde::Serialize)]
pub struct CollectorCost {
    pub collector: &'static str,
    pub milliseconds: f64,
}
pub struct Collector {
    pub costs: Vec<CollectorCost>,
    system: System,
    memory_total_bytes: f64,
    cpu_probe: CgroupCpuProbe,
    guest_cpu: Option<bool>,
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
    docker_scope: Option<String>,
    child_devices: Vec<ChildDeviceReport>,
    child_inventory_complete: bool,
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
        name: extras::clean(&name, 255),
        mount: "/".into(),
        filesystem: extras::clean(&filesystem, 128),
        total_bytes: total_bytes as f64,
        available_bytes: available_bytes.min(total_bytes) as f64,
    })
}

#[cfg(any(target_os = "linux", test))]
fn include_root_disk(disks: &mut Vec<Disk>, root: Option<Disk>) {
    if !disks.iter().any(|disk| disk.mount == "/")
        && let Some(root) = root
    {
        if disks.len() >= 128 {
            disks.pop();
        }
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

async fn is_container() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    if tokio::fs::try_exists("/run/systemd/container")
        .await
        .unwrap_or(false)
        || tokio::fs::try_exists("/.dockerenv").await.unwrap_or(false)
    {
        return true;
    }
    command("systemd-detect-virt", &["--container"], 2)
        .await
        .is_some()
}

impl DiskProbe {
    fn new() -> Self {
        Self {
            receiver: None,
            collect: std::sync::Arc::new(|| {
                // A single detached worker may wait on a remote filesystem. It cannot
                // accumulate further workers or prevent process shutdown.
                let disks = Disks::new_with_refreshed_list();
                let collected = disks
                    .iter()
                    .take(128)
                    .map(|disk| Disk {
                        name: extras::clean(&disk.name().to_string_lossy(), 255),
                        mount: extras::clean(&disk.mount_point().to_string_lossy(), 4096),
                        filesystem: extras::clean(&disk.file_system().to_string_lossy(), 128),
                        total_bytes: disk.total_space() as f64,
                        available_bytes: disk.available_space() as f64,
                    })
                    .collect::<Vec<_>>();
                #[cfg(target_os = "linux")]
                {
                    let mut collected = collected;
                    include_root_disk(&mut collected, root_disk());
                    collected
                }
                #[cfg(not(target_os = "linux"))]
                collected
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
        Self {
            monitors: extras::Monitors::new(&settings),
            settings,
            sensors: extras::Sensors::new(),
            previous_devices: HashMap::new(),
            previous_pools: HashMap::new(),
            docker_scope: None,
            child_devices: vec![],
            child_inventory_complete: false,
            costs: Vec::with_capacity(20),
            system,
            memory_total_bytes: 0.0,
            cpu_probe: CgroupCpuProbe::default(),
            guest_cpu: None,
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
        let guest_cpu = match self.guest_cpu {
            Some(guest) => guest,
            None => {
                let guest = is_container().await;
                self.guest_cpu = Some(guest);
                guest
            }
        };
        let started = Instant::now();
        self.system.refresh_cpu_usage();
        let cpu_cores = if guest_cpu {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or_else(|_| self.system.cpus().len())
        } else {
            self.system.cpus().len()
        }
        .max(1);
        // Prime the chosen source on sample one, and never fall back to a different scope.
        let cpu_percent = if guest_cpu {
            self.cpu_probe.sample(cpu_cores)
        } else {
            (!self.first && !self.system.cpus().is_empty())
                .then(|| self.system.global_cpu_usage() as f64)
        };
        self.cost("cpu", started);
        let started = Instant::now();
        let memory = crate::memory::sample();
        if let Some(total) = memory.total {
            self.memory_total_bytes = total;
        }
        self.cost("memory", started);
        let started = Instant::now();
        // A fresh snapshot also clears removed addresses. sysinfo 0.37's
        // in-place refresh can retain old IPs when an interface loses its last one.
        // Our rate baselines are held separately in previous_network.
        self.networks = Networks::new_with_refreshed_list();
        self.cost("network_refresh", started);
        let started = Instant::now();
        let disk_sample = self.disks.sample(Duration::from_millis(500)).await;
        self.cost("filesystems", started);
        if let Some(disks) = &disk_sample {
            self.disk_inventory = disks.clone();
        }
        let started = Instant::now();
        let docker = docker_containers().await;
        self.docker_scope = docker.as_ref().map(|(scope, _, _)| scope.clone());
        self.child_devices = docker
            .as_ref()
            .map(|(scope, containers, _)| {
                containers
                    .iter()
                    .map(|container| docker_child(container, scope))
                    .collect()
            })
            .unwrap_or_default();
        self.child_inventory_complete = docker.as_ref().is_some_and(|(_, _, complete)| *complete);
        let containers = docker.map(|(_, containers, _)| containers);
        self.cost("docker_containers", started);
        let started = Instant::now();
        let gateway = default_gateway().await;
        let selected_networks = crate::network::select(
            &self.networks,
            gateway.as_ref().map(|(name, _)| name.as_str()),
        );
        self.cost("gateway", started);
        let started = Instant::now();
        let mut receive = 0.0;
        let mut transmit = 0.0;
        let mut has_rate = true;
        let mut selected = 0;
        let mut network_interfaces = vec![];
        let mut current_network = HashMap::new();
        for name in &selected_networks.names {
            let Some(network) = self.networks.get(name) else {
                continue;
            };
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
                .unwrap_or_else(|| !crate::network::virtual_interface(name));
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
        let breakdown = extras::memory_breakdown(guest_cpu).await;
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
        let started = Instant::now();
        let system_uptime = crate::uptime::sample(guest_cpu).await;
        self.cost("uptime", started);
        let mut collectors = vec![
            status("uptime", system_uptime.is_some(), Some("System uptime is unavailable")),
            CollectorStatus {
                name: "cpu".into(),
                available: cpu_percent.is_some(),
                detail: Some(if cpu_percent.is_none() {
                    "CPU utilization is warming up or the selected machine counters are unavailable"
                } else if guest_cpu {
                    "Container CPU usage at the init process's cgroup boundary"
                } else {
                    "Host CPU usage from system counters"
                }.into()),
            },
            status("memory", memory.used.is_some(), Some("Memory counters are unavailable or inconsistent")),
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
            CollectorStatus {
                name: "docker_containers".into(),
                available: containers.is_some() && self.child_inventory_complete,
                detail: if containers.is_none() {
                    Some("Docker Engine is unavailable or cannot be reached".into())
                } else if !self.child_inventory_complete {
                    Some("Docker inventory exceeded the per-report limit or contained invalid entries; absence is not authoritative".into())
                } else {
                    None
                },
            },
        ];
        if let Some(status) = crate::proxmox::collector_status() {
            collectors.push(status);
        }
        if self.monitors.configured() > 0 {
            collectors.push(status(
                "monitors",
                monitors.len() == self.monitors.configured(),
                Some("Some configured monitors could not be checked"),
            ));
        }
        let started = Instant::now();
        let (interfaces, network_limited) =
            crate::network::inventory(&self.networks, &selected_networks, gateway.as_ref());
        collectors.push(status(
            "network_inventory",
            !network_limited,
            Some("Interface inventory limited to 128 interfaces and 64 addresses per interface"),
        ));
        let mut inventory = Inventory {
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
            memory_total_bytes: self.memory_total_bytes,
            interfaces,
            disks: self.disk_inventory.clone(),
            gpus: gpus.clone(),
            collectors: collectors.clone(),
        };
        let mut metrics = Metrics {
            system_uptime,
            cpu_percent,
            memory_used_bytes: memory.used,
            memory_total_bytes: memory.total,
            memory_cached_bytes: breakdown.cached_bytes,
            memory_zfs_arc_bytes: breakdown.zfs_arc_bytes,
            swap_used_bytes: memory.swap_used,
            swap_total_bytes: memory.swap_total,
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
        crate::telemetry::isolate(&mut inventory, &mut metrics, &mut collectors);
        inventory.collectors = collectors.clone();
        (inventory, metrics, collectors)
    }
    pub fn child_snapshots(&self) -> Vec<ChildSnapshot> {
        let mut snapshots = vec![];
        if let Some(source_scope) = self.docker_scope.clone() {
            snapshots.push(ChildSnapshot {
                source: hope_protocol::ChildSourceKind::Docker,
                source_scope,
                source_token: None,
                legacy_source_scope: None,
                legacy_identities: vec![],
                observed_at: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64(),
                devices: self.child_devices.clone(),
                inventory_complete: self.child_inventory_complete,
            });
        }
        snapshots.extend(crate::proxmox::read_snapshot());
        snapshots
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
    fn docker_restart_inspection_rotates_through_inventory_with_a_bounded_batch() {
        assert_eq!(docker_inspect_indices(0, 0), Vec::<usize>::new());
        assert_eq!(docker_inspect_indices(4, 0), vec![0, 1, 2, 3]);
        assert_eq!(
            docker_inspect_indices(40, 32),
            (32..40).chain(0..24).collect::<Vec<_>>()
        );
        assert_eq!(docker_inspect_indices(40, 40), (0..32).collect::<Vec<_>>());
        assert_eq!(
            docker_inspect_indices(128, 0).len(),
            MAX_DOCKER_INSPECTS_PER_REPORT
        );
        let visited: std::collections::HashSet<_> = (0..16)
            .flat_map(|report| docker_inspect_indices(128, report * DOCKER_INSPECT_ROTATION_STEP))
            .collect();
        assert_eq!(
            visited.len(),
            128,
            "rotation covers slow inspection batches"
        );
    }

    #[test]
    fn docker_restart_count_rejects_invalid_negative_or_overflow_values() {
        assert_eq!(docker_restart_count_value(Some(12)), Some(12));
        assert_eq!(docker_restart_count_value(Some(-1)), None);
        assert_eq!(docker_restart_count_value(Some(i64::MAX)), None);
        assert_eq!(docker_restart_count_value(None), None);
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
    fn docker_engine_inventory_is_structured_and_keeps_network_scope() {
        let item: bollard::models::ContainerSummary = serde_json::from_value(serde_json::json!({
            "Id":"abc123", "Names":["/web"], "Image":"nginx:latest", "State":"running",
            "Status":"Up 2 hours", "Created":1234, "Labels":{"com.docker.compose.project":"stack","com.docker.compose.service":"web","secret":"do-not-copy"},
            "Health":{"Status":"healthy"}, "Ports":[{"IP":"0.0.0.0","PrivatePort":80,"PublicPort":8080,"Type":"tcp"}],
            "NetworkSettings":{"Networks":{"front":{"NetworkID":"net-1","IPAddress":"10.0.0.2","MacAddress":"02:00:00:00:00:01"}}}
        })).expect("Docker API summary");
        let container = docker_summary(item).expect("container identity");
        assert_eq!(container.id, "abc123");
        assert_eq!(container.name, "web");
        assert_eq!(container.created_at, Some(1234.0));
        assert_eq!(container.health.as_deref(), Some("healthy"));
        assert_eq!(container.compose_project.as_deref(), Some("stack"));
        assert_eq!(container.compose_service.as_deref(), Some("web"));
        assert_eq!(container.networks[0].ipv4.as_deref(), Some("10.0.0.2"));
        assert_eq!(container.published_ports[0].host_port, Some(8080));
        assert!(
            !serde_json::to_string(&container)
                .unwrap()
                .contains("do-not-copy")
        );
        assert!(docker_inventory_complete(1, 1, 128));
        assert!(!docker_inventory_complete(129, 128, 128));
        assert!(!docker_inventory_complete(2, 1, 128));
    }

    #[test]
    fn docker_stats_use_engine_counters_and_ids_instead_of_display_names() {
        let summary = |id: &str| {
            docker_summary(serde_json::from_value(serde_json::json!({
            "Id":id, "Names":["/duplicate"], "Image":"image", "State":"running", "Status":"Up"
        })).unwrap()).unwrap()
        };
        let mut containers = [summary("id-a"), summary("id-b")];
        let stats: bollard::models::ContainerStatsResponse = serde_json::from_value(serde_json::json!({
            "id":"id-b", "cpu_stats":{"cpu_usage":{"total_usage":300},"system_cpu_usage":4000,"online_cpus":2},
            "precpu_stats":{"cpu_usage":{"total_usage":100},"system_cpu_usage":3000,"online_cpus":2},
            "memory_stats":{"usage":1000,"limit":2000,"stats":{"inactive_file":200}},
            "networks":{"eth0":{"rx_bytes":100,"tx_bytes":50}},"pids_stats":{"current":7},
            "blkio_stats":{"io_service_bytes_recursive":[{"op":"read","value":1000},{"op":"write","value":2000}]}
        })).unwrap();
        apply_docker_api_stats(&mut containers[1], &stats);
        assert_eq!(containers[0].cpu_percent, None);
        assert_eq!(containers[1].cpu_percent, Some(40.0));
        assert_eq!(containers[1].memory_used_bytes, Some(800.0));
        assert_eq!(containers[1].memory_limit_bytes, Some(2000.0));
        assert_eq!(containers[1].network_rx_bytes, Some(100.0));
        assert_eq!(containers[1].block_write_bytes, Some(2000.0));
        assert_eq!(containers[1].pids, Some(7));
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
