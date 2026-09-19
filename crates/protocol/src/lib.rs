//! Wire protocol types shared between the server's agent gateway and the
//! agent binary (spec §7, ADR-0007 mTLS auth, ADR-0008 WebSocket transport).
//!
//! M5 adds capability negotiation, bounded inventory snapshots, and bounded
//! observations as additive message variants. Hello and heartbeat keep their
//! original Rust construction shape so older peers can continue to use the
//! liveness path while the JSON hello advertises the compiled capabilities.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

/// Legacy envelope version. Additive M5 messages remain readable by peers
/// that only understand the original hello/heartbeat messages.
pub const LEGACY_PROTOCOL_VERSION: u32 = 1;
/// Current protocol version. Bump when changing the envelope or changing the
/// meaning of an existing message.
pub const PROTOCOL_VERSION: u32 = 2;
/// Versions accepted by this implementation, ordered from oldest to newest.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[LEGACY_PROTOCOL_VERSION, PROTOCOL_VERSION];

/// Current schema version for M5 snapshot and observation payloads.
pub const M5_SCHEMA_VERSION: u32 = 1;
pub const MAX_CAPABILITIES: usize = 32;
pub const MAX_PROTOCOL_VERSIONS: usize = 8;
pub const MAX_COLLECTORS: usize = 16;
pub const MAX_OBSERVATIONS: usize = 256;
pub const MAX_STRING_BYTES: usize = 512;
pub const MAX_COLLECTOR_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_OBSERVATION_BATCH_BYTES: usize = 64 * 1024;
pub const MAX_ENVELOPE_BYTES: usize = MAX_SNAPSHOT_BYTES + 16 * 1024;

/// Top-level envelope sent in both directions over the agent WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub message_id: Uuid,
    pub protocol_version: u32,
    #[serde(flatten)]
    pub message: Message,
}

impl Envelope {
    pub fn new(message: Message) -> Self {
        Self {
            message_id: Uuid::new_v4(),
            protocol_version: PROTOCOL_VERSION,
            message,
        }
    }

    /// Build an envelope using an explicitly selected protocol version.
    pub fn with_protocol_version(protocol_version: u32, message: Message) -> Self {
        Self {
            message_id: Uuid::new_v4(),
            protocol_version,
            message,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        self.message.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// First message an agent sends after the transport (mTLS) handshake
    /// completes, identifying itself. Kept unchanged for old Rust peers.
    Hello(Hello),
    /// Server's reply to `Hello`, confirming acceptance and any directives.
    /// Kept unchanged for old Rust peers.
    HelloAck(HelloAck),
    /// Periodic liveness/metrics report from the agent.
    Heartbeat(Heartbeat),
    /// Server's acknowledgement of a heartbeat.
    HeartbeatAck(HeartbeatAck),
    /// Additive M5 capability offer. An older server can ignore this message
    /// and still process hello/heartbeat.
    CapabilityOffer(CapabilityOffer),
    /// Server's selected protocol version and capability intersection.
    CapabilityAck(CapabilityAck),
    /// Idempotent-friendly, bounded host inventory snapshot.
    InventorySnapshot(InventorySnapshot),
    /// Server acknowledgement for an inventory snapshot.
    InventorySnapshotAck(InventorySnapshotAck),
    /// Bounded observations associated with one snapshot collection.
    ObservationBatch(ObservationBatch),
}

/// Existing Rust fields stay source-compatible with the current server. The
/// manual serializer adds the M5 JSON-only `capabilities` string array.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Hello {
    pub agent_id: Uuid,
    pub agent_version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
}

impl Hello {
    pub fn capabilities(&self) -> Vec<Capability> {
        default_agent_capabilities()
    }
}

impl Serialize for Hello {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct WireHello<'a> {
            agent_id: Uuid,
            agent_version: &'a str,
            hostname: &'a str,
            os: &'a str,
            arch: &'a str,
            capabilities: Vec<&'static str>,
        }

        WireHello {
            agent_id: self.agent_id,
            agent_version: &self.agent_version,
            hostname: &self.hostname,
            os: &self.os,
            arch: &self.arch,
            capabilities: default_agent_capabilities()
                .into_iter()
                .map(Capability::as_str)
                .collect(),
        }
        .serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelloAck {
    pub accepted: bool,
    pub server_version: String,
    pub heartbeat_interval_secs: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {
    pub agent_id: Uuid,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeartbeatAck {
    pub server_time_unix_secs: i64,
}

/// M5 capability names. Collector capabilities are separate so a server can
/// accept host inventory while declining an unavailable Docker or systemd
/// collector.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    InventorySnapshots,
    BoundedObservations,
    Host,
    Network,
    Filesystem,
    Systemd,
    Packages,
    Processes,
    Sockets,
    Docker,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventorySnapshots => "inventory_snapshots",
            Self::BoundedObservations => "bounded_observations",
            Self::Host => "host",
            Self::Network => "network",
            Self::Filesystem => "filesystem",
            Self::Systemd => "systemd",
            Self::Packages => "packages",
            Self::Processes => "processes",
            Self::Sockets => "sockets",
            Self::Docker => "docker",
        }
    }
}

