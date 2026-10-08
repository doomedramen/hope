//! Shared, versioned wire contract. Timestamps are Unix seconds; byte values are bytes.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Gpu {
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

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
pub struct Metrics {
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<f64>,
    pub memory_total_bytes: Option<f64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    pub disk_read_bytes_per_second: Option<f64>,
    pub disk_write_bytes_per_second: Option<f64>,
    pub disks: Vec<Disk>,
    pub gpus: Vec<Gpu>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScanJob {
    pub id: String,
    pub network_id: String,
    pub cidr: String,
    pub status: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Host {
    pub id: String,
    pub agent_id: Option<String>,
    pub name: String,
    pub addresses: Vec<String>,
    pub macs: Vec<String>,
    pub network_ids: Vec<String>,
    pub last_seen: f64,
    pub last_report: Option<f64>,
    pub discovery_method: String,
    pub inventory: Option<Inventory>,
    pub metrics: Option<Metrics>,
    pub collectors: Vec<CollectorStatus>,
    pub services: Vec<Service>,
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
    pub positions: Vec<Position>,
    pub links: Vec<MapLink>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct InstallJob {
    pub id: String,
    pub host: String,
    pub status: String,
    pub messages: Vec<String>,
    pub created_at: f64,
}

/// Generate declarations from the same types used by the Rust HTTP handlers.
pub fn typescript() -> String {
    let declarations = [
        Interface::decl(),
        Disk::decl(),
        Gpu::decl(),
        CollectorStatus::decl(),
        Inventory::decl(),
        Metrics::decl(),
        AgentReport::decl(),
        EnrollmentRequest::decl(),
        EnrollmentResponse::decl(),
        Network::decl(),
        ScanJob::decl(),
        Service::decl(),
        Host::decl(),
        MetricPoint::decl(),
        Position::decl(),
        MapLink::decl(),
        MapState::decl(),
        Capabilities::decl(),
        Bootstrap::decl(),
        InstallJob::decl(),
    ];
    format!(
        "// Generated by hope-server export-types. Do not edit.\n{}\n",
        declarations
            .into_iter()
            .map(|d| format!("export {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
