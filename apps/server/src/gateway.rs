//! Agent gateway: mTLS WebSocket listener (ADR-0007 client-cert auth,
//! ADR-0008 WebSocket transport). Every connection must present a client
//! certificate signed by the internal CA; the certificate's SHA-256
//! fingerprint must map to a known, non-revoked agent.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::agent_inventory::{self, AgentInventoryError};
use crate::agents;
use crate::config::Config;
use crate::pki;
use protocol::{CapabilityAck, CapabilityOffer, Envelope, Message};

fn build_server_config(config: &Config) -> anyhow::Result<ServerConfig> {
    let cert_pem = std::fs::read_to_string(&config.server_cert_path)?;
    let key_pem = std::fs::read_to_string(&config.server_key_path)?;
    let ca_pem = std::fs::read_to_string(&config.ca_cert_path)?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", config.server_key_path))?;

    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert?)?;
    }

    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;

    let server_config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)?;

    Ok(server_config)
}

pub async fn serve(config: Config, pool: PgPool) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_config = build_server_config(&config)?;
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind(&config.gateway_bind_addr).await?;
    tracing::info!(addr = %config.gateway_bind_addr, "agent gateway listening");

    loop {
        let (tcp, peer_addr) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(error = %err, "gateway accept error");
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let pool = pool.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_connection(acceptor, tcp, pool).await {
                tracing::warn!(peer = %peer_addr, error = %err, "gateway connection ended with error");
            }
        });
    }
}