/// Capabilities and protocol versions sent by an agent after hello.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityOffer {
    pub supported_protocol_versions: Vec<u32>,
    pub capabilities: Vec<Capability>,
}

/// Server response to [`CapabilityOffer`]. Missing fields are not added to
/// `HelloAck`, preserving source compatibility for existing servers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityAck {
    pub accepted: bool,
    pub selected_protocol_version: Option<u32>,
    pub capabilities: Vec<Capability>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedCapabilities {
    pub protocol_version: u32,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    NoCommonProtocolVersion,
}

impl fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommonProtocolVersion => f.write_str("no common protocol version"),
        }
    }
}

impl std::error::Error for NegotiationError {}

/// Select highest common protocol version and preserve agent capability order
/// while intersecting with server capabilities.
pub fn negotiate_capabilities(
    agent_versions: &[u32],
    server_versions: &[u32],
    agent_capabilities: &[Capability],
    server_capabilities: &[Capability],
) -> Result<NegotiatedCapabilities, NegotiationError> {
    let server_versions: BTreeSet<_> = server_versions.iter().copied().collect();
    let protocol_version = agent_versions
        .iter()
        .copied()
        .filter(|version| server_versions.contains(version))
        .max()
        .ok_or(NegotiationError::NoCommonProtocolVersion)?;

    let server_capabilities: BTreeSet<_> = server_capabilities.iter().copied().collect();
    let mut seen = BTreeSet::new();
    let capabilities = agent_capabilities
        .iter()
        .copied()
        .filter(|capability| server_capabilities.contains(capability) && seen.insert(*capability))
        .take(MAX_CAPABILITIES)
        .collect();

    Ok(NegotiatedCapabilities {
        protocol_version,
        capabilities,
    })
}

