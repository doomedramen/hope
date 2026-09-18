//! Wire protocol types shared between the server's agent gateway and the
//! agent binary (spec §7, ADR-0007 mTLS auth, ADR-0008 WebSocket transport).
//!
//! Every message carries a monotonically-meaningless but unique `message_id`
//! (for correlation/logging) and a `protocol_version` so the server and
//! agent can detect skew before parsing the rest of the envelope.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current protocol version. Bump when making a breaking change to the
/// message envelope or any message variant.
pub const PROTOCOL_VERSION: u32 = 1;

/// Top-level envelope sent in both directions over the agent WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// First message an agent sends after the transport (mTLS) handshake
    /// completes, identifying itself and its capabilities.
    Hello(Hello),
    /// Server's reply to `Hello`, confirming acceptance and any directives.
    HelloAck(HelloAck),
    /// Periodic liveness/metrics report from the agent.
    Heartbeat(Heartbeat),
    /// Server's acknowledgement of a heartbeat.
    HeartbeatAck(HeartbeatAck),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub agent_id: Uuid,
    pub agent_version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelloAck {
    pub accepted: bool,
    pub server_version: String,
    pub heartbeat_interval_secs: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub agent_id: Uuid,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatAck {
    pub server_time_unix_secs: i64,
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
    fn hello_roundtrip() {
        roundtrip(Message::Hello(Hello {
            agent_id: Uuid::new_v4(),
            agent_version: "0.1.0".into(),
            hostname: "box1".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
        }));
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
}
