//! Agent runtime: connect to the gateway over mTLS WebSocket, send Hello,
//! then heartbeat on a fixed interval (ADR-0007/0008). Reconnects with
//! exponential backoff + jitter on any error or disconnect; the backoff
//! counter resets after a session is successfully established.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use rustls::RootCertStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::collectors;
use crate::identity::{Paths, write_private};
use protocol::{
    Capability, CapabilityAck, CapabilityOffer, Envelope, Heartbeat, Hello, InventorySnapshot,
    MAX_OBSERVATION_BATCH_BYTES, MAX_SNAPSHOT_BYTES, Message, ObservationBatch,
    SUPPORTED_PROTOCOL_VERSIONS, negotiate_capabilities,
};

const MAX_BACKOFF_SECS: u64 = 60;
const SNAPSHOT_SEQUENCE_FILE: &str = "snapshot-sequence";
const PENDING_SNAPSHOT_FILE: &str = "pending-snapshot.json";
const MAX_PENDING_SNAPSHOT_BYTES: usize = MAX_SNAPSHOT_BYTES + MAX_OBSERVATION_BATCH_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingSnapshot {
    snapshot: InventorySnapshot,
    observations: ObservationBatch,
}

/// Base backoff delay (before jitter) for the given zero-indexed retry
/// attempt: `2^attempt` seconds, capped at `MAX_BACKOFF_SECS`. Pure and
/// deterministic so it can be unit tested independent of the RNG.
fn backoff_base_secs(attempt: u32) -> u64 {
    2u64.saturating_pow(attempt).min(MAX_BACKOFF_SECS)
}

/// Full-jitter backoff delay: a random duration in `[0, base]`, where
/// `base` is `backoff_base_secs(attempt)`. Full jitter (rather than
/// e.g. +/-50%) avoids synchronized reconnect storms across many agents.
fn backoff_delay(attempt: u32) -> Duration {
    let base = backoff_base_secs(attempt);
    let jittered = rand::thread_rng().gen_range(0..=base.max(1));
    Duration::from_secs(jittered)
}

pub async fn run(gateway_url: &str, state_dir: &str) -> anyhow::Result<()> {
    let mut attempt: u32 = 0;

    loop {
        match run_session(gateway_url, state_dir).await {
            Ok(()) => {
                tracing::info!("gateway session ended cleanly; reconnecting");
                attempt = 0;
            }
            Err(err) => {
                tracing::warn!(error = %err, attempt, "gateway session failed; reconnecting");
            }
        }

        let delay = backoff_delay(attempt);
        tracing::debug!(delay_secs = delay.as_secs(), "waiting before reconnect");
        tokio::time::sleep(delay).await;
        attempt = attempt.saturating_add(1);
    }
}