pub fn default_agent_capabilities() -> Vec<Capability> {
    vec![
        Capability::InventorySnapshots,
        Capability::BoundedObservations,
        Capability::Host,
        Capability::Network,
        Capability::Filesystem,
        Capability::Systemd,
        Capability::Packages,
        Capability::Processes,
        Capability::Sockets,
        Capability::Docker,
    ]
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectorSnapshot {
    pub capability: Capability,
    pub status: CollectorStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Wire shape for `type: inventory_snapshot`. The fields below intentionally
/// remain at envelope level after serde's internally tagged enum flattening.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventorySnapshot {
    pub agent_id: Uuid,
    pub sequence: u64,
    pub collected_at_unix_secs: i64,
    pub inventory: serde_json::Value,
    pub complete: bool,
    /// Stable when retransmitting this exact snapshot. Server can use this or
    /// `(agent_id, sequence)` as its idempotency key.
    pub snapshot_id: Uuid,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventorySnapshotAck {
    pub snapshot_id: Uuid,
    pub accepted: bool,
    pub sequence: u64,
    pub replayed: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Ok,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    /// Server deduplicates retransmission using this key within agent scope.
    pub idempotency_key: String,
    pub key: String,
    pub source: String,
    pub observed_at_unix_secs: i64,
    pub state: ObservationState,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservationBatch {
    pub schema_version: u32,
    /// Reuse this ID when retrying the same batch.
    pub batch_id: Uuid,
    pub agent_id: Uuid,
    pub collected_at_unix_secs: i64,
    pub observations: Vec<Observation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

fn validate_string(field: &str, value: &str) -> Result<(), ValidationError> {
    if value.len() > MAX_STRING_BYTES {
        return Err(ValidationError::new(format!(
            "{field} exceeds {MAX_STRING_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_capabilities(capabilities: &[Capability], field: &str) -> Result<(), ValidationError> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(ValidationError::new(format!(
            "{field} exceeds {MAX_CAPABILITIES} entries"
        )));
    }
    Ok(())
}

impl CapabilityOffer {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.supported_protocol_versions.len() > MAX_PROTOCOL_VERSIONS {
            return Err(ValidationError::new(format!(
                "supported_protocol_versions exceeds {MAX_PROTOCOL_VERSIONS} entries"
            )));
        }
        validate_capabilities(&self.capabilities, "capabilities")
    }
}

impl CapabilityAck {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_capabilities(&self.capabilities, "capabilities")?;
        if let Some(reason) = &self.reason {
            validate_string("reason", reason)?;
        }
        Ok(())
    }
}

impl InventorySnapshot {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.inventory.is_object() {
            return Err(ValidationError::new("inventory must be a JSON object"));
        }
        let bytes = serde_json::to_vec(&self.inventory)
            .map_err(|err| ValidationError::new(format!("invalid inventory payload: {err}")))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(ValidationError::new(format!(
                "inventory exceeds {MAX_SNAPSHOT_BYTES} bytes"
            )));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|err| ValidationError::new(format!("invalid snapshot: {err}")))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(ValidationError::new(format!(
                "snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

impl ObservationBatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.observations.len() > MAX_OBSERVATIONS {
            return Err(ValidationError::new(format!(
                "observations exceeds {MAX_OBSERVATIONS} entries"
            )));
        }
        for observation in &self.observations {
            validate_string("observation idempotency_key", &observation.idempotency_key)?;
            validate_string("observation key", &observation.key)?;
            validate_string("observation source", &observation.source)?;
            let bytes = serde_json::to_vec(&observation.value)
                .map_err(|err| ValidationError::new(format!("invalid observation value: {err}")))?;
            if bytes.len() > MAX_COLLECTOR_PAYLOAD_BYTES {
                return Err(ValidationError::new(format!(
                    "observation value exceeds {MAX_COLLECTOR_PAYLOAD_BYTES} bytes"
                )));
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|err| ValidationError::new(format!("invalid observation batch: {err}")))?;
        if bytes.len() > MAX_OBSERVATION_BATCH_BYTES {
            return Err(ValidationError::new(format!(
                "observation batch exceeds {MAX_OBSERVATION_BATCH_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

impl Message {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Hello(hello) => {
                validate_string("agent_version", &hello.agent_version)?;
                validate_string("hostname", &hello.hostname)?;
                validate_string("os", &hello.os)?;
                validate_string("arch", &hello.arch)
            }
            Self::HelloAck(ack) => {
                validate_string("server_version", &ack.server_version)?;
                if let Some(reason) = &ack.reason {
                    validate_string("reason", reason)?;
                }
                Ok(())
            }
            Self::Heartbeat(_) | Self::HeartbeatAck(_) => Ok(()),
            Self::CapabilityOffer(offer) => offer.validate(),
            Self::CapabilityAck(ack) => ack.validate(),
            Self::InventorySnapshot(snapshot) => snapshot.validate(),
            Self::InventorySnapshotAck(ack) => {
                if let Some(reason) = &ack.reason {
                    validate_string("reason", reason)?;
                }
                Ok(())
            }
            Self::ObservationBatch(batch) => batch.validate(),
        }
    }
}

/// Serialize only protocol messages that pass their declared size bounds.
pub fn serialize_envelope(envelope: &Envelope) -> Result<String, ValidationError> {
    envelope.validate()?;
    let bytes = serde_json::to_vec(envelope)
        .map_err(|err| ValidationError::new(format!("serialize envelope: {err}")))?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(ValidationError::new(format!(
            "envelope exceeds {MAX_ENVELOPE_BYTES} bytes"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|err| ValidationError::new(format!("serialize envelope as UTF-8: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: Message) {
        let envelope = Envelope::new(msg);
        let json = serde_json::to_string(&envelope).expect("serialize");
        let decoded: Envelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(envelope, decoded);
        assert_eq!(decoded.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn hello_roundtrip_and_advertises_capabilities() {
        let hello = Hello {
            agent_id: Uuid::new_v4(),
            agent_version: "0.1.0".into(),
            hostname: "box1".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
        };
        let value = serde_json::to_value(&hello).expect("serialize hello");
        assert!(value["capabilities"].is_array());
        assert!(
            value["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "docker")
        );
        roundtrip(Message::Hello(hello));
    }

    #[test]
    fn hello_ack_roundtrip() {
        roundtrip(Message::HelloAck(HelloAck {
            accepted: true,
            server_version: "0.1.0".into(),
            heartbeat_interval_secs: 30,
            reason: None,
        }));
    }

    #[test]
    fn heartbeat_roundtrip() {
        roundtrip(Message::Heartbeat(Heartbeat {
            agent_id: Uuid::new_v4(),
            uptime_secs: 42,
        }));
    }

    #[test]
    fn heartbeat_ack_roundtrip() {
        roundtrip(Message::HeartbeatAck(HeartbeatAck {
            server_time_unix_secs: 1_700_000_000,
        }));
    }

    #[test]
    fn m5_messages_roundtrip_with_contract_field_names() {
        let agent_id = Uuid::new_v4();
        let snapshot_id = Uuid::new_v4();
        let envelope = Envelope::new(Message::InventorySnapshot(InventorySnapshot {
            agent_id,
            sequence: 7,
            collected_at_unix_secs: 1_700_000_000,
            inventory: serde_json::json!({"host": {"hostname": "box1"}}),
            complete: true,
            snapshot_id,
            schema_version: M5_SCHEMA_VERSION,
        }));
        let value = serde_json::to_value(&envelope).expect("serialize snapshot");
        assert_eq!(value["type"], "inventory_snapshot");
        assert_eq!(value["agent_id"], agent_id.to_string());
        assert_eq!(value["sequence"], 7);
        assert!(value["inventory"].is_object());
        assert_eq!(value["complete"], true);
        assert_eq!(value["message_id"].as_str().unwrap().len(), 36);
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
        let decoded: Envelope = serde_json::from_value(value).expect("decode snapshot");
        assert_eq!(decoded, envelope);

        roundtrip(Message::CapabilityOffer(CapabilityOffer {
            supported_protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            capabilities: default_agent_capabilities(),
        }));
        roundtrip(Message::CapabilityAck(CapabilityAck {
            accepted: true,
            selected_protocol_version: Some(PROTOCOL_VERSION),
            capabilities: vec![Capability::InventorySnapshots],
            reason: None,
        }));
        roundtrip(Message::ObservationBatch(ObservationBatch {
            schema_version: M5_SCHEMA_VERSION,
            batch_id: snapshot_id,
            agent_id,
            collected_at_unix_secs: 1_700_000_000,
            observations: vec![Observation {
                idempotency_key: "snapshot/host".into(),
                key: "host.collector".into(),
                source: "agent".into(),
                observed_at_unix_secs: 1_700_000_000,
                state: ObservationState::Ok,
                value: serde_json::json!({"status": "available"}),
            }],
        }));
    }

    #[test]
    fn envelope_has_unique_message_ids() {
        let a = Envelope::new(Message::Heartbeat(Heartbeat {
            agent_id: Uuid::new_v4(),
            uptime_secs: 1,
        }));
        let b = Envelope::new(Message::Heartbeat(Heartbeat {
            agent_id: Uuid::new_v4(),
            uptime_secs: 1,
        }));
        assert_ne!(a.message_id, b.message_id);
    }

    #[test]
    fn envelope_tag_is_type_field() {
        let envelope = Envelope::new(Message::Hello(Hello {
            agent_id: Uuid::new_v4(),
            agent_version: "0.1.0".into(),
            hostname: "box1".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
        }));
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["type"], "hello");
    }

    #[test]
    fn capability_negotiation_selects_highest_common_version_and_intersection() {
        let result = negotiate_capabilities(
            &[1, 2],
            &[1, 2, 3],
            &[
                Capability::InventorySnapshots,
                Capability::Docker,
                Capability::Docker,
            ],
            &[Capability::Docker, Capability::Host],
        )
        .expect("common version");

        assert_eq!(result.protocol_version, 2);
        assert_eq!(result.capabilities, vec![Capability::Docker]);
    }

    #[test]
    fn capability_negotiation_rejects_no_common_version() {
        assert_eq!(
            negotiate_capabilities(&[2], &[1], &[], &[]),
            Err(NegotiationError::NoCommonProtocolVersion)
        );
    }

    #[test]
    fn snapshot_and_observation_bounds_are_enforced() {
        let large_inventory = serde_json::json!({
            "data": "x".repeat(MAX_SNAPSHOT_BYTES)
        });
        let snapshot = InventorySnapshot {
            agent_id: Uuid::new_v4(),
            sequence: 0,
            collected_at_unix_secs: 0,
            inventory: large_inventory,
            complete: false,
            snapshot_id: Uuid::new_v4(),
            schema_version: M5_SCHEMA_VERSION,
        };
        assert!(snapshot.validate().is_err());

        let batch = ObservationBatch {
            schema_version: M5_SCHEMA_VERSION,
            batch_id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            collected_at_unix_secs: 0,
            observations: (0..=MAX_OBSERVATIONS)
                .map(|index| Observation {
                    idempotency_key: index.to_string(),
                    key: "key".into(),
                    source: "agent".into(),
                    observed_at_unix_secs: 0,
                    state: ObservationState::Ok,
                    value: serde_json::Value::Null,
                })
                .collect(),
        };
        assert!(batch.validate().is_err());
    }

    #[test]
    fn old_hello_json_still_decodes() {
        let json = r#"{
            "message_id":"00000000-0000-0000-0000-000000000001",
            "protocol_version":1,
            "type":"hello",
            "agent_id":"00000000-0000-0000-0000-000000000002",
            "agent_version":"0.1.0",
            "hostname":"box1",
            "os":"linux",
            "arch":"x86_64"
        }"#;
        let envelope: Envelope = serde_json::from_str(json).expect("legacy hello");
        assert!(matches!(envelope.message, Message::Hello(_)));
        assert_eq!(envelope.protocol_version, LEGACY_PROTOCOL_VERSION);
    }
}
