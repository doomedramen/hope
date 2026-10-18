//! Shared, versioned wire contract. Timestamps are Unix seconds; byte values are bytes.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod validation;

pub const SCHEMA_VERSION: u32 = 1;
/// Maximum JSON request body accepted by the server, including report metadata.
pub const MAX_AGENT_REPORT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ScanStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum EnrichmentStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Reporting,
    Stale,
    Revoked,
    Unenrolled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
    Uncertain,
}

/// Allowlisted installer phase (or server-side connection step) reached by an SSH job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InstallStage {
    Connecting,
    Authenticating,
    Preflight,
    Recover,
    Uninstall,
    Stage,
    Verify,
    Stop,
    PlaceBinary,
    Configure,
    WriteUnit,
    Activate,
    WaitReport,
    Rollback,
    Complete,
    ConfirmingReport,
}

/// Fresh installs enroll with a token; reinstalls and uninstalls are bound to the existing identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    Install,
    Reinstall,
    Uninstall,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LoginRequest {
    pub password: String,
    pub setup_token: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}
/// ntfy notification settings. `ntfy_token` is write-only: responses never contain it and report
/// `ntfy_token_set` instead; on update, null keeps the stored token and "" removes it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
pub struct NotificationSettings {
    pub ntfy_enabled: bool,
    pub ntfy_server: String,
    pub ntfy_topic: String,
    #[serde(default)]
    pub ntfy_token: Option<String>,
    #[serde(default)]
    pub ntfy_token_set: bool,
    /// A device the server has never seen joins a monitored range.
    pub notify_new_devices: bool,
    /// An agent stops reporting, or infrastructure (router, switch, access point, server, NAS,
    /// hypervisor) stops answering scans.
    pub notify_offline: bool,
    /// Full disks or pools, degraded pools, hot sensors, failed services, failing monitors.
    pub notify_resources: bool,
    /// Send a follow-up when a problem clears.
    pub notify_recovery: bool,
    #[serde(default)]
    pub last_sent_at: Option<f64>,
    #[serde(default)]
    pub last_error: Option<String>,
}
impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            ntfy_enabled: false,
            ntfy_server: "https://ntfy.sh".into(),
            ntfy_topic: String::new(),
            ntfy_token: None,
            ntfy_token_set: false,
            notify_new_devices: true,
            notify_offline: true,
            notify_resources: true,
            notify_recovery: true,
            last_sent_at: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AuthStatus {
    pub setup_required: bool,
    pub authenticated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OkResponse {
    pub ok: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentUninstallInstructions {
    /// Run on the device to stop and remove the service, updater and binary. Identity is retained.
    pub command: String,
    pub custom_ca_required: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReportResponse {
    pub accepted: bool,
    pub duplicate: bool,
    /// Seconds the server asks the agent to wait between samples; older servers omit it.
    #[serde(default)]
    pub report_interval_seconds: Option<u32>,
    /// Release version an administrator asked this agent to update to; absent when none is pending.
    /// The agent only passes the request to its local updater, which decides for itself.
    #[serde(default)]
    pub update_to: Option<String>,
}
/// How many agents an "update all" request reached.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct UpdateRequested {
    pub requested: u32,
}
/// How long an update request stays pending before it lapses.
pub const UPDATE_REQUEST_SECONDS: f64 = 86400.0;
/// Whether `candidate` is a later release than `installed`. Compares the dotted numbers only
/// (`0.1.57` against `0.1.9`), ignoring anything after `-` or `+`.
pub fn version_newer(candidate: &str, installed: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    let (candidate, installed) = (parts(candidate), parts(installed));
    for index in 0..candidate.len().max(installed.len()) {
        let (a, b) = (
            candidate.get(index).copied().unwrap_or(0),
            installed.get(index).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    false
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ErrorResponse {
    pub code: String,
    pub error: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkInput {
    pub name: String,
    pub cidr: String,
    pub enabled: Option<bool>,
    #[serde(default)]
    pub interval_minutes: Option<u32>,
}
/// Partial update: omitted fields keep their value.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub interval_minutes: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MergeRequest {
    pub source_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MetricWindow {
    pub hours: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
pub struct EnrollmentTokenRequest {
    #[serde(default)]
    pub reset_agent_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EnrollmentTokenResponse {
    pub token: String,
    pub expires_at: f64,
    pub command: String,
    pub custom_ca_required: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SshTarget {
    pub host_id: String,
    pub address: String,
    pub port: Option<u16>,
}
#[derive(Clone, Serialize, Deserialize, TS)]
pub struct SshInstallRequest {
    #[serde(flatten)]
    pub target: SshTarget,
    pub username: String,
    pub fingerprint: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
}
/// Credentials never appear in debug output.
impl std::fmt::Debug for SshInstallRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |value: &Option<String>| value.as_ref().map(|_| "[redacted]");
        f.debug_struct("SshInstallRequest")
            .field("target", &self.target)
            .field("username", &self.username)
            .field("fingerprint", &self.fingerprint)
            .field("password", &redact(&self.password))
            .field("private_key", &redact(&self.private_key))
            .field("passphrase", &redact(&self.passphrase))
            .finish()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FingerprintResponse {
    pub fingerprint: String,
    pub instruction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Interface {
    pub name: String,
    pub mac: Option<String>,
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Disk {
    pub name: String,
    pub mount: String,
    pub filesystem: String,
    pub total_bytes: f64,
    pub available_bytes: f64,
}

impl Disk {
    /// Whether this mount is a guest's root filesystem exposed on its hypervisor,
    /// such as a Proxmox LXC subvolume (`rpool/data/subvol-123-disk-0`). Its
    /// usage belongs to the guest, not the host's own storage.
    pub fn is_guest_storage(&self) -> bool {
        // Inside the guest, this is its own root filesystem even when the
        // backing dataset retains the hypervisor's subvol/basevol name.
        if self.mount == "/" {
            return false;
        }
        fn guest_volume(path: &str) -> bool {
            let leaf = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            let Some(rest) = leaf
                .strip_prefix("subvol-")
                .or_else(|| leaf.strip_prefix("basevol-"))
            else {
                return false;
            };
            let mut parts = rest.split('-');
            let numeric = |part: Option<&str>| {
                part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            };
            numeric(parts.next())
                && parts.next() == Some("disk")
                && numeric(parts.next())
                && parts.next().is_none()
        }
        let lxc_rootfs = self.mount.starts_with("/var/lib/lxc/")
            && self.mount.trim_end_matches('/').ends_with("/rootfs");
        guest_volume(&self.mount) || guest_volume(&self.name) || lxc_rootfs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Gpu {
    /// Driver UUID or device path; absent when the platform cannot identify the adapter.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub vendor: String,
    pub utilization_percent: Option<f64>,
    pub memory_used_bytes: Option<f64>,
    pub memory_total_bytes: Option<f64>,
    pub temperature_celsius: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CollectorStatus {
    pub name: String,
    pub available: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Inventory {
    pub hostname: String,
    pub os: String,
    pub architecture: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub memory_total_bytes: f64,
    pub interfaces: Vec<Interface>,
    pub disks: Vec<Disk>,
    pub gpus: Vec<Gpu>,
    pub collectors: Vec<CollectorStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkCounter {
    pub name: String,
    pub received_bytes: f64,
    pub transmitted_bytes: f64,
    pub rx_bytes_per_second: Option<f64>,
    pub tx_bytes_per_second: Option<f64>,
    pub included_in_total: bool,
}

/// System load averages as the kernel reports them (runnable tasks, not percentages).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// One temperature sensor reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Temperature {
    /// Stable identifier for history (driver and channel), unique within a host.
    pub sensor: String,
    /// Human label, for example "Package id 0" or "nvme Composite".
    pub label: String,
    pub celsius: f64,
}

/// Throughput of one block device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct DiskIo {
    pub device: String,
    pub read_bytes_per_second: Option<f64>,
    pub write_bytes_per_second: Option<f64>,
    /// Share of the interval the device spent with I/O in flight.
    pub busy_percent: Option<f64>,
    /// True when the root filesystem lives on this device.
    pub root: bool,
}

/// A storage pool (ZFS today) with capacity and throughput.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct StoragePool {
    pub name: String,
    /// Pool technology, for example "zfs".
    pub kind: String,
    pub size_bytes: f64,
    pub allocated_bytes: f64,
    /// Pool-reported health, for example ONLINE or DEGRADED.
    pub health: String,
    pub read_bytes_per_second: Option<f64>,
    pub write_bytes_per_second: Option<f64>,
    /// True when the root filesystem is a dataset of this pool.
    pub root: bool,
}

/// A service unit the agent reports: failed units, and units the operator asked to watch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ServiceUnit {
    pub name: String,
    /// Manager state: active, inactive, failed, activating, deactivating.
    pub state: String,
    pub sub_state: String,
    pub description: String,
    /// True for units named in the agent's watch list (reported whatever their state).
    pub watched: bool,
}

/// Counts over every service unit the service manager knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ServiceSummary {
    pub active: u32,
    pub failed: u32,
    pub total: u32,
}

/// Result of one configured reachability check from the agent's point of view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorResult {
    /// The configured target, for example "tcp://10.0.0.1:22" or "https://example.com/".
    pub target: String,
    /// "tcp", "http" or "https".
    pub kind: String,
    pub up: bool,
    pub latency_ms: Option<f64>,
    pub status_code: Option<u16>,
    /// Short failure reason; never remote response bodies.
    pub detail: Option<String>,
}

/// A container discovered through the local Docker Engine API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// Docker state such as running, exited, or created.
    pub state: String,
    pub status: String,
    pub ports: String,
    /// Docker creation time as Unix seconds.
    #[serde(default)]
    pub created_at: Option<f64>,
    /// Health-check state, when a health check is configured.
    #[serde(default)]
    pub health: Option<String>,
    /// Docker Compose project and service labels, when present.
    #[serde(default)]
    pub compose_project: Option<String>,
    #[serde(default)]
    pub compose_service: Option<String>,
    /// Docker network attachments. Addresses stay scoped to their network.
    #[serde(default)]
    pub networks: Vec<ContainerNetwork>,
    /// Structured host-to-container port mappings.
    #[serde(default)]
    pub published_ports: Vec<ContainerPort>,
    /// Docker restart count. Unavailable when the daemon summary omits it.
    #[serde(default)]
    pub restart_count: Option<u32>,
    #[serde(default)]
    pub cpu_percent: Option<f64>,
    #[serde(default)]
    pub memory_used_bytes: Option<f64>,
    #[serde(default)]
    pub memory_limit_bytes: Option<f64>,
    #[serde(default)]
    pub memory_percent: Option<f64>,
    #[serde(default)]
    pub network_rx_bytes: Option<f64>,
    #[serde(default)]
    pub network_tx_bytes: Option<f64>,
    #[serde(default)]
    pub block_read_bytes: Option<f64>,
    #[serde(default)]
    pub block_write_bytes: Option<f64>,
    #[serde(default)]
    pub pids: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ContainerNetwork {
    pub name: String,
    #[serde(default)]
    pub network_id: Option<String>,
    #[serde(default)]
    pub ipv4: Option<String>,
    #[serde(default)]
    pub ipv6: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ContainerPort {
    #[serde(default)]
    pub host_ip: Option<String>,
    pub container_port: u16,
    #[serde(default)]
    pub host_port: Option<u16>,
    pub protocol: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UptimeScope {
    System,
    /// Kernel counters visible inside a container; these may describe its host.
    ContainerKernel,
}

/// Latest observed boot metadata, never an average across different boots.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SystemUptime {
    pub seconds: f64,
    pub boot_time: Option<f64>,
    /// Linux boot UUID, when readable. A container may share its host's UUID.
    pub boot_id: Option<String>,
    pub scope: UptimeScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
pub struct Metrics {
    #[serde(default)]
    pub system_uptime: Option<SystemUptime>,
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<f64>,
    pub memory_total_bytes: Option<f64>,
    /// Page cache plus buffers and reclaimable slab; part of the kernel's reclaimable memory.
    #[serde(default)]
    pub memory_cached_bytes: Option<f64>,
    /// ZFS ARC size; counted as used by the kernel, released under pressure.
    #[serde(default)]
    pub memory_zfs_arc_bytes: Option<f64>,
    #[serde(default)]
    pub swap_used_bytes: Option<f64>,
    #[serde(default)]
    pub swap_total_bytes: Option<f64>,
    #[serde(default)]
    pub load_average: Option<LoadAverage>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    #[serde(default)]
    pub network_interfaces: Vec<NetworkCounter>,
    pub disk_read_bytes_per_second: Option<f64>,
    pub disk_write_bytes_per_second: Option<f64>,
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub disk_io: Vec<DiskIo>,
    #[serde(default)]
    pub pools: Vec<StoragePool>,
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub temperatures: Vec<Temperature>,
    #[serde(default)]
    pub services: Vec<ServiceUnit>,
    #[serde(default)]
    pub service_summary: Option<ServiceSummary>,
    #[serde(default)]
    pub monitors: Vec<MonitorResult>,
    #[serde(default)]
    pub containers: Vec<Container>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentReport {
    pub schema_version: u32,
    pub report_id: String,
    pub agent_id: String,
    pub collected_at: f64,
    pub inventory: Option<Inventory>,
    pub metrics: Metrics,
    pub collectors: Vec<CollectorStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EnrollmentRequest {
    pub schema_version: u32,
    pub token: String,
    pub agent_id: String,
    /// Client-generated secret persisted before enrollment, allowing safe retries after a lost response.
    pub credential: String,
    pub inventory: Inventory,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EnrollmentResponse {
    pub credential: String,
    pub report_interval_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub cidr: String,
    pub enabled: bool,
    /// Minutes between scheduled background scans of this range.
    #[serde(default = "default_scan_interval")]
    pub interval_minutes: u32,
}
pub const DEFAULT_SCAN_INTERVAL_MINUTES: u32 = 5;
/// Scheduled scan intervals the server accepts, in minutes.
pub const SCAN_INTERVALS_MINUTES: [u32; 6] = [5, 15, 30, 60, 360, 1440];
fn default_scan_interval() -> u32 {
    DEFAULT_SCAN_INTERVAL_MINUTES
}
/// Seconds between agent samples unless an administrator chooses otherwise.
pub const DEFAULT_REPORT_INTERVAL_SECONDS: u32 = 10;
/// Agent report intervals the server accepts, in seconds.
pub const REPORT_INTERVALS_SECONDS: [u32; 5] = [5, 10, 15, 30, 60];
/// Stored metric history keeps at most one raw sample per this many seconds per agent, so a
/// faster report interval makes live figures fresher without growing history.
pub const HISTORY_SAMPLE_SECONDS: f64 = 30.0;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReportingSettings {
    /// Seconds between agent samples; one of `REPORT_INTERVALS_SECONDS`.
    pub interval_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScanJob {
    pub id: String,
    pub network_id: String,
    pub cidr: String,
    pub status: ScanStatus,
    pub scanned: u32,
    pub total: u32,
    pub discovered: u32,
    pub started_at: f64,
    pub finished_at: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Service {
    pub port: u16,
    pub name: String,
    pub observed_at: f64,
}

/// What the server could learn about a device without an agent. Every field is optional evidence.
#[derive(Debug, Clone, Serialize, Deserialize, TS, Default, PartialEq)]
pub struct DeviceEvidence {
    /// Name from reverse DNS.
    #[serde(default)]
    pub dns_name: Option<String>,
    /// Host name the device announces over mDNS (without `.local`).
    #[serde(default)]
    pub mdns_name: Option<String>,
    /// mDNS service types, for example `_googlecast._tcp`.
    #[serde(default)]
    pub mdns_services: Vec<String>,
    /// Model from mDNS TXT records (`model=`, `md=`, `ty=`).
    #[serde(default)]
    pub mdns_model: Option<String>,
    /// Service instance names the device announces, for example "Living Room TV".
    #[serde(default)]
    pub mdns_instances: Vec<String>,
    #[serde(default)]
    pub ssdp_server: Option<String>,
    #[serde(default)]
    pub upnp_device_type: Option<String>,
    #[serde(default)]
    pub upnp_friendly_name: Option<String>,
    #[serde(default)]
    pub upnp_manufacturer: Option<String>,
    #[serde(default)]
    pub upnp_model: Option<String>,
    /// `<title>` of the device's web interface.
    #[serde(default)]
    pub http_title: Option<String>,
    /// `Server` header of the device's web interface.
    #[serde(default)]
    pub http_server: Option<String>,
}

/// The server's best guess at what a device is, with the evidence behind it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, Default, PartialEq)]
pub struct DeviceGuess {
    /// Stable identifier: `router`, `switch`, `access_point`, `hypervisor`, `nas`, `server`, `vm`,
    /// `computer`, `phone`, `tablet`, `tv`, `streamer`, `speaker`, `printer`, `camera`, `vacuum`,
    /// `appliance`, `game_console`, `smart_home` or `unknown`.
    pub kind: String,
    /// Specific name when known (for example "Proxmox VE" or "Dishwasher"), else the kind's name.
    pub label: String,
    /// 0 to 100.
    pub confidence: u8,
    /// Human-readable evidence, strongest first.
    pub reasons: Vec<String>,
    /// True when the administrator set the type; guesses then never replace it.
    #[serde(default)]
    pub manual: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeviceTypeUpdate {
    /// A `DeviceGuess::kind`, or null to return to automatic detection.
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct HostIdentityUpdate {
    /// Administrator-chosen name, or null to use discovery data.
    pub name: Option<String>,
    /// `selfhst:<slug>` or `unifi:<fingerprint id>`, or null for automatic matching.
    pub icon: Option<String>,
}

/// Per-device uptime monitoring. Disabled until explicitly enabled by an administrator.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, Default)]
pub struct UptimeSettings {
    pub enabled: bool,
    pub device: bool,
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Host {
    #[serde(default)]
    pub uptime: UptimeSettings,
    pub id: String,
    pub agent_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub name_override: Option<String>,
    #[serde(default)]
    pub icon_override: Option<String>,
    pub addresses: Vec<String>,
    pub macs: Vec<String>,
    pub network_ids: Vec<String>,
    pub last_seen: f64,
    pub last_report: Option<f64>,
    #[serde(default)]
    pub last_collected_at: Option<f64>,
    #[serde(default)]
    pub agent_revoked: bool,
    #[serde(default)]
    pub enrichment_status: EnrichmentStatus,
    pub discovery_method: String,
    pub inventory: Option<Inventory>,
    pub metrics: Option<Metrics>,
    pub collectors: Vec<CollectorStatus>,
    pub services: Vec<Service>,
    /// Manufacturer registered for the MAC address prefix (IEEE OUI).
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub device: Option<DeviceGuess>,
    #[serde(default)]
    pub evidence: DeviceEvidence,
    /// When the common-port scan last completed.
    #[serde(default)]
    pub ports_scanned_at: Option<f64>,
    /// The device this one sits behind (its gateway or upstream router), when inferred.
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub parent_reason: Option<String>,
    /// Version of the agent, from its `User-Agent` on enrollment and reports.
    #[serde(default)]
    pub agent_version: Option<String>,
    /// When an administrator last asked this agent to update; cleared once it runs the release.
    #[serde(default)]
    pub update_requested_at: Option<f64>,
    /// The release version the pending request is for.
    #[serde(default)]
    pub update_target: Option<String>,
}

impl Host {
    /// A host with no observations yet.
    pub fn new(id: String, name: String, discovery_method: &str) -> Self {
        Self {
            id,
            agent_id: None,
            name,
            uptime: UptimeSettings::default(),
            name_override: None,
            icon_override: None,
            addresses: vec![],
            macs: vec![],
            network_ids: vec![],
            last_seen: 0.0,
            last_report: None,
            last_collected_at: None,
            agent_revoked: false,
            enrichment_status: EnrichmentStatus::Pending,
            discovery_method: discovery_method.into(),
            inventory: None,
            metrics: None,
            collectors: vec![],
            services: vec![],
            vendor: None,
            device: None,
            evidence: DeviceEvidence::default(),
            ports_scanned_at: None,
            parent_id: None,
            parent_reason: None,
            agent_version: None,
            update_requested_at: None,
            update_target: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MetricPoint {
    pub collected_at: f64,
    pub resolution_seconds: u32,
    pub metrics: Metrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Position {
    pub id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MapLink {
    pub id: String,
    pub source: String,
    pub target: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
pub struct MapState {
    #[serde(default)]
    #[ts(type = "number")]
    pub revision: u64,
    pub positions: Vec<Position>,
    pub links: Vec<MapLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MapEdit {
    SetPosition { position: Position },
    AddLink { link: MapLink },
    RemoveLink { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReleaseAvailability {
    pub packaged_platforms: Vec<String>,
    /// Version of the signed agent release on offer, when there is one.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Capabilities {
    pub platform: String,
    pub discovery_mode: String,
    pub arp: bool,
    pub icmp: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Bootstrap {
    pub networks: Vec<Network>,
    pub hosts: Vec<Host>,
    pub scans: Vec<ScanJob>,
    pub map: MapState,
    pub capabilities: Capabilities,
    pub server_url: String,
    /// Version of the signed agent release the server offers; agents older than it can be updated.
    #[serde(default)]
    pub agent_release: Option<String>,
    /// Seconds between agent samples, as agents are currently asked to report.
    pub report_interval_seconds: u32,
    /// Command that updates an installed agent on the host where it runs. It carries no token or
    /// credential: the agent is already enrolled, and the installer checks the release signature.
    pub agent_upgrade_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct InstallJob {
    pub id: String,
    /// Selected SSH address.
    pub host: String,
    pub host_id: String,
    pub port: u16,
    /// Host-key fingerprint the user confirmed; the connection is refused if it differs.
    pub fingerprint: String,
    pub username: String,
    pub mode: InstallMode,
    pub status: InstallStatus,
    pub stage: Option<InstallStage>,
    pub stages: Vec<InstallStage>,
    /// Stable code from the installer allowlist or the server (never remote text).
    pub error_code: Option<String>,
    /// Server-vetted description of the outcome.
    pub message: Option<String>,
    /// Server-vetted retry guidance for the error code.
    pub guidance: Option<String>,
    /// Server-vetted installer notice (for example the user-service fallback).
    pub notice: Option<String>,
    /// Agent identity bound to or created by this job.
    pub agent_id: Option<String>,
    pub created_at: f64,
    pub finished_at: Option<f64>,
}

/// Generate declarations from the same types used by the Rust HTTP handlers.
pub fn typescript() -> String {
    let declarations = [
        ScanStatus::decl(),
        EnrichmentStatus::decl(),
        AgentStatus::decl(),
        InstallStatus::decl(),
        InstallStage::decl(),
        InstallMode::decl(),
        LoginRequest::decl(),
        PasswordChange::decl(),
        NotificationSettings::decl(),
        ReportingSettings::decl(),
        UpdateRequested::decl(),
        DeviceEvidence::decl(),
        DeviceGuess::decl(),
        DeviceTypeUpdate::decl(),
        HostIdentityUpdate::decl(),
        AuthStatus::decl(),
        OkResponse::decl(),
        AgentUninstallInstructions::decl(),
        ReportResponse::decl(),
        ErrorResponse::decl(),
        NetworkInput::decl(),
        NetworkUpdate::decl(),
        MergeRequest::decl(),
        MetricWindow::decl(),
        EnrollmentTokenRequest::decl(),
        EnrollmentTokenResponse::decl(),
        SshTarget::decl(),
        SshInstallRequest::decl(),
        FingerprintResponse::decl(),
        Interface::decl(),
        Disk::decl(),
        Gpu::decl(),
        CollectorStatus::decl(),
        Inventory::decl(),
        NetworkCounter::decl(),
        LoadAverage::decl(),
        Temperature::decl(),
        DiskIo::decl(),
        StoragePool::decl(),
        ServiceUnit::decl(),
        ServiceSummary::decl(),
        MonitorResult::decl(),
        ContainerNetwork::decl(),
        ContainerPort::decl(),
        Container::decl(),
        UptimeScope::decl(),
        SystemUptime::decl(),
        Metrics::decl(),
        AgentReport::decl(),
        EnrollmentRequest::decl(),
        EnrollmentResponse::decl(),
        Network::decl(),
        ScanJob::decl(),
        Service::decl(),
        UptimeSettings::decl(),
        Host::decl(),
        MetricPoint::decl(),
        Position::decl(),
        MapLink::decl(),
        MapState::decl(),
        MapEdit::decl(),
        ReleaseAvailability::decl(),
        Capabilities::decl(),
        Bootstrap::decl(),
        InstallJob::decl(),
    ];
    format!(
        "// Generated by hope-server export-types. Do not edit.\n{}\n",
        declarations
            .into_iter()
            .map(|d| {
                let declaration = d.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
                format!("export {declaration}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uptime_is_optional_for_old_agents_and_rejects_invalid_boot_observations() {
        let old: Metrics = serde_json::from_str(
            r#"{"cpu_percent":null,"memory_used_bytes":null,"memory_total_bytes":null,"network_rx_bytes_per_second":null,"network_tx_bytes_per_second":null,"disk_read_bytes_per_second":null,"disk_write_bytes_per_second":null,"disks":[],"gpus":[]}"#,
        ).expect("old agent metrics");
        assert!(old.system_uptime.is_none());
        assert!(validation::metrics(&old).is_ok());
        let mut metrics = Metrics {
            system_uptime: Some(SystemUptime {
                seconds: 100.0,
                boot_time: None,
                boot_id: Some("28046bda-bbbf-4101-9455-8a7818f0969b".into()),
                scope: UptimeScope::System,
            }),
            ..Default::default()
        };
        assert!(validation::metrics(&metrics).is_ok());
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 1e13] {
            metrics.system_uptime.as_mut().unwrap().seconds = invalid;
            assert!(validation::metrics(&metrics).is_err());
        }
        metrics.system_uptime.as_mut().unwrap().seconds = 1.0;
        metrics.system_uptime.as_mut().unwrap().boot_id = Some("invalid".into());
        assert!(validation::metrics(&metrics).is_err());
        metrics.system_uptime.as_mut().unwrap().boot_id = None;
        metrics.system_uptime.as_mut().unwrap().boot_time = Some(-1.0);
        assert!(validation::metrics(&metrics).is_err());
    }
    #[test]
    fn ssh_request_debug_redacts_credentials() {
        let request = SshInstallRequest {
            target: SshTarget {
                host_id: "h".into(),
                address: "127.0.0.1".into(),
                port: Some(22),
            },
            username: "root".into(),
            fingerprint: "SHA256:x".into(),
            password: Some("pw-secret-1".into()),
            private_key: Some("key-secret-2".into()),
            passphrase: Some("phrase-secret-3".into()),
        };
        let shown = format!("{request:?}");
        for secret in ["pw-secret-1", "key-secret-2", "phrase-secret-3"] {
            assert!(!shown.contains(secret));
        }
        assert!(shown.contains("[redacted]") && shown.contains("root"));
    }
    #[test]
    fn versions_compare_by_number_not_text() {
        assert!(version_newer("0.1.10", "0.1.9"));
        assert!(version_newer("0.2.0", "0.1.99"));
        assert!(version_newer("1.0", "0.9.9"));
        assert!(!version_newer("0.1.9", "0.1.10"));
        assert!(!version_newer("0.1.57", "0.1.57"));
        assert!(!version_newer("0.1.57", "0.1.57-rc.1"));
        assert!(version_newer("0.1.58-rc.1", "0.1.57"));
        assert!(!version_newer("garbage", "0.1.0"));
    }
    #[test]
    fn guest_storage_is_recognised() {
        let disk = |name: &str, mount: &str| Disk {
            name: name.into(),
            mount: mount.into(),
            filesystem: "zfs".into(),
            total_bytes: 1.0,
            available_bytes: 0.0,
        };
        for (name, mount) in [
            (
                "rpool/data/subvol-123-disk-0",
                "/rpool/data/subvol-123-disk-0",
            ),
            ("tank/basevol-9000-disk-1", "/tank/basevol-9000-disk-1"),
            ("rpool/data/subvol-108-disk-0", "/mnt/elsewhere"),
            ("/dev/mapper/x", "/var/lib/lxc/101/rootfs"),
        ] {
            assert!(disk(name, mount).is_guest_storage(), "{mount}");
        }
        for (name, mount) in [
            ("rpool/ROOT/pve-1", "/"),
            ("rpool/data/subvol-122-disk-0", "/"),
            ("tank/basevol-9000-disk-1", "/"),
            ("rpool/data", "/rpool/data"),
            ("tank/subvol-backups", "/tank/subvol-backups"),
            ("tank/subvol-1-disk-0-old", "/tank/subvol-1-disk-0-old"),
        ] {
            assert!(!disk(name, mount).is_guest_storage(), "{mount}");
        }
    }
    #[test]
    fn legacy_host_defaults_new_metadata() {
        let host: Host = serde_json::from_str(r#"{"id":"host","agent_id":null,"name":"host","addresses":[],"macs":[],"network_ids":[],"last_seen":0,"last_report":null,"discovery_method":"tcp","inventory":null,"metrics":null,"collectors":[],"services":[]}"#).expect("legacy host");
        assert!(!host.agent_revoked);
        assert!(host.last_collected_at.is_none());
    }
    #[test]
    fn contract_statuses_reject_unknown_values() {
        assert!(serde_json::from_str::<ScanStatus>("\"running\"").is_ok());
        assert!(serde_json::from_str::<ScanStatus>("\"unknown\"").is_err());
        assert!(serde_json::from_str::<InstallStatus>("\"unknown\"").is_err());
    }
}