async fn handle_connection(
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    pool: PgPool,
) -> anyhow::Result<()> {
    let tls_stream = acceptor.accept(tcp).await?;

    let peer_certs = tls_stream
        .get_ref()
        .1
        .peer_certificates()
        .map(|certs| certs.to_vec())
        .unwrap_or_default();

    let Some(leaf) = peer_certs.first() else {
        anyhow::bail!("no client certificate presented");
    };

    let fingerprint = pki::fingerprint_der(leaf);

    let agent = agents::find_by_fingerprint(&pool, &fingerprint).await?;
    let Some(agent) = agent else {
        anyhow::bail!("unknown agent certificate: {fingerprint}");
    };
    if agent.revoked_at.is_some() {
        anyhow::bail!("revoked agent certificate: {fingerprint}");
    }

    let ws_stream = tokio_tungstenite::accept_async(tls_stream).await?;
    let (mut write, mut read) = ws_stream.split();

    while let Some(msg) = read.next().await {
        let msg = msg?;
        let WsMessage::Text(text) = msg else {
            if msg.is_close() {
                break;
            }
            continue;
        };

        if text.len() > agent_inventory::MAX_MESSAGE_BYTES {
            let error = protocol_error(None, "payload_too_large", "agent message is too large");
            write.send(WsMessage::Text(error)).await?;
            break;
        }

        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(error = %err, "malformed envelope from agent");
                let error =
                    protocol_error(None, "malformed_json", "agent message is not valid JSON");
                write.send(WsMessage::Text(error)).await?;
                continue;
            }
        };
        let message_id = value
            .get("message_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        if message_id.is_none() {
            let error = protocol_error(
                None,
                "missing_message_id",
                "agent message must contain a valid message_id",
            );
            write.send(WsMessage::Text(error)).await?;
            continue;
        }
        let Some(message_type) = value.get("type").and_then(Value::as_str) else {
            let error = protocol_error(message_id, "missing_type", "agent message has no type");
            write.send(WsMessage::Text(error)).await?;
            continue;
        };
        let Some(protocol_version) = value.get("protocol_version").and_then(Value::as_u64) else {
            let error = protocol_error(
                message_id,
                "missing_protocol_version",
                "agent message has no protocol_version",
            );
            write.send(WsMessage::Text(error)).await?;
            continue;
        };
        let protocol_version = u32::try_from(protocol_version).unwrap_or(u32::MAX);
        if !(agent_inventory::SUPPORTED_PROTOCOL_MIN..=agent_inventory::SUPPORTED_PROTOCOL_MAX)
            .contains(&protocol_version)
        {
            let response = protocol_error(
                message_id,
                "unsupported_protocol",
                "agent protocol version is not supported",
            );
            write.send(WsMessage::Text(response)).await?;
            continue;
        }

        match message_type {
            "hello" => {
                let hello = match agent_inventory::parse_hello(&value, protocol_version) {
                    Ok(hello) => hello,
                    Err(error) => {
                        let reason = error.to_string();
                        let response = protocol_error(message_id, "hello_rejected", &reason);
                        write.send(WsMessage::Text(response)).await?;
                        break;
                    }
                };
                if hello.agent_id != agent.id {
                    let response = protocol_error(
                        message_id,
                        "agent_identity_mismatch",
                        "hello agent_id does not match mTLS certificate",
                    );
                    write.send(WsMessage::Text(response)).await?;
                    break;
                }
                let capabilities = serde_json::to_value(&hello.capabilities)?;
                agents::update_hello(
                    &pool,
                    agent.id,
                    &agents::HelloMetadata {
                        agent_version: &hello.agent_version,
                        hostname: &hello.hostname,
                        os: &hello.os,
                        arch: &hello.arch,
                        protocol_version: i32::try_from(protocol_version).unwrap_or(i32::MAX),
                        capabilities: &capabilities,
                    },
                )
                .await?;
                tracing::info!(agent_id = %agent.id, capabilities = hello.capabilities.len(), "agent hello");
                let ack = Envelope::with_protocol_version(
                    protocol_version,
                    Message::HelloAck(protocol::HelloAck {
                        accepted: true,
                        server_version: env!("CARGO_PKG_VERSION").to_string(),
                        heartbeat_interval_secs: 30,
                        reason: None,
                    }),
                );
                write
                    .send(WsMessage::Text(serde_json::to_string(&ack)?))
                    .await?;
            }
            "capability_offer" => {
                let offer: CapabilityOffer = match serde_json::from_value(value) {
                    Ok(offer) => offer,
                    Err(error) => {
                        let response = protocol_error(
                            message_id,
                            "malformed_capability_offer",
                            &error.to_string(),
                        );
                        write.send(WsMessage::Text(response)).await?;
                        continue;
                    }
                };
                if let Err(error) = offer.validate() {
                    let response =
                        protocol_error(message_id, "invalid_capability_offer", &error.to_string());
                    write.send(WsMessage::Text(response)).await?;
                    continue;
                }

                let negotiated = protocol::negotiate_capabilities(
                    &offer.supported_protocol_versions,
                    protocol::SUPPORTED_PROTOCOL_VERSIONS,
                    &offer.capabilities,
                    agent_inventory::SUPPORTED_CAPABILITIES,
                );
                let (accepted, selected_protocol_version, capabilities, reason) = match negotiated {
                    Ok(negotiated) => (
                        true,
                        Some(negotiated.protocol_version),
                        negotiated.capabilities,
                        None,
                    ),
                    Err(error) => (false, None, Vec::new(), Some(error.to_string())),
                };
                let ack_protocol_version = selected_protocol_version.unwrap_or(protocol_version);
                let ack = Envelope::with_protocol_version(
                    ack_protocol_version,
                    Message::CapabilityAck(CapabilityAck {
                        accepted,
                        selected_protocol_version,
                        capabilities,
                        reason,
                    }),
                );
                write
                    .send(WsMessage::Text(serde_json::to_string(&ack)?))
                    .await?;
            }
            "heartbeat" => {
                let heartbeat: protocol::Heartbeat = match serde_json::from_value(value) {
                    Ok(heartbeat) => heartbeat,
                    Err(error) => {
                        let response =
                            protocol_error(message_id, "malformed_heartbeat", &error.to_string());
                        write.send(WsMessage::Text(response)).await?;
                        continue;
                    }
                };
                if heartbeat.agent_id != agent.id {
                    let response = protocol_error(
                        message_id,
                        "agent_identity_mismatch",
                        "heartbeat agent_id does not match mTLS certificate",
                    );
                    write.send(WsMessage::Text(response)).await?;
                    break;
                }
                agents::touch_last_seen(&pool, agent.id).await?;
                let ack = Envelope::with_protocol_version(
                    protocol_version,
                    Message::HeartbeatAck(protocol::HeartbeatAck {
                        server_time_unix_secs: time::OffsetDateTime::now_utc().unix_timestamp(),
                    }),
                );
                write
                    .send(WsMessage::Text(serde_json::to_string(&ack)?))
                    .await?;
            }
            "inventory_snapshot" => {
                let snapshot = match agent_inventory::parse_snapshot_value(value) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        let code = match error {
                            AgentInventoryError::UnsupportedProtocol(_) => "unsupported_protocol",
                            AgentInventoryError::PayloadTooLarge => "payload_too_large",
                            AgentInventoryError::AgentIdentityMismatch => "agent_identity_mismatch",
                            _ => "inventory_rejected",
                        };
                        let response = protocol_error(message_id, code, &error.to_string());
                        write.send(WsMessage::Text(response)).await?;
                        continue;
                    }
                };
                match agent_inventory::ingest_snapshot(&pool, agent.id, &snapshot).await {
                    Ok(outcome) => {
                        let ack = Envelope::new(Message::InventorySnapshotAck(
                            protocol::InventorySnapshotAck {
                                snapshot_id: snapshot.snapshot_id,
                                accepted: true,
                                sequence: outcome.sequence,
                                replayed: outcome.replayed,
                                reason: None,
                            },
                        ));
                        write
                            .send(WsMessage::Text(serde_json::to_string(&ack)?))
                            .await?;
                    }
                    Err(error @ AgentInventoryError::StaleSnapshot { .. }) => {
                        let response = protocol_error(
                            Some(snapshot.message_id),
                            "stale_snapshot",
                            &error.to_string(),
                        );
                        write.send(WsMessage::Text(response)).await?;
                    }
                    Err(error @ AgentInventoryError::UnknownAgent(_))
                    | Err(error @ AgentInventoryError::RevokedAgent(_)) => {
                        let response = protocol_error(
                            Some(snapshot.message_id),
                            "agent_rejected",
                            &error.to_string(),
                        );
                        write.send(WsMessage::Text(response)).await?;
                        break;
                    }
                    Err(error) => {
                        let response = protocol_error(
                            Some(snapshot.message_id),
                            "inventory_rejected",
                            &error.to_string(),
                        );
                        write.send(WsMessage::Text(response)).await?;
                    }
                }
            }
            "hello_ack" | "heartbeat_ack" | "inventory_snapshot_ack" | "protocol_error" => {
                tracing::debug!(agent_id = %agent.id, message_type, "ignored server message from agent");
            }
            other => {
                tracing::debug!(agent_id = %agent.id, message_type = other, "unhandled message from agent");
            }
        }
    }

    Ok(())
}

fn protocol_error(message_id: Option<Uuid>, code: &str, reason: &str) -> String {
    json!({
        "type": "protocol_error",
        "message_id": message_id.unwrap_or_else(Uuid::new_v4),
        "protocol_version": protocol::PROTOCOL_VERSION,
        "accepted": false,
        "code": code,
        "reason": reason,
    })
    .to_string()
}
