use hope_protocol::{CollectorStatus, Disk, Gpu, Interface, Inventory, Metrics, NetworkCounter};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use sysinfo::{Disks, Networks, System};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

pub struct Collector {
    system: System,
    networks: Networks,
    disks: DiskProbe,
    disk_inventory: Vec<Disk>,
    last: Instant,
    previous_network: HashMap<String, (u64, u64, String)>,
    previous_disk: Option<(u64, u64)>,
    gpus: Vec<Gpu>,
    gpu_at: Instant,
    first: bool,
}

struct DiskProbe {
    receiver: Option<tokio::sync::oneshot::Receiver<Vec<Disk>>>,
    collect: std::sync::Arc<dyn Fn() -> Vec<Disk> + Send + Sync>,
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
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self {
            system,
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
    pub async fn sample(&mut self) -> (Inventory, Metrics, Vec<CollectorStatus>) {
        let elapsed = self.last.elapsed().as_secs_f64();
        self.last = Instant::now();
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        let disk_sample = self.disks.sample(Duration::from_millis(500)).await;
        if let Some(disks) = &disk_sample {
            self.disk_inventory = disks.clone();
        }
        let gateway = default_gateway().await;
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
        let disks = disk_sample.clone().unwrap_or_default();
        let io = disk_counters().await;
        let disk_rate = self
            .previous_disk
            .zip(io)
            .map(|(old, new)| (rate(old.0, new.0, elapsed), rate(old.1, new.1, elapsed)));
        self.previous_disk = io;
        if self.gpu_at.elapsed() > Duration::from_secs(3600) {
            self.gpus = gpu_inventory().await;
            self.gpu_at = Instant::now();
        }
        let mut gpus = self.gpus.clone();
        if cfg!(target_os = "linux") {
            gpu_metrics(&mut gpus).await;
        }
        let gpu_metrics_available = gpus.iter().any(|g| g.utilization_percent.is_some());
        let collectors = vec![
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
        ];
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
            cpu_cores: self.system.cpus().len() as u32,
            memory_total_bytes: self.system.total_memory() as f64,
            interfaces,
            disks: self.disk_inventory.clone(),
            gpus: gpus.clone(),
            collectors: collectors.clone(),
        };
        let metrics = Metrics {
            cpu_percent: (!self.first).then(|| self.system.global_cpu_usage() as f64),
            memory_used_bytes: Some(self.system.used_memory() as f64),
            memory_total_bytes: Some(self.system.total_memory() as f64),
            network_rx_bytes_per_second: has_rate.then_some(receive),
            network_tx_bytes_per_second: has_rate.then_some(transmit),
            network_interfaces,
            disk_read_bytes_per_second: disk_rate.and_then(|r| r.0),
            disk_write_bytes_per_second: disk_rate.and_then(|r| r.1),
            disks,
            gpus,
        };
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
async fn command(binary: &str, args: &[&str], seconds: u64) -> Option<String> {
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
