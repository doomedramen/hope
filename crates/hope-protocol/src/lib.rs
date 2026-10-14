//! Shared, versioned wire contract. Timestamps are Unix seconds; byte values are bytes.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SCHEMA_VERSION: u32 = 1;

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

/// A fresh install enrolls with the job's token; a reinstall is bound to the host's existing agent identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    Install,
    Reinstall,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LoginRequest {
    pub password: String,
    pub setup_token: Option<String>,
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
pub struct ReportResponse {
    pub accepted: bool,
    pub duplicate: bool,
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
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkUpdate {
    pub enabled: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
pub struct Metrics {
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<f64>,
    pub memory_total_bytes: Option<f64>,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
    #[serde(default)]
    pub network_interfaces: Vec<NetworkCounter>,
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
        AuthStatus::decl(),
        OkResponse::decl(),
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