async fn run_session(gateway_url: &str, state_dir: &str) -> anyhow::Result<()> {
    let paths = Paths::new(state_dir);
    if !paths.exist() {
        anyhow::bail!(
            "agent is not enrolled (run `agent enroll` first): missing files under {state_dir}"
        );
    }

    let cert_pem = std::fs::read_to_string(&paths.cert)?;
    let key_pem = std::fs::read_to_string(&paths.key)?;
    let ca_pem = std::fs::read_to_string(&paths.ca)?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>()?;
    let agent_id = load_or_create_agent_id(state_dir, certs.first().map(|cert| cert.as_ref()))?;
    let pending_snapshot = load_or_collect_pending_snapshot(state_dir, agent_id).await?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", paths.key.display()))?;

    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert?)?;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)?;

    let (ws_stream, _response) = tokio_tungstenite::connect_async_tls_with_config(
        gateway_url,
        None,
        false,
        Some(Connector::Rustls(Arc::new(tls_config))),
    )
    .await?;

    tracing::info!(gateway_url, "connected to agent gateway");

    let (mut write, mut read) = ws_stream.split();

    let hello = Envelope::new(Message::Hello(Hello {
        agent_id,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: hostname_or_unknown(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }));
    write
        .send(WsMessage::Text(protocol::serialize_envelope(&hello)?))
        .await?;

    let offer = CapabilityOffer {
        supported_protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        capabilities: collectors::capabilities(),
    };
    let offer_envelope = Envelope::new(Message::CapabilityOffer(offer.clone()));
    write
        .send(WsMessage::Text(protocol::serialize_envelope(
            &offer_envelope,
        )?))
        .await?;

    let start = Instant::now();
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    heartbeat_interval.tick().await; // first tick fires immediately
    let mut snapshot_sent = false;
    let mut pending_snapshot = Some(pending_snapshot);

    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                let hb = Envelope::new(Message::Heartbeat(Heartbeat {
                    agent_id,
                    uptime_secs: start.elapsed().as_secs(),
                }));
                write.send(WsMessage::Text(protocol::serialize_envelope(&hb)?)).await?;
            }
            msg = read.next() => {
                let Some(msg) = msg else {
                    tracing::warn!("gateway closed the connection");
                    break;
                };
                let msg = msg?;
                if let WsMessage::Text(text) = msg {
                    match serde_json::from_str::<Envelope>(&text) {
                        Ok(envelope) => match envelope.message {
                            Message::CapabilityAck(ack) => {
                                if let Some(negotiated) = negotiated_from_ack(&offer, &ack) {
                                    tracing::info!(
                                        protocol_version = negotiated.protocol_version,
                                        capabilities = ?negotiated.capabilities,
                                        "agent capabilities negotiated"
                                    );
                                    if !snapshot_sent
                                        && negotiated
                                            .capabilities
                                            .contains(&Capability::InventorySnapshots)
                                    {
                                        let pending = pending_snapshot
                                            .as_ref()
                                            .expect("pending snapshot exists until acknowledged");
                                        let snapshot_message = Envelope::new(
                                            Message::InventorySnapshot(pending.snapshot.clone()),
                                        );
                                        write
                                            .send(WsMessage::Text(
                                                protocol::serialize_envelope(&snapshot_message)?,
                                            ))
                                            .await?;
                                        if negotiated
                                            .capabilities
                                            .contains(&Capability::BoundedObservations)
                                        {
                                            let observation_message = Envelope::new(
                                                Message::ObservationBatch(
                                                    pending.observations.clone(),
                                                ),
                                            );
                                            write
                                                .send(WsMessage::Text(
                                                    protocol::serialize_envelope(
                                                        &observation_message,
                                                    )?,
                                                ))
                                                .await?;
                                        }
                                        snapshot_sent = true;
                                    }
                                } else {
                                    tracing::warn!(
                                        ?ack,
                                        "gateway rejected or returned invalid capability negotiation"
                                    );
                                }
                            }
                            Message::InventorySnapshotAck(ack) => {
                                let acknowledged = pending_snapshot.as_ref().is_some_and(
                                    |pending| pending.snapshot.snapshot_id == ack.snapshot_id,
                                );
                                if ack.accepted && acknowledged {
                                    clear_pending_snapshot(state_dir)?;
                                    pending_snapshot = None;
                                    snapshot_sent = false;
                                }
                            }
                            other => tracing::info!(?other, "received from gateway"),
                        },
                        Err(err) => tracing::warn!(error = %err, "malformed envelope from gateway"),
                    }
                } else if msg.is_close() {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn load_or_collect_pending_snapshot(
    state_dir: &str,
    agent_id: Uuid,
) -> anyhow::Result<PendingSnapshot> {
    if let Some(pending) = load_pending_snapshot(state_dir)? {
        if pending.snapshot.agent_id != agent_id {
            anyhow::bail!(
                "pending snapshot belongs to agent {}, not enrolled agent {agent_id}",
                pending.snapshot.agent_id
            );
        }
        return Ok(pending);
    }

    let sequence = next_snapshot_sequence(state_dir)?;
    let mut snapshot = collectors::collect_snapshot(agent_id).await;
    snapshot.sequence = sequence;
    let observations = collectors::observations_from_snapshot(&snapshot);
    let pending = PendingSnapshot {
        snapshot,
        observations,
    };
    persist_pending_snapshot(state_dir, &pending)?;
    Ok(pending)
}

fn pending_snapshot_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join(PENDING_SNAPSHOT_FILE)
}

fn snapshot_sequence_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join(SNAPSHOT_SEQUENCE_FILE)
}

fn load_pending_snapshot(state_dir: &str) -> anyhow::Result<Option<PendingSnapshot>> {
    let path = pending_snapshot_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)?;
    if contents.len() > MAX_PENDING_SNAPSHOT_BYTES {
        anyhow::bail!(
            "pending snapshot {} exceeds {MAX_PENDING_SNAPSHOT_BYTES} bytes",
            path.display()
        );
    }
    let pending: PendingSnapshot = serde_json::from_str(&contents)?;
    pending
        .snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid pending snapshot: {error}"))?;
    pending
        .observations
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid pending observations: {error}"))?;
    Ok(Some(pending))
}

