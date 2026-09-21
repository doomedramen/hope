//! Agent runtime: prove its enrolled identity over WebSocket, send Hello,
//! then heartbeat and periodic inventory refresh (ADR-0007/0008). Reconnects
//! with exponential backoff + jitter on any error or disconnect; the backoff
//! counter resets after a session is successfully established.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use futures_util::{Sink, SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest, Sha256};
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use uuid::Uuid;

use crate::collectors;
use crate::identity::{Paths, write_private};
use crate::pinning::PinnedFingerprintVerifier;
use protocol::{
    Capability, CapabilityAck, CapabilityOffer, Envelope, Heartbeat, Hello, InventorySnapshot,
    InventorySnapshotAck, MAX_OBSERVATION_BATCH_BYTES, MAX_SNAPSHOT_BYTES, Message,
    ObservationBatch, SUPPORTED_PROTOCOL_VERSIONS, negotiate_capabilities,
};

const MAX_BACKOFF_SECS: u64 = 60;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Inventory refresh cadence for an established session. Collectors can run
/// bounded host probes, so inventory refreshes stay slower than heartbeats.
const INVENTORY_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
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

fn inventory_refresh_interval() -> tokio::time::Interval {
    let connected_at = tokio::time::Instant::now();
    let mut interval = tokio::time::interval_at(
        inventory_refresh_deadline(connected_at),
        INVENTORY_REFRESH_INTERVAL,
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval
}

fn inventory_refresh_deadline(connected_at: tokio::time::Instant) -> tokio::time::Instant {
    connected_at + INVENTORY_REFRESH_INTERVAL
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

    let agent_id = Uuid::parse_str(std::fs::read_to_string(&paths.agent_id)?.trim())?;
    let identity: [u8; 32] = hex::decode(std::fs::read_to_string(&paths.identity_key)?.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid agent identity key"))?;
    let signing_key = SigningKey::from_bytes(&identity);
    let trust = std::fs::read_to_string(&paths.tls_trust)?;
    let pending_snapshot = load_or_collect_pending_snapshot(state_dir, agent_id).await?;
    let pinned_config = if let Some(fingerprint) = trust.trim().strip_prefix("pinned:") {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Some(Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinnedFingerprintVerifier::new(
                    fingerprint.to_string(),
                    provider,
                )))
                .with_no_client_auth(),
        ))
    } else if trust.trim() == "system" {
        None
    } else {
        anyhow::bail!("invalid agent TLS trust mode");
    };
    let mut challenge_url = reqwest::Url::parse(gateway_url)?;
    challenge_url
        .set_scheme("https")
        .map_err(|_| anyhow::anyhow!("invalid agent gateway URL"))?;
    challenge_url.set_path(&format!("/agent/v1/challenge/{agent_id}"));
    challenge_url.set_query(None);
    let challenge_client = if let Some(config) = pinned_config.as_ref() {
        reqwest::Client::builder()
            .use_preconfigured_tls((**config).clone())
            .build()?
    } else {
        reqwest::Client::new()
    };
    let response = challenge_client.get(challenge_url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("agent challenge failed: {}", response.status());
    }
    let challenge: serde_json::Value = response.json().await?;
    let nonce = challenge["nonce"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("agent challenge response is invalid"))?;
    let message = format!("hope-agent-connect-v1\n{agent_id}\n{nonce}");
    let signature = hex::encode(signing_key.sign(message.as_bytes()).to_bytes());
    let mut request = gateway_url.into_client_request()?;
    request.headers_mut().insert(
        "x-hope-agent-id",
        HeaderValue::from_str(&agent_id.to_string())?,
    );
    request
        .headers_mut()
        .insert("x-hope-challenge", HeaderValue::from_str(nonce)?);
    request
        .headers_mut()
        .insert("x-hope-signature", HeaderValue::from_str(&signature)?);

    let (ws_stream, _response) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        pinned_config.map(Connector::Rustls),
    )
    .await?;

    tracing::info!(gateway_url, "connected to agent gateway");

    let (mut write, mut read) = ws_stream.split();

    let hello = Envelope::new(Message::Hello(Hello {
        agent_id,
        agent_version: option_env!("HOPE_AGENT_RELEASE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string(),
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
    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.tick().await; // first tick fires immediately
    let mut inventory_refresh = inventory_refresh_interval();
    let mut snapshot_sent = false;
    let mut pending_snapshot = Some(pending_snapshot);
    let mut snapshot_acknowledged = false;
    let mut observations_acknowledged = false;
    let mut negotiated_capabilities: Option<protocol::NegotiatedCapabilities> = None;

    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                let hb = Envelope::new(Message::Heartbeat(Heartbeat {
                    agent_id,
                    uptime_secs: start.elapsed().as_secs(),
                }));
                write.send(WsMessage::Text(protocol::serialize_envelope(&hb)?)).await?;
            }
            _ = inventory_refresh.tick() => {
                let Some(negotiated) = negotiated_capabilities.as_ref() else {
                    continue;
                };
                if !negotiated.capabilities.contains(&Capability::InventorySnapshots) {
                    continue;
                }

                if pending_snapshot.is_none() {
                    pending_snapshot = Some(load_or_collect_pending_snapshot(state_dir, agent_id).await?);
                    snapshot_acknowledged = false;
                    observations_acknowledged = !negotiated
                        .capabilities
                        .contains(&Capability::BoundedObservations);
                }

                let pending = pending_snapshot
                    .as_ref()
                    .expect("pending snapshot exists after refresh collection");
                send_pending_snapshot(
                    &mut write,
                    pending,
                    negotiated.protocol_version,
                    &negotiated.capabilities,
                )
                .await?;
                snapshot_sent = true;
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
                                    let inventory_enabled = negotiated
                                        .capabilities
                                        .contains(&Capability::InventorySnapshots);
                                    observations_acknowledged = !negotiated
                                        .capabilities
                                        .contains(&Capability::BoundedObservations);
                                    negotiated_capabilities = Some(negotiated);
                                    if !snapshot_sent
                                        && inventory_enabled
                                        && let (Some(pending), Some(negotiated)) = (
                                            pending_snapshot.as_ref(),
                                            negotiated_capabilities.as_ref(),
                                        )
                                    {
                                        send_pending_snapshot(
                                            &mut write,
                                            pending,
                                            negotiated.protocol_version,
                                            &negotiated.capabilities,
                                        )
                                        .await?;
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
                                if ack.accepted
                                    && pending_snapshot.as_ref().is_some_and(|pending| {
                                        pending.snapshot.snapshot_id == ack.snapshot_id
                                    })
                                {
                                    snapshot_acknowledged = true;
                                }
                                if acknowledge_pending_snapshot(
                                    state_dir,
                                    &mut pending_snapshot,
                                    &ack,
                                    observations_acknowledged,
                                )? {
                                    snapshot_sent = false;
                                    snapshot_acknowledged = false;
                                    observations_acknowledged = false;
                                }
                            }
                            Message::ObservationBatchAck(ack) => {
                                if ack.accepted
                                    && pending_snapshot.as_ref().is_some_and(|pending| {
                                        pending.observations.batch_id == ack.batch_id
                                    })
                                {
                                    observations_acknowledged = true;
                                }
                                if acknowledge_pending_observations(
                                    state_dir,
                                    &mut pending_snapshot,
                                    &ack,
                                    snapshot_acknowledged,
                                )? {
                                    snapshot_sent = false;
                                    snapshot_acknowledged = false;
                                    observations_acknowledged = false;
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

async fn send_pending_snapshot<S>(
    write: &mut S,
    pending: &PendingSnapshot,
    protocol_version: u32,
    capabilities: &[Capability],
) -> anyhow::Result<()>
where
    S: Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    let snapshot_message = Envelope::with_protocol_version(
        protocol_version,
        Message::InventorySnapshot(pending.snapshot.clone()),
    );
    write
        .send(WsMessage::Text(protocol::serialize_envelope(
            &snapshot_message,
        )?))
        .await
        .map_err(|error| anyhow::anyhow!("send inventory snapshot: {error}"))?;

    if capabilities.contains(&Capability::BoundedObservations) {
        let observation_message = Envelope::with_protocol_version(
            protocol_version,
            Message::ObservationBatch(pending.observations.clone()),
        );
        write
            .send(WsMessage::Text(protocol::serialize_envelope(
                &observation_message,
            )?))
            .await
            .map_err(|error| anyhow::anyhow!("send inventory observations: {error}"))?;
    }

    Ok(())
}

fn acknowledge_pending_snapshot(
    state_dir: &str,
    pending_snapshot: &mut Option<PendingSnapshot>,
    ack: &InventorySnapshotAck,
    observations_acknowledged: bool,
) -> anyhow::Result<bool> {
    let acknowledged = pending_snapshot
        .as_ref()
        .is_some_and(|pending| pending.snapshot.snapshot_id == ack.snapshot_id);
    if !ack.accepted || !acknowledged || !observations_acknowledged {
        return Ok(false);
    }

    clear_pending_snapshot(state_dir)?;
    *pending_snapshot = None;
    Ok(true)
}

fn acknowledge_pending_observations(
    state_dir: &str,
    pending_snapshot: &mut Option<PendingSnapshot>,
    ack: &protocol::ObservationBatchAck,
    snapshot_acknowledged: bool,
) -> anyhow::Result<bool> {
    let acknowledged = pending_snapshot
        .as_ref()
        .is_some_and(|pending| pending.observations.batch_id == ack.batch_id);
    if !ack.accepted || !acknowledged || !snapshot_acknowledged {
        return Ok(false);
    }

    clear_pending_snapshot(state_dir)?;
    *pending_snapshot = None;
    Ok(true)
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

#[cfg(test)]
fn agent_id_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join("agent-id")
}

/// Enrollment persists the client certificate. Persist a derived UUID beside
/// it on first run, then always load that value on reconnect. The certificate
/// hash gives first-run identity a deterministic fallback without generating a
/// new identity for every WebSocket session.
#[cfg(test)]
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

#[cfg(test)]
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

    #[test]
    fn inventory_refresh_interval_starts_after_configured_period() {
        let connected_at = tokio::time::Instant::now();
        let first_refresh = inventory_refresh_deadline(connected_at);

        assert_eq!(first_refresh - connected_at, INVENTORY_REFRESH_INTERVAL);
        assert_eq!(INVENTORY_REFRESH_INTERVAL, Duration::from_secs(15 * 60));
    }

    #[test]
    fn pending_snapshot_is_reused_until_ack_then_refreshes_sequence() {
        let directory = std::env::temp_dir().join(format!("hope-agent-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create state directory");
        let state_dir = directory.to_str().expect("state directory path");
        let agent_id = Uuid::from_u128(1);
        let first = PendingSnapshot {
            snapshot: InventorySnapshot {
                agent_id,
                sequence: 1,
                collected_at_unix_secs: 123,
                inventory: serde_json::json!({"host": {"hostname": "test-agent"}}),
                complete: true,
                snapshot_id: Uuid::from_u128(2),
                schema_version: protocol::M5_SCHEMA_VERSION,
            },
            observations: ObservationBatch {
                schema_version: protocol::M5_SCHEMA_VERSION,
                batch_id: Uuid::from_u128(2),
                agent_id,
                collected_at_unix_secs: 123,
                observations: Vec::new(),
            },
        };
        std::fs::write(snapshot_sequence_path(state_dir), "1\n").expect("write sequence");
        persist_pending_snapshot(state_dir, &first).expect("persist initial snapshot");

        let retry = load_pending_snapshot(state_dir)
            .expect("load pending snapshot")
            .expect("pending snapshot exists");
        assert_eq!(retry.snapshot, first.snapshot);
        assert_eq!(retry.observations, first.observations);
        assert_eq!(
            std::fs::read_to_string(snapshot_sequence_path(state_dir)).expect("read sequence"),
            "1\n"
        );

        let mut pending_snapshot = Some(retry);
        let rejected = InventorySnapshotAck {
            snapshot_id: first.snapshot.snapshot_id,
            accepted: false,
            sequence: first.snapshot.sequence,
            replayed: false,
            reason: Some("retry".into()),
        };
        assert!(
            !acknowledge_pending_snapshot(state_dir, &mut pending_snapshot, &rejected, false)
                .expect("keep rejected snapshot pending")
        );
        assert!(
            load_pending_snapshot(state_dir)
                .expect("load rejected snapshot")
                .is_some()
        );

        let mismatched = InventorySnapshotAck {
            snapshot_id: Uuid::from_u128(99),
            accepted: true,
            sequence: first.snapshot.sequence,
            replayed: false,
            reason: None,
        };
        assert!(
            !acknowledge_pending_snapshot(state_dir, &mut pending_snapshot, &mismatched, false)
                .expect("keep mismatched snapshot pending")
        );

        let acknowledged = InventorySnapshotAck {
            accepted: true,
            reason: None,
            ..rejected
        };
        assert!(
            !acknowledge_pending_snapshot(state_dir, &mut pending_snapshot, &acknowledged, false)
                .expect("acknowledge snapshot")
        );
        assert!(pending_snapshot.is_some());

        let observation_ack = protocol::ObservationBatchAck {
            batch_id: first.observations.batch_id,
            accepted: true,
            observation_count: 0,
            replayed: false,
            reason: None,
        };
        assert!(
            acknowledge_pending_observations(
                state_dir,
                &mut pending_snapshot,
                &observation_ack,
                true,
            )
            .expect("acknowledge observations")
        );
        assert!(pending_snapshot.is_none());
        assert!(
            load_pending_snapshot(state_dir)
                .expect("check cleared snapshot")
                .is_none()
        );

        assert_eq!(
            next_snapshot_sequence(state_dir).expect("advance sequence"),
            2
        );
        let refreshed = PendingSnapshot {
            snapshot: InventorySnapshot {
                sequence: 2,
                snapshot_id: Uuid::from_u128(3),
                ..first.snapshot
            },
            observations: ObservationBatch {
                batch_id: Uuid::from_u128(3),
                ..first.observations
            },
        };
        persist_pending_snapshot(state_dir, &refreshed).expect("persist refreshed snapshot");
        let refreshed = load_pending_snapshot(state_dir)
            .expect("load refreshed snapshot")
            .expect("refreshed snapshot exists");
        assert_eq!(refreshed.snapshot.sequence, 2);
        assert_eq!(refreshed.snapshot.snapshot_id, Uuid::from_u128(3));
        assert_eq!(
            std::fs::read_to_string(snapshot_sequence_path(state_dir)).expect("read sequence"),
            "2\n"
        );

        std::fs::remove_dir_all(directory).expect("remove test state directory");
    }
}
