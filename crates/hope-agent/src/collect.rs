use hope_protocol::{CollectorStatus, Disk, Gpu, Interface, Inventory, Metrics};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use sysinfo::{Disks, Networks, System};
use tokio::{process::Command, time::timeout};

pub struct Collector {
    system: System,
    networks: Networks,
    disks: Disks,
    last: Instant,
    previous_network: HashMap<String, (u64, u64)>,
    previous_disk: Option<(u64, u64)>,
    gpus: Vec<Gpu>,
    gpu_at: Instant,
    first: bool,
}

pub fn rate(previous: u64, current: u64, seconds: f64) -> Option<f64> {
    if seconds <= 0.0 || current < previous {
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
            disks: Disks::new_with_refreshed_list(),
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
        self.disks.refresh(true);
        let mut receive = 0.0;
        let mut transmit = 0.0;
        let mut has_rate = false;
        let mut current_network = HashMap::new();
        for (name, network) in &self.networks {
            if name == "lo" || name == "lo0" {
                continue;
            }
            let counters = (network.total_received(), network.total_transmitted());
            if let Some(previous) = self.previous_network.get(name)
                && let (Some(rx), Some(tx)) = (
                    rate(previous.0, counters.0, elapsed),
                    rate(previous.1, counters.1, elapsed),
                ) {
                    receive += rx;
                    transmit += tx;
                    has_rate = true;
                }
            current_network.insert(name.clone(), counters);
        }
        self.previous_network = current_network;
        let disks: Vec<Disk> = self
            .disks
            .iter()
            .take(128)
            .map(|d| Disk {
                name: d.name().to_string_lossy().into(),
                mount: d.mount_point().to_string_lossy().into(),
                filesystem: d.file_system().to_string_lossy().into(),
                total_bytes: d.total_space() as f64,
                available_bytes: d.available_space() as f64,
            })
            .collect();
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
            status("cpu", true, None),
            status("memory", true, None),
            status("network", true, None),
            status("storage", true, None),
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
        let interfaces = interfaces(&self.networks).await;
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
            disks: disks.clone(),
            gpus: gpus.clone(),
            collectors: collectors.clone(),
        };
        let metrics = Metrics {
            cpu_percent: (!self.first).then(|| self.system.global_cpu_usage() as f64),
            memory_used_bytes: Some(self.system.used_memory() as f64),
            memory_total_bytes: Some(self.system.total_memory() as f64),
            network_rx_bytes_per_second: has_rate.then_some(receive),
            network_tx_bytes_per_second: has_rate.then_some(transmit),
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
}

async fn interfaces(networks: &Networks) -> Vec<Interface> {
    let addresses = if_addrs::get_if_addrs().unwrap_or_default();
    let gateway = default_gateway().await;
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
        if p[2].starts_with("loop") || p[2].starts_with("ram") || p[2].starts_with("dm-") {
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
    cmd.args(args).kill_on_drop(true);
    let output = timeout(Duration::from_secs(seconds), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return None;
    }
    String::from_utf8(output.stdout).ok()
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
                && let Some(devices) = value["SPDisplaysDataType"].as_array() {
                    for device in devices.iter().take(32) {
                        gpus.push(Gpu {
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
            gpus.push(Gpu {
                name: format!("{vendor_name} {} ({name})", device.trim()),
                vendor: vendor_name.into(),
                utilization_percent: None,
                memory_used_bytes: None,
                memory_total_bytes: None,
                temperature_celsius: None,
            });
        }
    }
    gpus
}
async fn gpu_metrics(gpus: &mut Vec<Gpu>) {
    if let Some(output) = command(
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
        2,
    )
    .await
    {
        gpus.retain(|g| g.vendor != "NVIDIA");
        for line in output.lines().take(32) {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            if fields.len() != 5 {
                continue;
            }
            gpus.push(Gpu {
                name: fields[0].into(),
                vendor: "NVIDIA".into(),
                utilization_percent: fields[1].parse().ok(),
                memory_used_bytes: fields[2].parse::<f64>().ok().map(|v| v * 1048576.0),
                memory_total_bytes: fields[3].parse::<f64>().ok().map(|v| v * 1048576.0),
                temperature_celsius: fields[4].parse().ok(),
            });
        }
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
            && let Ok(Some(entry)) = entries.next_entry().await {
                gpu.temperature_celsius =
                    read_number(&entry.path().join("temp1_input").to_string_lossy())
                        .await
                        .map(|v| v / 1000.0);
            }
    }
}
async fn read_number(path: &str) -> Option<f64> {
    tokio::fs::read_to_string(path)
        .await
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counter_reset_is_unavailable() {
        assert_eq!(rate(100, 50, 30.0), None);
        assert_eq!(rate(100, 400, 30.0), Some(10.0));
        assert_eq!(rate(0, 1, 0.0), None);
    }
}