fn persist_pending_snapshot(state_dir: &str, pending: &PendingSnapshot) -> anyhow::Result<()> {
    let encoded = serde_json::to_string(pending)?;
    if encoded.len() > MAX_PENDING_SNAPSHOT_BYTES {
        anyhow::bail!("pending snapshot exceeds {MAX_PENDING_SNAPSHOT_BYTES} bytes");
    }
    let path = pending_snapshot_path(state_dir);
    let temporary = path.with_extension("json.tmp");
    write_private(&temporary, &encoded)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn clear_pending_snapshot(state_dir: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(pending_snapshot_path(state_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn next_snapshot_sequence(state_dir: &str) -> anyhow::Result<u64> {
    let path = snapshot_sequence_path(state_dir);
    let current = if path.exists() {
        std::fs::read_to_string(&path)?.trim().parse::<u64>()?
    } else {
        0
    };
    let next = current
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("snapshot sequence exhausted"))?;
    write_private(&path, &format!("{next}\n"))?;
    Ok(next)
}

fn negotiated_from_ack(
    offer: &CapabilityOffer,
    ack: &CapabilityAck,
) -> Option<protocol::NegotiatedCapabilities> {
    if !ack.accepted {
        return None;
    }
    let selected = ack.selected_protocol_version?;
    if !offer.supported_protocol_versions.contains(&selected) {
        return None;
    }
    negotiate_capabilities(
        &offer.supported_protocol_versions,
        &[selected],
        &offer.capabilities,
        &ack.capabilities,
    )
    .ok()
}

fn agent_id_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join("agent-id")
}

/// Enrollment persists the client certificate. Persist a derived UUID beside
/// it on first run, then always load that value on reconnect. The certificate
/// hash gives first-run identity a deterministic fallback without generating a
/// new identity for every WebSocket session.
fn load_or_create_agent_id(
    state_dir: &str,
    certificate_der: Option<&[u8]>,
) -> anyhow::Result<Uuid> {
    let path = agent_id_path(state_dir);
    if path.exists() {
        let value = std::fs::read_to_string(&path)?;
        return Uuid::parse_str(value.trim()).map_err(|err| {
            anyhow::anyhow!("invalid persisted agent identity {}: {err}", path.display())
        });
    }

    let certificate_der = certificate_der
        .ok_or_else(|| anyhow::anyhow!("enrolled certificate contains no leaf certificate"))?;
    let agent_id = stable_agent_id_from_certificate(certificate_der);
    write_private(&path, &format!("{agent_id}\n"))?;
    Ok(agent_id)
}

fn stable_agent_id_from_certificate(certificate_der: &[u8]) -> Uuid {
    let digest = Sha256::digest(certificate_der);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Mark as a UUID v5-shaped, RFC 4122 variant identifier. This is a
    // deterministic local identity, not a replacement for mTLS authority.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn hostname_or_unknown() -> String {
    if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            return hostname.to_string();
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_base_grows_exponentially_then_caps() {
        assert_eq!(backoff_base_secs(0), 1);
        assert_eq!(backoff_base_secs(1), 2);
        assert_eq!(backoff_base_secs(2), 4);
        assert_eq!(backoff_base_secs(3), 8);
        assert_eq!(backoff_base_secs(4), 16);
        assert_eq!(backoff_base_secs(5), 32);
        assert_eq!(backoff_base_secs(6), MAX_BACKOFF_SECS); // 64 -> capped
        assert_eq!(backoff_base_secs(20), MAX_BACKOFF_SECS);
        assert_eq!(backoff_base_secs(u32::MAX), MAX_BACKOFF_SECS);
    }

    #[test]
    fn jittered_delay_never_exceeds_base() {
        for attempt in 0..10 {
            let base = backoff_base_secs(attempt);
            for _ in 0..50 {
                let delay = backoff_delay(attempt).as_secs();
                assert!(delay <= base, "delay {delay} exceeded base {base}");
            }
        }
    }

    #[test]
    fn stable_identity_uses_certificate_bytes() {
        let first = stable_agent_id_from_certificate(b"certificate-a");
        let second = stable_agent_id_from_certificate(b"certificate-a");
        let other = stable_agent_id_from_certificate(b"certificate-b");
        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn persisted_identity_wins_on_reconnect() {
        let directory = std::env::temp_dir().join(format!("hope-agent-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create state directory");
        let path = directory.join("agent-id");
        let expected = Uuid::new_v4();
        std::fs::write(&path, format!("{expected}\n")).expect("write identity");

        let actual = load_or_create_agent_id(directory.to_str().unwrap(), Some(b"different"))
            .expect("load identity");
        assert_eq!(actual, expected);
        std::fs::remove_dir_all(directory).expect("remove test state directory");
    }

    #[test]
    fn snapshot_sequence_is_persistent_and_monotonic() {
        let directory = std::env::temp_dir().join(format!("hope-agent-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create state directory");

        assert_eq!(
            next_snapshot_sequence(directory.to_str().unwrap()).unwrap(),
            1
        );
        assert_eq!(
            next_snapshot_sequence(directory.to_str().unwrap()).unwrap(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(directory.join(SNAPSHOT_SEQUENCE_FILE)).unwrap(),
            "2\n"
        );

        std::fs::remove_dir_all(directory).expect("remove test state directory");
    }
}
