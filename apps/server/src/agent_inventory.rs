//! M5 agent inventory protocol, projection, and authenticated read APIs.
//!
//! The gateway keeps the authenticated storage and projection boundary here.
//! The shared protocol crate owns the versioned wire types; these local types
//! retain compatibility with the first M5 snapshot shape while accepting the
//! additive snapshot identity fields used by current agents.

use std::net::IpAddr;
use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use domain::inventory::identity::{Identifier, IdentifierType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::inventory::events::Recorder;
use crate::inventory::evidence;
use crate::inventory::identity_service;
use crate::inventory::pagination::{ListParams, decode_cursor, effective_limit, encode_cursor};
use crate::state::AppState;

pub const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CAPABILITIES: usize = 128;
pub const MAX_CAPABILITY_BYTES: usize = 128;
pub const MAX_STRING_BYTES: usize = 16 * 1024;
pub const MAX_JSON_DEPTH: usize = 32;
pub const MAX_JSON_NODES: usize = 100_000;
pub const INVENTORY_HISTORY_LIMIT: i64 = 30;
pub const AGENT_OBSERVATION_HISTORY_LIMIT: i64 = 4_096;
pub const SUPPORTED_PROTOCOL_MIN: u32 = protocol::LEGACY_PROTOCOL_VERSION;
pub const SUPPORTED_PROTOCOL_MAX: u32 = protocol::PROTOCOL_VERSION;
pub const SUPPORTED_CAPABILITIES: &[protocol::Capability] = &[
    protocol::Capability::InventorySnapshots,
    protocol::Capability::BoundedObservations,
    protocol::Capability::ResourceMetrics,
];

#[derive(Debug, Error)]
pub enum AgentInventoryError {
    #[error("inventory payload exceeds {MAX_MESSAGE_BYTES} bytes")]
    PayloadTooLarge,
    #[error("observation batch payload exceeds the bounded batch size")]
    ObservationPayloadTooLarge,
    #[error("inventory payload has unsupported JSON shape: {0}")]
    InvalidPayload(String),
    #[error("unsupported agent protocol version {0}")]
    UnsupportedProtocol(u32),
    #[error("inventory snapshot agent id does not match authenticated certificate")]
    AgentIdentityMismatch,
    #[error("agent {0} is unknown")]
    UnknownAgent(Uuid),
    #[error("agent {0} is revoked")]
    RevokedAgent(Uuid),
    #[error("inventory snapshot sequence {sequence} is older than current sequence {current}")]
    StaleSnapshot { sequence: u64, current: u64 },
    #[error("inventory reconciliation did not produce a device")]
    ReconciliationMissingDevice,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HelloMessage {
    pub agent_id: Uuid,
    pub agent_version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InventorySnapshot {
    #[serde(rename = "type")]
    pub message_type: String,
    pub message_id: Uuid,
    pub protocol_version: u32,
    pub agent_id: Uuid,
    pub sequence: u64,
    pub collected_at_unix_secs: i64,
    pub inventory: Value,
    #[serde(default = "default_complete")]
    pub complete: bool,
    #[serde(default)]
    pub snapshot_id: Uuid,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParsedObservationBatch {
    #[serde(rename = "type")]
    pub message_type: String,
    pub message_id: Uuid,
    pub protocol_version: u32,
    #[serde(flatten)]
    pub batch: protocol::ObservationBatch,
}

fn default_complete() -> bool {
    true
}

fn default_schema_version() -> u32 {
    protocol::M5_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestOutcome {
    pub snapshot_id: Uuid,
    pub sequence: u64,
    pub device_id: Option<Uuid>,
    pub replayed: bool,
    pub current: bool,
    pub reconciliation: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObservationIngestOutcome {
    pub batch_id: Uuid,
    pub observation_count: usize,
    pub accepted_count: usize,
    pub replayed: bool,
}

/// Parse and validate Hello's additive capability field. The mTLS identity is
/// checked by the gateway after parsing, not by this function.
pub fn parse_hello(
    value: &Value,
    protocol_version: u32,
) -> Result<HelloMessage, AgentInventoryError> {
    ensure_supported_protocol(protocol_version)?;
    let hello: HelloMessage = serde_json::from_value(value.clone())?;
    validate_text("agent_version", &hello.agent_version, 256)?;
    validate_text("hostname", &hello.hostname, 256)?;
    validate_text("os", &hello.os, 128)?;
    validate_text("arch", &hello.arch, 128)?;
    if hello.capabilities.len() > MAX_CAPABILITIES {
        return Err(AgentInventoryError::InvalidPayload(format!(
            "capabilities exceeds {MAX_CAPABILITIES} entries"
        )));
    }
    for capability in &hello.capabilities {
        validate_text("capability", capability, MAX_CAPABILITY_BYTES)?;
    }
    Ok(hello)
}

pub fn parse_snapshot_value(value: Value) -> Result<InventorySnapshot, AgentInventoryError> {
    let encoded = serde_json::to_vec(&value)?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err(AgentInventoryError::PayloadTooLarge);
    }
    let snapshot: InventorySnapshot = serde_json::from_value(value)?;
    validate_snapshot(&snapshot)
}

pub fn parse_observation_batch_value(
    value: Value,
) -> Result<ParsedObservationBatch, AgentInventoryError> {
    let batch: ParsedObservationBatch = serde_json::from_value(value)?;
    let encoded_batch = serde_json::to_vec(&batch.batch)?;
    if encoded_batch.len() > protocol::MAX_OBSERVATION_BATCH_BYTES {
        return Err(AgentInventoryError::ObservationPayloadTooLarge);
    }
    validate_observation_batch(&batch)
}

fn validate_observation_batch(
    parsed: &ParsedObservationBatch,
) -> Result<ParsedObservationBatch, AgentInventoryError> {
    if parsed.message_type != "observation_batch" {
        return Err(AgentInventoryError::InvalidPayload(format!(
            "expected type observation_batch, got {}",
            parsed.message_type
        )));
    }
    ensure_observation_protocol(parsed.protocol_version)?;
    if parsed.message_id.is_nil() {
        return Err(AgentInventoryError::InvalidPayload(
            "message_id must not be nil".to_string(),
        ));
    }
    parsed
        .batch
        .validate()
        .map_err(|error| AgentInventoryError::InvalidPayload(error.to_string()))?;
    OffsetDateTime::from_unix_timestamp(parsed.batch.collected_at_unix_secs).map_err(|error| {
        AgentInventoryError::InvalidPayload(format!("invalid collected_at_unix_secs: {error}"))
    })?;
    for observation in &parsed.batch.observations {
        OffsetDateTime::from_unix_timestamp(observation.observed_at_unix_secs).map_err(
            |error| {
                AgentInventoryError::InvalidPayload(format!(
                    "invalid observed_at_unix_secs: {error}"
                ))
            },
        )?;
        let mut nodes = 0;
        validate_json_tree(&observation.value, 0, &mut nodes)?;
    }
    Ok(parsed.clone())
}

fn validate_snapshot(
    snapshot: &InventorySnapshot,
) -> Result<InventorySnapshot, AgentInventoryError> {
    if snapshot.message_type != "inventory_snapshot" {
        return Err(AgentInventoryError::InvalidPayload(format!(
            "expected type inventory_snapshot, got {}",
            snapshot.message_type
        )));
    }
    ensure_supported_protocol(snapshot.protocol_version)?;
    if snapshot.message_id.is_nil() {
        return Err(AgentInventoryError::InvalidPayload(
            "message_id must not be nil".to_string(),
        ));
    }
    i64::try_from(snapshot.sequence).map_err(|_| {
        AgentInventoryError::InvalidPayload("sequence exceeds signed database range".to_string())
    })?;
    OffsetDateTime::from_unix_timestamp(snapshot.collected_at_unix_secs).map_err(|error| {
        AgentInventoryError::InvalidPayload(format!("invalid collected_at_unix_secs: {error}"))
    })?;
    if !snapshot.inventory.is_object() {
        return Err(AgentInventoryError::InvalidPayload(
            "inventory must be a JSON object".to_string(),
        ));
    }
    let mut nodes = 0;
    validate_json_tree(&snapshot.inventory, 0, &mut nodes)?;
    Ok(snapshot.clone())
}

fn ensure_supported_protocol(version: u32) -> Result<(), AgentInventoryError> {
    if !(SUPPORTED_PROTOCOL_MIN..=SUPPORTED_PROTOCOL_MAX).contains(&version) {
        return Err(AgentInventoryError::UnsupportedProtocol(version));
    }
    Ok(())
}

fn ensure_observation_protocol(version: u32) -> Result<(), AgentInventoryError> {
    ensure_supported_protocol(version)?;
    if version != protocol::PROTOCOL_VERSION {
        return Err(AgentInventoryError::UnsupportedProtocol(version));
    }
    Ok(())
}

fn validate_json_tree(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), AgentInventoryError> {
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err(AgentInventoryError::InvalidPayload(
            "inventory contains too many JSON nodes".to_string(),
        ));
    }
    if depth > MAX_JSON_DEPTH {
        return Err(AgentInventoryError::InvalidPayload(
            "inventory JSON nesting is too deep".to_string(),
        ));
    }
    match value {
        Value::String(text) => validate_text("inventory string", text, MAX_STRING_BYTES),
        Value::Array(values) => {
            if values.len() > MAX_JSON_NODES {
                return Err(AgentInventoryError::InvalidPayload(
                    "inventory array is too large".to_string(),
                ));
            }
            for value in values {
                validate_json_tree(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if map.len() > MAX_JSON_NODES {
                return Err(AgentInventoryError::InvalidPayload(
                    "inventory object is too large".to_string(),
                ));
            }
            for (key, value) in map {
                validate_text("inventory key", key, MAX_STRING_BYTES)?;
                validate_json_tree(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<(), AgentInventoryError> {
    if value.is_empty() || value.len() > maximum {
        return Err(AgentInventoryError::InvalidPayload(format!(
            "{field} must contain 1..={maximum} bytes"
        )));
    }
    Ok(())
}

/// Ingest one authenticated snapshot. The history insert and current pointer
/// update are idempotent on `(agent_id, message_id)` and `(agent_id, sequence)`.
/// A lower sequence is retained only as history and never projects stale data.
pub async fn ingest_snapshot(
    pool: &PgPool,
    authenticated_agent_id: Uuid,
    snapshot: &InventorySnapshot,
) -> Result<IngestOutcome, AgentInventoryError> {
    validate_snapshot(snapshot)?;
    if snapshot.agent_id != authenticated_agent_id {
        return Err(AgentInventoryError::AgentIdentityMismatch);
    }
    let sequence = i64::try_from(snapshot.sequence).map_err(|_| {
        AgentInventoryError::InvalidPayload("sequence exceeds signed database range".to_string())
    })?;
    let source_snapshot_id = if snapshot.snapshot_id.is_nil() {
        snapshot.message_id
    } else {
        snapshot.snapshot_id
    };

    let agent: Option<(String, Option<OffsetDateTime>)> =
        sqlx::query_as("select cert_fingerprint, revoked_at from agents where id = $1")
            .bind(authenticated_agent_id)
            .fetch_optional(pool)
            .await?;
    let Some((cert_fingerprint, revoked_at)) = agent else {
        return Err(AgentInventoryError::UnknownAgent(authenticated_agent_id));
    };
    if revoked_at.is_some() {
        return Err(AgentInventoryError::RevokedAgent(authenticated_agent_id));
    }

    // M1's identity service gives agent_id deterministic weight 1.0. This
    // makes a retry/reconnect attach to the same canonical device while still
    // preserving weaker identifiers as evidence for operator review.
    let identifiers = host_identifiers(authenticated_agent_id, &cert_fingerprint, snapshot);
    // Seed the deterministic agent identity first. A stale/shared MAC can
    // otherwise make the existing M1 scorer return a review suggestion before
    // it sees the agent_id candidate, leaving an authenticated host without a
    // canonical device.
    let deterministic = identity_service::reconcile(pool, &identifiers[..1], None).await?;
    let reconciliation = identity_service::reconcile(pool, &identifiers, None).await?;
    let reconciliation = if reconciliation.device_id.is_some() {
        reconciliation
    } else {
        deterministic
    };
    let device_id = reconciliation
        .device_id
        .ok_or(AgentInventoryError::ReconciliationMissingDevice)?;
    let reconciliation_json = json!({
        "decision": reconciliation.decision,
        "device_id": device_id,
        "suggestion_id": reconciliation.suggestion_id,
        "explanation": reconciliation.explanation,
    });

    let mut tx = pool.begin().await?;
    let existing_message: Option<(Uuid, i64)> = sqlx::query_as(
        "select id, sequence from agent_inventory_snapshots \
         where agent_id = $1 and (message_id = $2 or source_snapshot_id = $3) \
         order by id limit 1",
    )
    .bind(authenticated_agent_id)
    .bind(snapshot.message_id)
    .bind(source_snapshot_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((snapshot_id, existing_sequence)) = existing_message {
        tx.commit().await?;
        return Ok(IngestOutcome {
            snapshot_id,
            sequence: u64::try_from(existing_sequence).unwrap_or_default(),
            device_id: Some(device_id),
            replayed: true,
            current: true,
            reconciliation: reconciliation_json,
        });
    }

    let current_sequence: Option<(i64,)> = sqlx::query_as(
        "select sequence from agent_inventory_current where agent_id = $1 for update",
    )
    .bind(authenticated_agent_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((current,)) = current_sequence
        && sequence <= current
    {
        tx.commit().await?;
        return Err(AgentInventoryError::StaleSnapshot {
            sequence: snapshot.sequence,
            current: u64::try_from(current).unwrap_or_default(),
        });
    }

    let collected_at = OffsetDateTime::from_unix_timestamp(snapshot.collected_at_unix_secs)
        .map_err(|error| AgentInventoryError::InvalidPayload(error.to_string()))?;
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "insert into agent_inventory_snapshots \
            (agent_id, message_id, source_snapshot_id, protocol_version, sequence, collected_at, inventory, complete) \
         values ($1, $2, $3, $4, $5, $6, $7, $8) \
         on conflict (agent_id, sequence) do nothing returning id",
    )
    .bind(authenticated_agent_id)
    .bind(snapshot.message_id)
    .bind(source_snapshot_id)
    .bind(i32::try_from(snapshot.protocol_version).unwrap_or(i32::MAX))
    .bind(sequence)
    .bind(collected_at)
    .bind(&snapshot.inventory)
    .bind(snapshot.complete)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((snapshot_id,)) = inserted else {
        let existing: Option<(Uuid, i64, Uuid)> = sqlx::query_as(
            "select id, sequence, source_snapshot_id from agent_inventory_snapshots \
             where agent_id = $1 and sequence = $2",
        )
        .bind(authenticated_agent_id)
        .bind(sequence)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((existing_id, existing_sequence, existing_source_snapshot_id)) = existing {
            if existing_source_snapshot_id == source_snapshot_id {
                return Ok(IngestOutcome {
                    snapshot_id: existing_id,
                    sequence: u64::try_from(existing_sequence).unwrap_or_default(),
                    device_id: Some(device_id),
                    replayed: true,
                    current: existing_sequence > 0,
                    reconciliation: reconciliation_json,
                });
            }
            if existing_sequence == sequence {
                return Err(AgentInventoryError::StaleSnapshot {
                    sequence: snapshot.sequence,
                    current: snapshot.sequence,
                });
            }
            return Ok(IngestOutcome {
                snapshot_id: existing_id,
                sequence: u64::try_from(existing_sequence).unwrap_or_default(),
                device_id: Some(device_id),
                replayed: true,
                current: true,
                reconciliation: reconciliation_json,
            });
        }
        return Err(AgentInventoryError::InvalidPayload(
            "snapshot sequence conflict could not be resolved".to_string(),
        ));
    };

    sqlx::query(
        "insert into agent_inventory_current \
            (agent_id, snapshot_id, message_id, source_snapshot_id, protocol_version, sequence, collected_at, \
             received_at, inventory, complete) \
         values ($1, $2, $3, $4, $5, $6, $7, now(), $8, $9) \
         on conflict (agent_id) do update set snapshot_id = excluded.snapshot_id, \
             message_id = excluded.message_id, source_snapshot_id = excluded.source_snapshot_id, \
             protocol_version = excluded.protocol_version, sequence = excluded.sequence, \
             collected_at = excluded.collected_at, received_at = now(), inventory = excluded.inventory, \
             complete = excluded.complete \
             where agent_inventory_current.sequence < excluded.sequence",
    )
    .bind(authenticated_agent_id)
    .bind(snapshot_id)
    .bind(snapshot.message_id)
    .bind(source_snapshot_id)
    .bind(i32::try_from(snapshot.protocol_version).unwrap_or(i32::MAX))
    .bind(sequence)
    .bind(collected_at)
    .bind(&snapshot.inventory)
    .bind(snapshot.complete)
    .execute(&mut *tx)
    .await?;

    sqlx::query("update agents set last_seen = now(), last_heartbeat_at = now() where id = $1")
        .bind(authenticated_agent_id)
        .execute(&mut *tx)
        .await?;
    recover_incident_in_tx(&mut tx, authenticated_agent_id).await?;

    project_inventory(
        &mut tx,
        authenticated_agent_id,
        device_id,
        snapshot,
        snapshot_id,
    )
    .await?;

    sqlx::query(
        "delete from agent_inventory_snapshots s \
         where s.agent_id = $1 \
           and s.id <> (select snapshot_id from agent_inventory_current where agent_id = $1) \
           and s.id not in (select id from agent_inventory_snapshots \
                            where agent_id = $1 order by sequence desc limit $2)",
    )
    .bind(authenticated_agent_id)
    .bind(INVENTORY_HISTORY_LIMIT)
    .execute(&mut *tx)
    .await?;

    Recorder::record_change(
        &mut tx,
        "agents",
        authenticated_agent_id,
        "agent.inventory.updated",
        "info",
        None,
        Some(json!({
            "snapshot_id": snapshot_id,
            "sequence": snapshot.sequence,
            "complete": snapshot.complete,
            "device_id": device_id,
        })),
        Some("agent"),
    )
    .await?;
    tx.commit().await?;

    Ok(IngestOutcome {
        snapshot_id,
        sequence: snapshot.sequence,
        device_id: Some(device_id),
        replayed: false,
        current: true,
        reconciliation: reconciliation_json,
    })
}

/// Ingest one authenticated observation batch. Each observation is retained as
/// an append-only raw row, while `(agent_id, idempotency_key)` makes retries
/// safe across reconnects and new transport envelopes.
pub async fn ingest_observation_batch(
    pool: &PgPool,
    authenticated_agent_id: Uuid,
    protocol_version: u32,
    batch: &protocol::ObservationBatch,
) -> Result<ObservationIngestOutcome, AgentInventoryError> {
    ensure_observation_protocol(protocol_version)?;
    batch
        .validate()
        .map_err(|error| AgentInventoryError::InvalidPayload(error.to_string()))?;
    if batch.agent_id != authenticated_agent_id {
        return Err(AgentInventoryError::AgentIdentityMismatch);
    }

    let agent: Option<(Option<OffsetDateTime>,)> =
        sqlx::query_as("select revoked_at from agents where id = $1")
            .bind(authenticated_agent_id)
            .fetch_optional(pool)
            .await?;
    let Some((revoked_at,)) = agent else {
        return Err(AgentInventoryError::UnknownAgent(authenticated_agent_id));
    };
    if revoked_at.is_some() {
        return Err(AgentInventoryError::RevokedAgent(authenticated_agent_id));
    }

    let collected_at = OffsetDateTime::from_unix_timestamp(batch.collected_at_unix_secs)
        .map_err(|error| AgentInventoryError::InvalidPayload(error.to_string()))?;
    let protocol_version = i32::try_from(protocol_version).unwrap_or(i32::MAX);
    let mut tx = pool.begin().await?;
    let mut accepted_count = 0;

    for observation in &batch.observations {
        let observed_at = OffsetDateTime::from_unix_timestamp(observation.observed_at_unix_secs)
            .map_err(|error| AgentInventoryError::InvalidPayload(error.to_string()))?;
        let result = sqlx::query(
            "insert into agent_observations \
                (agent_id, batch_id, idempotency_key, observation_key, source, protocol_version, \
                 collected_at, observed_at, state, value) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             on conflict (agent_id, idempotency_key) do nothing",
        )
        .bind(authenticated_agent_id)
        .bind(batch.batch_id)
        .bind(&observation.idempotency_key)
        .bind(&observation.key)
        .bind(&observation.source)
        .bind(protocol_version)
        .bind(collected_at)
        .bind(observed_at)
        .bind(observation_state_name(observation.state))
        .bind(&observation.value)
        .execute(&mut *tx)
        .await?;
        accepted_count += usize::try_from(result.rows_affected()).unwrap_or(usize::MAX);
    }

    sqlx::query("update agents set last_seen = now(), last_heartbeat_at = now() where id = $1")
        .bind(authenticated_agent_id)
        .execute(&mut *tx)
        .await?;
    recover_incident_in_tx(&mut tx, authenticated_agent_id).await?;

    // Keep raw agent observations useful for replay/debugging without turning
    // an active agent into an unbounded append-only storage workload.
    sqlx::query(
        "delete from agent_observations o \
         where o.agent_id = $1 \
           and o.id not in (select id from agent_observations \
                            where agent_id = $1 order by received_at desc, id desc limit $2)",
    )
    .bind(authenticated_agent_id)
    .bind(AGENT_OBSERVATION_HISTORY_LIMIT)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(ObservationIngestOutcome {
        batch_id: batch.batch_id,
        observation_count: batch.observations.len(),
        accepted_count,
        replayed: !batch.observations.is_empty() && accepted_count == 0,
    })
}

fn observation_state_name(state: protocol::ObservationState) -> &'static str {
    match state {
        protocol::ObservationState::Ok => "ok",
        protocol::ObservationState::Degraded => "degraded",
        protocol::ObservationState::Unavailable => "unavailable",
    }
}

async fn recover_incident_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
) -> sqlx::Result<()> {
    let recovered: Option<(Uuid,)> = sqlx::query_as(
        "update agent_health_incidents set state = 'recovered', recovered_at = now(), \
         last_event_at = now(), updated_at = now() \
         where agent_id = $1 and state = 'open' returning id",
    )
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((incident_id,)) = recovered {
        sqlx::query(
            "insert into change_events \
                (entity_kind, entity_id, category, severity, after, evidence_source) \
             values ('agents', $1, 'agent.heartbeat.recovered', 'notice', $2, 'agent')",
        )
        .bind(agent_id)
        .bind(json!({ "incident_id": incident_id, "state": "recovered" }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn host_identifiers(
    agent_id: Uuid,
    cert_fingerprint: &str,
    snapshot: &InventorySnapshot,
) -> Vec<Identifier> {
    let mut identifiers = vec![Identifier {
        rule_type: IdentifierType::AgentId,
        value: agent_id.to_string(),
        pinned: false,
    }];
    push_identifier(
        &mut identifiers,
        IdentifierType::TlsCertIdentity,
        cert_fingerprint,
    );
    for (kind, names) in [
        (
            IdentifierType::MachineId,
            &["machine_id", "machineId", "machine_id_sha256"] as &[&str],
        ),
        (
            IdentifierType::HardwareUuid,
            &["hardware_uuid", "hardwareUuid"],
        ),
        (IdentifierType::Hostname, &["hostname", "host_name"]),
    ] {
        if let Some(value) = inventory_string(&snapshot.inventory, &names) {
            push_identifier(&mut identifiers, kind, value);
        }
    }
    for section_name in ["system", "host", "identifiers"] {
        let Some(system) = snapshot.inventory.get(section_name) else {
            continue;
        };
        for (kind, names) in [
            (
                IdentifierType::MachineId,
                &["machine_id", "machineId", "machine_id_sha256"] as &[&str],
            ),
            (
                IdentifierType::HardwareUuid,
                &["hardware_uuid", "hardwareUuid"],
            ),
            (IdentifierType::Hostname, &["hostname", "host_name"]),
        ] {
            if let Some(value) = object_string(system, &names) {
                push_identifier(&mut identifiers, kind, value);
            }
        }
    }
    if let Some(interfaces) =
        first_array(&snapshot.inventory, &["interfaces", "network.interfaces"])
    {
        for interface in interfaces.iter().take(128) {
            if let Some(mac) = object_string(interface, &["mac", "mac_address", "macAddress"]) {
                push_identifier(&mut identifiers, IdentifierType::Mac, mac);
            }
        }
    }
    identifiers
}

fn push_identifier(identifiers: &mut Vec<Identifier>, kind: IdentifierType, value: &str) {
    if value.trim().is_empty() || value.len() > MAX_STRING_BYTES {
        return;
    }
    if !identifiers
        .iter()
        .any(|item| item.rule_type == kind && item.value == value)
    {
        identifiers.push(Identifier {
            rule_type: kind,
            value: value.to_string(),
            pinned: false,
        });
    }
}

async fn project_inventory(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
    device_id: Uuid,
    snapshot: &InventorySnapshot,
    snapshot_id: Uuid,
) -> sqlx::Result<()> {
    let source_instance = format!("agent:{agent_id}:snapshot:{}", snapshot.sequence);
    let hostname =
        inventory_string(&snapshot.inventory, &["hostname", "host_name"]).or_else(|| {
            snapshot
                .inventory
                .get("system")
                .and_then(|value| object_string(value, &["hostname", "host_name"]))
        });
    sqlx::query(
        "update devices set device_type = 'physical_host', name = coalesce($2, name), \
         updated_at = now(), version = version + 1 where id = $1",
    )
    .bind(device_id)
    .bind(hostname)
    .execute(&mut **tx)
    .await?;

    for (attribute, value) in host_evidence(snapshot, hostname) {
        evidence::record_automatic_tx(
            tx,
            "devices",
            device_id,
            "agent",
            Some(&source_instance),
            attribute,
            &value,
            0.9,
            false,
        )
        .await?;
    }

    let interfaces = first_array(&snapshot.inventory, &["interfaces", "network.interfaces"])
        .cloned()
        .unwrap_or_default();
    let mut interface_ids = Vec::new();
    for (index, interface) in interfaces.iter().enumerate() {
        let Some(object) = interface.as_object() else {
            continue;
        };
        let mac =
            object_string(interface, &["mac", "mac_address", "macAddress"]).and_then(normalize_mac);
        let name =
            object_string(interface, &["name", "interface", "description"]).unwrap_or("interface");
        let source_key = format!("agent:{agent_id}:interface:{name}:{index}");
        let interface_id: Option<Uuid> = sqlx::query_scalar(
            "select id from interfaces where device_id = $1 and agent_source_key = $2",
        )
        .bind(device_id)
        .bind(&source_key)
        .fetch_optional(&mut **tx)
        .await?;
        let interface_id = if let Some(id) = interface_id {
            sqlx::query(
                "update interfaces set mac = coalesce($2::macaddr, mac), description = $3, \
                 source_type = 'agent', last_seen = now(), updated_at = now(), version = version + 1 \
                 where id = $1",
            )
            .bind(id)
            .bind(mac.as_deref())
            .bind(name)
            .execute(&mut **tx)
            .await?;
            id
        } else {
            sqlx::query_scalar(
                "insert into interfaces (device_id, mac, description, source_type, agent_source_key) \
                 values ($1, $2::macaddr, $3, 'agent', $4) returning id",
            )
            .bind(device_id)
            .bind(mac.as_deref())
            .bind(name)
            .bind(&source_key)
            .fetch_one(&mut **tx)
            .await?
        };
        interface_ids.push(interface_id);

        let interface_value = json!(object);
        evidence::record_automatic_tx(
            tx,
            "interfaces",
            interface_id,
            "agent",
            Some(&source_instance),
            "interface",
            &interface_value,
            0.9,
            false,
        )
        .await?;

        let addresses = object
            .get("addresses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for address in addresses {
            let Some(ip) = address_value(&address) else {
                continue;
            };
            let address_type = object_string(&address, &["type", "address_type"])
                .filter(|value| matches!(*value, "static" | "dhcp" | "unknown"))
                .unwrap_or("unknown");
            let address_id = upsert_address(tx, interface_id, &ip, address_type).await?;
            evidence::record_automatic_tx(
                tx,
                "addresses",
                address_id,
                "agent",
                Some(&source_instance),
                "address",
                &json!({ "ip": ip, "address_type": address_type }),
                0.9,
                false,
            )
            .await?;
        }
    }

    let mut workloads = Vec::new();
    let mut containers = snapshot
        .inventory
        .get("containers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if containers.is_empty()
        && let Some(array) = snapshot
            .inventory
            .get("docker")
            .and_then(|docker| docker.get("containers"))
            .and_then(Value::as_array)
    {
        containers.extend(array.iter().cloned());
    }
    for container in &containers {
        let Some(object) = container.as_object() else {
            continue;
        };
        let runtime_id = object_string(
            container,
            &["id", "container_id", "containerId", "runtime_id"],
        );
        let name = object_string(container, &["name", "container_name"]);
        let source_key = format!(
            "agent:{agent_id}:container:{}",
            runtime_id.or(name).unwrap_or("unknown")
        );
        let workload_id: Option<Uuid> = sqlx::query_scalar(
            "select id from workloads where host_device_id = $1 and agent_source_key = $2",
        )
        .bind(device_id)
        .bind(&source_key)
        .fetch_optional(&mut **tx)
        .await?;
        let image = object_string(container, &["image", "image_or_template"]);
        let status = object_string(container, &["status", "state", "health"]);
        let workload_id = if let Some(id) = workload_id {
            sqlx::query(
                "update workloads set runtime_id = coalesce($2, runtime_id), name = coalesce($3, name), \
                 image_or_template = coalesce($4, image_or_template), status = coalesce($5, status), \
                 source_type = 'agent', updated_at = now(), version = version + 1 where id = $1",
            )
            .bind(id)
            .bind(runtime_id)
            .bind(name)
            .bind(image)
            .bind(status)
            .execute(&mut **tx)
            .await?;
            id
        } else {
            sqlx::query_scalar(
                "insert into workloads \
                    (workload_type, host_device_id, runtime_id, image_or_template, name, status, \
                     source_type, agent_source_key) \
                 values ('docker_container', $1, $2, $3, $4, $5, 'agent', $6) returning id",
            )
            .bind(device_id)
            .bind(runtime_id)
            .bind(image)
            .bind(name)
            .bind(status)
            .bind(&source_key)
            .fetch_one(&mut **tx)
            .await?
        };
        workloads.push((workload_id, source_key.clone()));
        upsert_containment(
            tx,
            "devices",
            device_id,
            "workloads",
            workload_id,
            "hosts_container",
        )
        .await?;
        evidence::record_automatic_tx(
            tx,
            "workloads",
            workload_id,
            "agent",
            Some(&source_instance),
            "container",
            &json!(object),
            0.9,
            false,
        )
        .await?;

        let ports = object
            .get("published_ports")
            .or_else(|| object.get("ports"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (port_index, port) in ports.iter().enumerate() {
            project_service_endpoint(
                tx,
                agent_id,
                device_id,
                Some(workload_id),
                &source_key,
                port,
                "published_port",
                port_index,
                &source_instance,
            )
            .await?;
        }
    }

    let sockets = first_array(
        &snapshot.inventory,
        &[
            "sockets.sockets",
            "sockets",
            "listening_sockets",
            "listening.services",
        ],
    )
    .cloned()
    .unwrap_or_default();
    for (index, socket) in sockets.iter().enumerate() {
        project_service_endpoint(
            tx,
            agent_id,
            device_id,
            None,
            &format!("agent:{agent_id}:host"),
            socket,
            "socket",
            index,
            &source_instance,
        )
        .await?;
    }

    let summary = json!({
        "snapshot_id": snapshot_id,
        "sequence": snapshot.sequence,
        "interfaces": interface_ids.len(),
        "workloads": workloads.len(),
        "sockets": sockets.len(),
        "complete": snapshot.complete,
    });
    evidence::record_automatic_tx(
        tx,
        "devices",
        device_id,
        "agent",
        Some(&source_instance),
        "inventory_summary",
        &summary,
        0.9,
        false,
    )
    .await?;

    Ok(())
}

fn host_evidence(
    snapshot: &InventorySnapshot,
    hostname: Option<&str>,
) -> Vec<(&'static str, Value)> {
    let mut values = Vec::new();
    if let Some(hostname) = hostname {
        values.push(("hostname", json!(hostname)));
    }
    for (attribute, names) in [
        ("machine_id", ["machine_id", "machineId"]),
        ("hardware_uuid", ["hardware_uuid", "hardwareUuid"]),
        ("boot_id", ["boot_id", "bootId"]),
        ("os", ["os", "operating_system"]),
        ("arch", ["arch", "architecture"]),
    ] {
        if let Some(value) = inventory_string(&snapshot.inventory, &names).or_else(|| {
            snapshot
                .inventory
                .get("system")
                .and_then(|item| object_string(item, &names))
        }) {
            values.push((attribute, json!(value)));
        }
    }
    values
}

async fn upsert_address(
    tx: &mut Transaction<'_, Postgres>,
    interface_id: Uuid,
    ip: &str,
    address_type: &str,
) -> sqlx::Result<Uuid> {
    // The canonical address table permits one current owner per IP. Serialize
    // cross-agent moves on the textual IP so two concurrent snapshots cannot
    // both pass the pre-insert checks and race the partial unique index.
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(ip)
        .execute(&mut **tx)
        .await?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "select id from addresses where interface_id = $1 and ip = $2::inet and is_current for update",
    )
    .bind(interface_id)
    .bind(ip)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((id,)) = existing {
        sqlx::query(
            "update addresses set address_type = $2, source_type = 'agent', last_seen = now(), \
             updated_at = now(), version = version + 1 where id = $1",
        )
        .bind(id)
        .bind(address_type)
        .execute(&mut **tx)
        .await?;
        return Ok(id);
    }

    sqlx::query(
        "update addresses set is_current = false, last_seen = now(), updated_at = now(), \
         version = version + 1 where interface_id = $1 and is_current",
    )
    .bind(interface_id)
    .execute(&mut **tx)
    .await?;
    // M1 enforces one current owner per IP. Close a previous owner only after
    // taking its row out of the current set; the old address remains history.
    sqlx::query(
        "update addresses set is_current = false, last_seen = now(), updated_at = now(), \
         version = version + 1 where ip = $1::inet and is_current",
    )
    .bind(ip)
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar(
        "insert into addresses (interface_id, ip, address_type, source_type, is_current) \
         values ($1, $2::inet, $3, 'agent', true) returning id",
    )
    .bind(interface_id)
    .bind(ip)
    .bind(address_type)
    .fetch_one(&mut **tx)
    .await
}

async fn upsert_containment(
    tx: &mut Transaction<'_, Postgres>,
    parent_kind: &str,
    parent_id: Uuid,
    child_kind: &str,
    child_id: Uuid,
    relation: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "insert into containment_edges (parent_kind, parent_id, child_kind, child_id, relation) \
         values ($1, $2, $3, $4, $5) on conflict \
         (parent_kind, parent_id, child_kind, child_id, relation) \
         do update set updated_at = now(), version = containment_edges.version + 1",
    )
    .bind(parent_kind)
    .bind(parent_id)
    .bind(child_kind)
    .bind(child_id)
    .bind(relation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn project_service_endpoint(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
    device_id: Uuid,
    workload_id: Option<Uuid>,
    owner_source_key: &str,
    value: &Value,
    endpoint_type: &str,
    index: usize,
    source_instance: &str,
) -> sqlx::Result<()> {
    let protocol = object_string(value, &["protocol", "transport"])
        .unwrap_or("tcp")
        .to_ascii_lowercase();
    let port = object_i64(value, &["port", "host_port", "published_port"])
        .and_then(|port| u16::try_from(port).ok());
    let address =
        object_string(value, &["address", "ip", "host_ip", "host"]).and_then(normalize_ip);
    let name = object_string(value, &["service", "name", "process", "program", "unit"]);
    let product = object_string(value, &["product"]);
    let product_version = object_string(value, &["version", "product_version"]);
    let owner_kind = workload_id.map(|_| "workload").unwrap_or("device");
    let owner_id = workload_id.unwrap_or(device_id);
    let source_key = format!(
        "agent:{agent_id}:{endpoint_type}:{owner_source_key}:{}:{}:{}:{index}",
        protocol,
        address.as_deref().unwrap_or("*"),
        port.unwrap_or_default()
    );

    let mut service_id: Option<Uuid> = None;
    // An agent-reported published port can corroborate a network-discovered
    // endpoint. Reuse that service before creating an agent-owned duplicate.
    if endpoint_type == "published_port"
        && let Some(port) = port
    {
        let wildcard_address = address.as_deref().is_none_or(is_unspecified_ip);
        service_id = if wildcard_address {
            sqlx::query_scalar(
                "select s.id from services s join endpoints e on e.service_id = s.id \
                 where e.port = $1 and e.is_current \
                   and lower(coalesce(s.protocol, '')) = $2 \
                   and ((s.owner_kind = 'device' and s.owner_id = $3) \
                        or (s.owner_kind = 'workload' and exists ( \
                            select 1 from workloads w where w.id = s.owner_id \
                              and w.host_device_id = $3))) \
                   and exists (select 1 from addresses a \
                                join interfaces i on i.id = a.interface_id \
                               where i.device_id = $3 and a.is_current and a.ip = e.address) \
                 limit 1",
            )
            .bind(i32::from(port))
            .bind(&protocol)
            .bind(device_id)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            let Some(address) = address.as_deref() else {
                unreachable!("non-wildcard endpoint address must be present")
            };
            sqlx::query_scalar(
                "select s.id from services s join endpoints e on e.service_id = s.id \
                 where e.address = $1::inet and e.port = $2 and e.is_current \
                   and lower(coalesce(s.protocol, '')) = $3 \
                   and ((s.owner_kind = 'device' and s.owner_id = $4) \
                        or (s.owner_kind = 'workload' and exists ( \
                            select 1 from workloads w where w.id = s.owner_id \
                              and w.host_device_id = $4))) \
                 limit 1",
            )
            .bind(address)
            .bind(i32::from(port))
            .bind(&protocol)
            .bind(device_id)
            .fetch_optional(&mut **tx)
            .await?
        };
    }

    let service_id = if let Some(service_id) = service_id {
        service_id
    } else {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "select id from services where owner_kind = $1 and owner_id = $2 \
             and agent_source_key = $3",
        )
        .bind(owner_kind)
        .bind(owner_id)
        .bind(&source_key)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(service_id) = existing {
            sqlx::query(
                "update services set name = coalesce($2, name), protocol = coalesce($3, protocol), \
                 product = coalesce($4, product), product_version = coalesce($5, product_version), \
                 source_type = 'agent', updated_at = now(), version = version + 1 where id = $1",
            )
            .bind(service_id)
            .bind(name)
            .bind(&protocol)
            .bind(product)
            .bind(product_version)
            .execute(&mut **tx)
            .await?;
            service_id
        } else {
            sqlx::query_scalar(
                "insert into services (name, protocol, product, product_version, owner_kind, owner_id, \
                 source_type, agent_source_key) values ($1, $2, $3, $4, $5, $6, 'agent', $7) returning id",
            )
            .bind(name)
            .bind(&protocol)
            .bind(product)
            .bind(product_version)
            .bind(owner_kind)
            .bind(owner_id)
            .bind(&source_key)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    let endpoint_source_key = format!("{source_key}:endpoint");
    let reachability = reachability_for(value, endpoint_type);
    let endpoint_id: Option<Uuid> = sqlx::query_scalar(
        "select id from endpoints where service_id = $1 and agent_source_key = $2",
    )
    .bind(service_id)
    .bind(&endpoint_source_key)
    .fetch_optional(&mut **tx)
    .await?;
    let endpoint_id = if let Some(endpoint_id) = endpoint_id {
        sqlx::query(
            "update endpoints set address = $2::inet, port = $3, endpoint_type = $4, \
            source_type = 'agent', reachability_state = $5, reachability_checked_at = $6::timestamptz, \
             reachability_worker_id = $7, last_seen = now(), updated_at = now(), version = version + 1 \
             where id = $1",
        )
        .bind(endpoint_id)
        .bind(address.as_deref())
        .bind(port.map(i32::from))
        .bind(endpoint_type)
        .bind(&reachability.0)
        .bind(&reachability.1)
        .bind(&reachability.2)
        .execute(&mut **tx)
        .await?;
        endpoint_id
    } else {
        sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port, source_type, \
             agent_source_key, reachability_state, reachability_checked_at, reachability_worker_id) \
             values ($1, $2, $3::inet, $4, 'agent', $5, $6, $7::timestamptz, $8) returning id",
        )
        .bind(service_id)
        .bind(endpoint_type)
        .bind(address.as_deref())
        .bind(port.map(i32::from))
        .bind(&endpoint_source_key)
        .bind(&reachability.0)
        .bind(&reachability.1)
        .bind(&reachability.2)
        .fetch_one(&mut **tx)
        .await?
    };

    if let Some(workload_id) = workload_id {
        upsert_containment(
            tx,
            "workloads",
            workload_id,
            "services",
            service_id,
            "runs_service",
        )
        .await?;
    } else {
        upsert_containment(
            tx,
            "devices",
            device_id,
            "services",
            service_id,
            "runs_service",
        )
        .await?;
    }
    let evidence_value = json!({
        "listening": endpoint_type == "socket",
        "reachability": {
            "state": reachability.0,
            "checked_at": reachability.1,
            "worker_id": reachability.2,
            "endpoint": {
                "address": address,
                "port": port,
                "protocol": protocol,
            },
        },
    });
    evidence::record_automatic_tx(
        tx,
        "endpoints",
        endpoint_id,
        "agent",
        Some(source_instance),
        "agent_reachability",
        &evidence_value,
        0.8,
        false,
    )
    .await?;
    Ok(())
}

fn reachability_for(
    value: &Value,
    endpoint_type: &str,
) -> (String, Option<String>, Option<String>) {
    let reachability = value.get("reachability");
    let state = reachability
        .and_then(|item| item.get("state").or(Some(item)))
        .and_then(Value::as_str)
        .map(normalize_reachability_state)
        .unwrap_or_else(|| {
            if endpoint_type == "socket" {
                "internal".to_string()
            } else {
                "unknown".to_string()
            }
        });
    let checked_at = reachability
        .and_then(|item| item.get("checked_at"))
        .and_then(Value::as_str)
        .and_then(|value| {
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
        .map(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });
    let worker_id = reachability
        .and_then(|item| item.get("worker_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if value.get("reachable").and_then(Value::as_bool) == Some(true) {
        return ("reachable".to_string(), checked_at, worker_id);
    }
    if value.get("reachable").and_then(Value::as_bool) == Some(false) {
        return ("unreachable".to_string(), checked_at, worker_id);
    }
    (state, checked_at, worker_id)
}

fn normalize_reachability_state(value: &str) -> String {
    match value {
        "internal" | "internal_only" | "listening" => "internal",
        "reachable" | "external" | "external_reachable" => "reachable",
        "unreachable" | "blocked" => "unreachable",
        _ => "unknown",
    }
    .to_string()
}

fn first_array<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a Vec<Value>> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for component in path.split('.') {
            current = current.get(component)?;
        }
        current.as_array()
    })
}

fn inventory_string<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
}

fn object_string<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    value.as_object().and_then(|object| {
        names
            .iter()
            .find_map(|name| object.get(*name).and_then(Value::as_str))
    })
}

fn object_i64(value: &Value, names: &[&str]) -> Option<i64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_i64))
}

fn address_value(value: &Value) -> Option<String> {
    let candidate = value
        .as_str()
        .or_else(|| object_string(value, &["ip", "address"]));
    candidate.and_then(normalize_ip)
}

fn normalize_ip(value: &str) -> Option<String> {
    IpAddr::from_str(value).ok().map(|value| value.to_string())
}

fn is_unspecified_ip(value: &str) -> bool {
    IpAddr::from_str(value)
        .map(|address| address.is_unspecified())
        .unwrap_or(false)
}

fn normalize_mac(value: &str) -> Option<String> {
    let compact = value.replace('-', ":").to_ascii_lowercase();
    let parts: Vec<&str> = compact.split(':').collect();
    if parts.len() != 6
        || parts
            .iter()
            .any(|part| part.len() != 2 || u8::from_str_radix(part, 16).is_err())
    {
        return None;
    }
    Some(compact)
}

// ---- Authenticated read API -------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

fn api_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

pub async fn list_agents(
    State(state): State<AppState>,
    Query(query): Query<AgentListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = query.limit.map_or(100, |value| value.clamp(1, 100));
    let cursor = decode_cursor(&query.cursor);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select a.id, a.hostname, a.agent_version, a.os, a.arch, a.capabilities, \
                   a.last_seen, a.last_heartbeat_at, a.protocol_version, a.revoked_at, \
                   (select c.inventory from agent_inventory_current c where c.agent_id = a.id) as inventory, \
                   case when a.revoked_at is not null then 'revoked' \
                        when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                             now() - make_interval(secs => a.heartbeat_timeout_seconds::double precision) \
                             then 'offline' \
                        when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                             now() - make_interval(secs => (a.heartbeat_timeout_seconds / 2)::double precision) \
                             then 'stale' else 'online' end as status, \
                   (select ir.device_id from identity_rules ir where ir.rule_type = 'agent_id' \
                     and ir.value = a.id::text order by ir.created_at limit 1) as device_id, \
                   a.created_at \
              from agents a \
             where ($1::timestamptz is null or (a.created_at, a.id) > ($1::timestamptz, $2)) \
             order by a.created_at, a.id limit $3 \
        ) t",
    )
    .bind(cursor.as_ref().map(|(created_at, _)| created_at.clone()))
    .bind(cursor.as_ref().map(|(_, id)| *id))
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    let Ok(rows) = rows else {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            rows.unwrap_err().to_string(),
        );
    };
    let mut items: Vec<Value> = rows.into_iter().map(|(value,)| value).collect();
    for item in &mut items {
        if let Value::Object(map) = item {
            let inventory = map.remove("inventory").unwrap_or_else(|| json!({}));
            map.insert(
                "inventory_summary".to_string(),
                inventory_summary(&inventory),
            );
        }
    }
    let next_cursor = items.last().and_then(|item| {
        let created_at = item.get("created_at")?.as_str()?;
        let id = Uuid::parse_str(item.get("id")?.as_str()?).ok()?;
        Some(encode_cursor(created_at, id))
    });
    (
        StatusCode::OK,
        Json(json!({ "items": items, "next_cursor": next_cursor })),
    )
}

pub async fn get_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match build_agent_detail(&state.pool, agent_id).await {
        Ok(Some(detail)) => (StatusCode::OK, Json(detail)),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn get_agent_inventory(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let result: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select c.agent_id, c.snapshot_id, c.message_id, c.protocol_version, c.sequence, \
                   c.collected_at, c.received_at, c.complete, c.inventory \
              from agent_inventory_current c where c.agent_id = $1 \
        ) t",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await;
    match result {
        Ok(Some((inventory,))) => (StatusCode::OK, Json(inventory)),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "agent inventory not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn get_agent_inventory_history(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<ListParams>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(query.limit).min(INVENTORY_HISTORY_LIMIT);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (select id, agent_id, message_id, protocol_version, sequence, \
                collected_at, received_at, complete from agent_inventory_snapshots \
                where agent_id = $1 order by sequence desc limit $2) t",
    )
    .bind(agent_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "items": rows.into_iter().map(|(value,)| value).collect::<Vec<_>>() })),
        ),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn get_agent_health(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let row: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select a.id, a.last_seen, a.last_heartbeat_at, a.heartbeat_timeout_seconds, \
                   a.revoked_at, case when a.revoked_at is not null then 'revoked' \
                     when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                       now() - make_interval(secs => a.heartbeat_timeout_seconds::double precision) \
                     then 'offline' \
                     when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                       now() - make_interval(secs => (a.heartbeat_timeout_seconds / 2)::double precision) \
                     then 'stale' else 'online' end as status, \
                   (select row_to_json(i) from (select * from agent_health_incidents \
                     where agent_id = a.id order by opened_at desc limit 1) i) as incident \
              from agents a where a.id = $1 \
        ) t",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((value,))) => (StatusCode::OK, Json(value)),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn get_agent_version(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let row: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (select id, agent_version, os, arch, protocol_version, \
                capabilities, last_seen, last_heartbeat_at, revoked_at from agents where id = $1) t",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((value,))) => (StatusCode::OK, Json(value)),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn build_agent_detail(pool: &PgPool, agent_id: Uuid) -> sqlx::Result<Option<Value>> {
    let agent: Option<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select a.id, a.hostname, a.agent_version, a.os, a.arch, a.capabilities, \
                   a.last_seen, a.protocol_version, a.revoked_at, \
                   case when a.revoked_at is not null then 'revoked' \
                     when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                       now() - make_interval(secs => a.heartbeat_timeout_seconds::double precision) \
                     then 'offline' \
                     when coalesce(a.last_heartbeat_at, a.last_seen, a.created_at) < \
                       now() - make_interval(secs => (a.heartbeat_timeout_seconds / 2)::double precision) \
                     then 'stale' else 'online' end as status, \
                   (select ir.device_id from identity_rules ir where ir.rule_type = 'agent_id' \
                     and ir.value = a.id::text order by ir.created_at limit 1) as device_id, \
                   a.created_at, a.last_heartbeat_at \
              from agents a where a.id = $1 \
        ) t",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let Some((mut detail,)) = agent else {
        return Ok(None);
    };
    let device_id: Option<Uuid> = sqlx::query_scalar(
        "select device_id from identity_rules where rule_type = 'agent_id' and value = $1 order by created_at limit 1",
    )
    .bind(agent_id.to_string())
    .fetch_optional(pool)
    .await?;

    let inventory: Option<(Value,)> =
        sqlx::query_as("select inventory from agent_inventory_current where agent_id = $1")
            .bind(agent_id)
            .fetch_optional(pool)
            .await?;
    let inventory_value = inventory.map(|(value,)| value).unwrap_or_else(|| json!({}));
    let host = normalized_host(&inventory_value);
    let network = normalized_network(&inventory_value);
    let filesystems = normalized_filesystems(&inventory_value);
    let processes = normalized_processes(&inventory_value);
    let containers = normalized_containers(&inventory_value);
    let sockets = decorated_sockets(&inventory_value);
    let summary = inventory_summary(&inventory_value);
    let evidence: Vec<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from (select * from evidence where subject_table = 'devices' \
            and subject_id = $1 order by last_seen desc limit 500) t",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?;
    let latest_snapshot: Option<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from (select snapshot_id, message_id, protocol_version, sequence, \
            collected_at, received_at, complete from agent_inventory_current where agent_id = $1) t",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let identity_rules: Vec<Value> = sqlx::query_as::<_, (String, String, bool)>(
        "select rule_type, value, pinned from identity_rules where device_id = $1 order by rule_type, value",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(rule_type, value, pinned)| {
        json!({"rule_type": rule_type, "value": value, "pinned": pinned})
    })
    .collect();
    let matched_identifiers = identity_rules
        .iter()
        .filter_map(|rule| {
            Some(format!(
                "{}:{}",
                rule.get("rule_type")?.as_str()?,
                rule.get("value")?.as_str()?
            ))
        })
        .collect::<Vec<_>>();
    let reconciliation_time = latest_snapshot
        .as_ref()
        .and_then(|(value,)| value.get("received_at").cloned())
        .or_else(|| {
            latest_snapshot
                .as_ref()
                .and_then(|(value,)| value.get("collected_at").cloned())
        })
        .unwrap_or(Value::Null);
    let reconciliation = json!({
        "status": if device_id.is_some() { "matched" } else { "unmatched" },
        "device_id": device_id,
        "confidence": if device_id.is_some() { Some(1.0) } else { None },
        "matched_identifiers": matched_identifiers,
        "conflicts": [],
        "last_reconciled_at": reconciliation_time,
        "explanation": if device_id.is_some() {
            "Agent identity is linked to this device."
        } else {
            "Agent identity has not been linked to a device."
        },
        "agent_id": agent_id,
        "identity_rules": identity_rules,
    });

    if let Value::Object(map) = &mut detail {
        map.insert("device_id".to_string(), json!(device_id));
        map.insert("host".to_string(), host);
        map.insert("network".to_string(), network);
        map.insert("filesystems".to_string(), filesystems);
        map.insert("processes".to_string(), processes);
        map.insert("sockets".to_string(), sockets);
        map.insert("containers".to_string(), containers);
        map.insert(
            "collector_status".to_string(),
            inventory_value
                .get("collector_status")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        map.insert("inventory_summary".to_string(), summary);
        map.insert(
            "evidence".to_string(),
            Value::Array(evidence.into_iter().map(|(value,)| value).collect()),
        );
        map.insert("reconciliation".to_string(), reconciliation);
        map.insert(
            "inventory_snapshot".to_string(),
            latest_snapshot.map(|(value,)| value).unwrap_or(Value::Null),
        );
    }
    Ok(Some(detail))
}

fn decorated_sockets(inventory: &Value) -> Value {
    let sockets = first_array(
        inventory,
        &[
            "sockets.sockets",
            "sockets",
            "listening_sockets",
            "listening.services",
        ],
    )
    .cloned()
    .unwrap_or_default();
    Value::Array(
        sockets
            .into_iter()
            .map(|socket| {
                let mut object = socket.as_object().cloned().unwrap_or_default();
                let listening = object.get("listening").and_then(Value::as_bool).unwrap_or(true);
                object.insert("listening".to_string(), json!(listening));
                let endpoint = object
                    .get("reachability")
                    .and_then(|value| value.get("endpoint"))
                    .cloned()
                    .unwrap_or_else(|| {
                        json!({
                            "address": object.get("address").or_else(|| object.get("ip")).cloned(),
                            "port": object.get("port").cloned(),
                            "protocol": object.get("protocol").or_else(|| object.get("transport")).cloned(),
                        })
                    });
                let mut reachability = object
                    .get("reachability")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                reachability
                    .entry("state".to_string())
                    .or_insert_with(|| json!("internal"));
                reachability
                    .entry("checked_at".to_string())
                    .or_insert(Value::Null);
                reachability
                    .entry("worker_id".to_string())
                    .or_insert(Value::Null);
                reachability
                    .entry("endpoint".to_string())
                    .or_insert(endpoint);
                object.insert("reachability".to_string(), Value::Object(reachability));
                Value::Object(object)
            })
            .collect(),
    )
}

fn inventory_summary(inventory: &Value) -> Value {
    let interfaces = first_array(inventory, &["interfaces", "network.interfaces"])
        .map(Vec::len)
        .unwrap_or_default();
    let filesystems = first_array(inventory, &["filesystem.filesystems", "filesystems"])
        .map(Vec::len)
        .unwrap_or_default();
    let processes = first_array(inventory, &["processes.processes", "processes"])
        .map(Vec::len)
        .unwrap_or_default();
    let sockets = first_array(
        inventory,
        &[
            "sockets.sockets",
            "sockets",
            "listening_sockets",
            "listening.services",
        ],
    )
    .map(Vec::len)
    .unwrap_or_default();
    let containers = first_array(inventory, &["docker.containers", "containers"])
        .map(Vec::len)
        .unwrap_or_default();
    json!({
        "interfaces": interfaces,
        "filesystems": filesystems,
        "processes": processes,
        "sockets": sockets,
        "containers": containers,
    })
}

/// Normalize the collector wire shape into the stable detail API shape. Agent
/// collectors are intentionally nested by capability; the UI should not need
/// to know that transport detail or each collector's field aliases.
fn normalized_host(inventory: &Value) -> Value {
    let raw = inventory.get("host").unwrap_or(inventory);
    let cpu = raw.get("cpu").unwrap_or(&Value::Null);
    let load = raw.get("load").unwrap_or(&Value::Null);
    let memory = raw.get("memory").unwrap_or(&Value::Null);
    let total = value_u64(memory, &["MemTotal", "total_bytes"]);
    let available = value_u64(memory, &["MemAvailable", "available_bytes"]);
    let used = total
        .zip(available)
        .map(|(total, available)| total.saturating_sub(available));
    json!({
        "hostname": value_string(raw, &["hostname", "name"]),
        "os": value_string(raw, &["os", "operating_system"]),
        "distribution": value_string(raw, &["distribution", "distribution_name"]),
        "kernel": value_string(raw, &["kernel", "kernel_release"]),
        "arch": value_string(raw, &["arch", "architecture"]),
        "boot_id": value_string(raw, &["boot_id", "boot_id_sha256"]),
        "uptime_seconds": value_u64(raw, &["uptime_seconds", "uptime_secs"]),
        "cpu_model": value_string(cpu, &["model", "name"]),
        "cpu_count": value_u64(cpu, &["logical_cpus", "cpu_count"]),
        "load_1m": value_f64(load, &["one", "load_1m"]),
        "memory_total_bytes": total,
        "memory_used_bytes": used,
        "machine_id_hash": value_string(raw, &["machine_id_hash", "machine_id_sha256"]),
    })
}

fn normalized_network(inventory: &Value) -> Value {
    let raw = inventory.get("network").unwrap_or(inventory);
    let interfaces = first_array(raw, &["interfaces"])
        .or_else(|| first_array(inventory, &["interfaces"]))
        .map(|items| {
            items
                .iter()
                .take(MAX_JSON_NODES)
                .map(|item| {
                    json!({
                        "name": value_string(item, &["name", "interface", "ifname"]).unwrap_or_else(|| "unknown".to_string()),
                        "mac": value_string(item, &["mac", "mac_address", "macAddress"]),
                        "addresses": item.get("addresses")
                            .and_then(Value::as_array)
                            .map(|addresses| addresses.iter().filter_map(address_value).collect::<Vec<_>>())
                            .unwrap_or_default(),
                        "state": value_string(item, &["state", "operstate"]),
                        "mtu": value_u64(item, &["mtu"]),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let routes = first_array(raw, &["routes"])
        .or_else(|| first_array(inventory, &["routes"]))
        .map(|items| {
            items
                .iter()
                .take(MAX_JSON_NODES)
                .map(|item| {
                    json!({
                        "destination": value_string(item, &["destination", "dst"]).unwrap_or_else(|| "unknown".to_string()),
                        "gateway": value_string(item, &["gateway", "gw"]),
                        "interface_name": value_string(item, &["interface_name", "interface", "dev"]),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({"interfaces": interfaces, "routes": routes})
}

fn normalized_filesystems(inventory: &Value) -> Value {
    let items = first_array(inventory, &["filesystem.filesystems", "filesystems"])
        .cloned()
        .unwrap_or_default();
    Value::Array(
        items
            .into_iter()
            .take(MAX_JSON_NODES)
            .map(|item| {
                json!({
                    "mount_point": value_string(&item, &["mount_point", "mount", "path"]).unwrap_or_else(|| "unknown".to_string()),
                    "device": value_string(&item, &["device", "source"]),
                    "filesystem": value_string(&item, &["filesystem", "fstype"]),
                    "total_bytes": value_u64(&item, &["total_bytes", "capacity_bytes"]),
                    "used_bytes": value_u64(&item, &["used_bytes"]),
                    "available_bytes": value_u64(&item, &["available_bytes", "free_bytes"]),
                    "inode_total": value_u64(&item, &["inode_total", "inodes_total"]),
                    "inode_used": value_u64(&item, &["inode_used", "inodes_used"]),
                    "read_only": item.get("read_only").and_then(Value::as_bool),
                })
            })
            .collect(),
    )
}

fn normalized_processes(inventory: &Value) -> Value {
    let items = first_array(inventory, &["processes.processes", "processes"])
        .cloned()
        .unwrap_or_default();
    Value::Array(
        items
            .into_iter()
            .take(MAX_JSON_NODES)
            .map(|item| {
                json!({
                    "pid": value_u64(&item, &["pid"]).unwrap_or_default(),
                    "name": value_string(&item, &["name", "process"]).unwrap_or_else(|| "unknown".to_string()),
                    "command": value_string(&item, &["command", "cmdline"]),
                    "user": value_string(&item, &["user", "username"]),
                    "state": value_string(&item, &["state"]),
                    "cpu_percent": value_f64(&item, &["cpu_percent", "cpu"]),
                    "memory_bytes": value_u64(&item, &["memory_bytes", "rss_bytes"]),
                    "started_at": value_string(&item, &["started_at"]),
                })
            })
            .collect(),
    )
}

fn normalized_containers(inventory: &Value) -> Value {
    Value::Array(
        first_array(inventory, &["docker.containers", "containers"])
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(MAX_JSON_NODES)
            .map(|item| {
                json!({
                    "id": value_string(&item, &["id", "container_id", "containerId"]).unwrap_or_else(|| "unknown".to_string()),
                    "name": value_string(&item, &["name", "container_name", "Names"]).unwrap_or_else(|| "unknown".to_string()),
                    "image": value_string(&item, &["image", "Image"]),
                    "status": value_string(&item, &["status", "state", "Status"]),
                    "health": value_string(&item, &["health"]),
                    "networks": string_array(&item, &["networks", "Networks"]),
                    "mounts": string_array(&item, &["mounts"]),
                    "published_ports": string_array(&item, &["published_ports", "ports"]),
                })
            })
            .collect(),
    )
}

fn value_string(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str).map(str::to_string))
}

fn value_u64(value: &Value, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_i64().and_then(|value| u64::try_from(value).ok()))
        })
    })
}

fn value_f64(value: &Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_f64))
}

fn string_array(value: &Value, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .find_map(|name| {
            value.get(*name).and_then(|item| {
                item.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .take(MAX_JSON_NODES)
                        .collect()
                })
            })
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents;
    use sqlx::PgPool;

    fn snapshot(agent_id: Uuid, sequence: u64) -> InventorySnapshot {
        InventorySnapshot {
            message_type: "inventory_snapshot".to_string(),
            message_id: Uuid::new_v4(),
            protocol_version: protocol::PROTOCOL_VERSION,
            agent_id,
            sequence,
            collected_at_unix_secs: OffsetDateTime::now_utc().unix_timestamp(),
            inventory: json!({
                "hostname": "agent-host",
                "machine_id": format!("machine-{agent_id}"),
                "hardware_uuid": format!("hardware-{agent_id}"),
                "interfaces": [{
                    "name": "eth0",
                    "mac": "aa:bb:cc:dd:ee:ff",
                    "addresses": [{"ip": "192.0.2.10", "type": "static"}]
                }],
                "containers": [{
                    "id": format!("container-{agent_id}"),
                    "name": "web",
                    "image": "nginx:latest",
                    "published_ports": [{"host_ip": "192.0.2.10", "host_port": 8080, "protocol": "tcp"}]
                }],
                "sockets": [{"address": "127.0.0.1", "port": 22, "protocol": "tcp"}]
            }),
            complete: true,
            snapshot_id: Uuid::new_v4(),
            schema_version: protocol::M5_SCHEMA_VERSION,
        }
    }

    fn observation_batch(agent_id: Uuid, batch_id: Uuid) -> protocol::ObservationBatch {
        protocol::ObservationBatch {
            schema_version: protocol::M5_SCHEMA_VERSION,
            batch_id,
            agent_id,
            collected_at_unix_secs: OffsetDateTime::now_utc().unix_timestamp(),
            observations: vec![protocol::Observation {
                idempotency_key: format!("{batch_id}/collector/host"),
                key: "collector.host".to_string(),
                source: "agent".to_string(),
                observed_at_unix_secs: OffsetDateTime::now_utc().unix_timestamp(),
                state: protocol::ObservationState::Ok,
                value: json!({"status": "available"}),
            }],
        }
    }

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    async fn insert_test_agent(pool: &PgPool, agent_id: Uuid, revoked: bool) {
        sqlx::query(
            "insert into agents (id, cert_fingerprint, cert_serial, hostname, revoked_at) \
             values ($1, $2, $3, $4, case when $5 then now() else null end)",
        )
        .bind(agent_id)
        .bind(format!("m5-{agent_id}"))
        .bind(format!("serial-{agent_id}"))
        .bind("agent-host")
        .bind(revoked)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn rejects_oversized_and_deep_payloads() {
        let agent_id = Uuid::new_v4();
        let mut large = snapshot(agent_id, 1);
        large.inventory = json!({"blob": "x".repeat(MAX_STRING_BYTES + 1)});
        let bytes = serde_json::to_vec(&large).unwrap();
        assert!(matches!(
            parse_snapshot_value(serde_json::from_slice(&bytes).unwrap()),
            Err(AgentInventoryError::InvalidPayload(_))
        ));

        let agent_id = Uuid::new_v4();
        let observation = json!({
            "type": "observation_batch",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "schema_version": protocol::M5_SCHEMA_VERSION,
            "batch_id": Uuid::new_v4(),
            "agent_id": agent_id,
            "collected_at_unix_secs": 0,
            "observations": [{
                "idempotency_key": "oversized",
                "key": "collector.host",
                "source": "agent",
                "observed_at_unix_secs": 0,
                "state": "ok",
                "value": "x".repeat(protocol::MAX_OBSERVATION_BATCH_BYTES)
            }]
        });
        assert!(matches!(
            parse_observation_batch_value(observation),
            Err(AgentInventoryError::ObservationPayloadTooLarge)
                | Err(AgentInventoryError::InvalidPayload(_))
        ));

        let mut value = json!(null);
        for _ in 0..=MAX_JSON_DEPTH {
            value = json!([value]);
        }
        let mut deep = snapshot(agent_id, 2);
        deep.inventory = json!({"deep": value});
        let bytes = serde_json::to_vec(&deep).unwrap();
        assert!(matches!(
            parse_snapshot_value(serde_json::from_slice(&bytes).unwrap()),
            Err(AgentInventoryError::InvalidPayload(_))
        ));
    }

    #[test]
    fn parses_valid_observation_batch_and_rejects_malformed_envelopes() {
        let agent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let batch = observation_batch(agent_id, batch_id);
        let envelope = json!({
            "type": "observation_batch",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "schema_version": batch.schema_version,
            "batch_id": batch.batch_id,
            "agent_id": batch.agent_id,
            "collected_at_unix_secs": batch.collected_at_unix_secs,
            "observations": batch.observations,
        });
        let parsed = parse_observation_batch_value(envelope).expect("valid observation batch");
        assert_eq!(parsed.batch.batch_id, batch_id);
        assert_eq!(parsed.batch.agent_id, agent_id);

        let malformed = json!({
            "type": "observation_batch",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "schema_version": protocol::M5_SCHEMA_VERSION,
            "batch_id": batch_id,
            "agent_id": agent_id,
            "collected_at_unix_secs": 0,
            "observations": [{"idempotency_key": "missing-fields"}]
        });
        assert!(matches!(
            parse_observation_batch_value(malformed),
            Err(AgentInventoryError::Json(_))
        ));
    }

    #[tokio::test]
    async fn observation_identity_mismatch_is_rejected_before_database_access() {
        let pool = PgPool::connect_lazy("postgres://invalid/hope").expect("lazy pool");
        let authenticated_agent_id = Uuid::new_v4();
        let batch = observation_batch(Uuid::new_v4(), Uuid::new_v4());
        let result = ingest_observation_batch(
            &pool,
            authenticated_agent_id,
            protocol::PROTOCOL_VERSION,
            &batch,
        )
        .await;
        assert!(matches!(
            result,
            Err(AgentInventoryError::AgentIdentityMismatch)
        ));
    }

    #[tokio::test]
    async fn idempotent_replay_and_identity_reconciliation() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let agent_id = Uuid::new_v4();
        insert_test_agent(&pool, agent_id, false).await;
        let report = snapshot(agent_id, 1);
        let first = ingest_snapshot(&pool, agent_id, &report).await.unwrap();
        let mut retransmission = report.clone();
        retransmission.message_id = Uuid::new_v4();
        let second = ingest_snapshot(&pool, agent_id, &retransmission)
            .await
            .unwrap();
        assert!(!first.replayed);
        assert!(second.replayed);
        assert_eq!(first.snapshot_id, second.snapshot_id);
        let snapshot_count: i64 = sqlx::query_scalar(
            "select count(*) from agent_inventory_snapshots where agent_id = $1",
        )
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(snapshot_count, 1);
        let device_count: i64 = sqlx::query_scalar(
            "select count(*) from identity_rules where rule_type = 'agent_id' and value = $1",
        )
        .bind(agent_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(device_count, 1);
    }

    #[tokio::test]
    async fn observation_ingest_persists_metadata_and_deduplicates_retry() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let agent_id = Uuid::new_v4();
        insert_test_agent(&pool, agent_id, false).await;
        let batch = observation_batch(agent_id, Uuid::new_v4());

        let first = ingest_observation_batch(&pool, agent_id, protocol::PROTOCOL_VERSION, &batch)
            .await
            .expect("persist observation batch");
        let second = ingest_observation_batch(&pool, agent_id, protocol::PROTOCOL_VERSION, &batch)
            .await
            .expect("deduplicate observation retry");

        assert_eq!(first.accepted_count, 1);
        assert!(!first.replayed);
        assert_eq!(second.accepted_count, 0);
        assert!(second.replayed);

        let row: (Uuid, String, String, String, Value) = sqlx::query_as(
            "select agent_id, observation_key, source, state, value \
             from agent_observations where agent_id = $1",
        )
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .expect("read persisted observation");
        assert_eq!(row.0, agent_id);
        assert_eq!(row.1, "collector.host");
        assert_eq!(row.2, "agent");
        assert_eq!(row.3, "ok");
        assert_eq!(row.4, json!({"status": "available"}));

        let count: i64 =
            sqlx::query_scalar("select count(*) from agent_observations where agent_id = $1")
                .bind(agent_id)
                .fetch_one(&pool)
                .await
                .expect("count persisted observations");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn wildcard_published_port_reuses_existing_service() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let agent_id = Uuid::new_v4();
        insert_test_agent(&pool, agent_id, false).await;

        let first = snapshot(agent_id, 1);
        let first_outcome = ingest_snapshot(&pool, agent_id, &first).await.unwrap();
        let device_id = first_outcome.device_id.unwrap();

        let mut followup = snapshot(agent_id, 2);
        followup.inventory["containers"][0]["published_ports"][0]["host_ip"] = json!("0.0.0.0");
        ingest_snapshot(&pool, agent_id, &followup).await.unwrap();

        let service_count: i64 = sqlx::query_scalar(
            "select count(*) from services s join workloads w on w.id = s.owner_id \
             where s.owner_kind = 'workload' and w.host_device_id = $1",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(service_count, 1);
    }

    #[tokio::test]
    async fn unknown_and_revoked_agents_are_rejected() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let unknown_id = Uuid::new_v4();
        let unknown = ingest_snapshot(&pool, unknown_id, &snapshot(unknown_id, 1)).await;
        assert!(matches!(unknown, Err(AgentInventoryError::UnknownAgent(_))));

        let revoked_id = Uuid::new_v4();
        insert_test_agent(&pool, revoked_id, true).await;
        let revoked = ingest_snapshot(&pool, revoked_id, &snapshot(revoked_id, 1)).await;
        assert!(matches!(revoked, Err(AgentInventoryError::RevokedAgent(_))));
    }

    #[tokio::test]
    async fn offline_incident_recovers_without_deleting_inventory() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let agent_id = Uuid::new_v4();
        insert_test_agent(&pool, agent_id, false).await;
        let report = snapshot(agent_id, 1);
        ingest_snapshot(&pool, agent_id, &report).await.unwrap();
        sqlx::query(
            "update agents set last_seen = now() - interval '5 minutes', \
             last_heartbeat_at = now() - interval '5 minutes' where id = $1",
        )
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(agents::sweep_offline(&pool, 30).await.unwrap() >= 1);
        let open: String = sqlx::query_scalar(
            "select state from agent_health_incidents where agent_id = $1 and state = 'open'",
        )
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, "open");

        agents::touch_last_seen(&pool, agent_id).await.unwrap();
        let recovered: String = sqlx::query_scalar(
            "select state from agent_health_incidents where agent_id = $1 order by opened_at desc limit 1",
        )
        .bind(agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(recovered, "recovered");
        let current_count: i64 =
            sqlx::query_scalar("select count(*) from agent_inventory_current where agent_id = $1")
                .bind(agent_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(current_count, 1);
    }

    #[test]
    fn normalizes_nested_collector_payloads_and_aliases() {
        let inventory = json!({
            "host": {
                "hostname": "box1",
                "os": "linux",
                "arch": "x86_64",
                "boot_id_sha256": "boot-hash",
                "uptime_secs": 42,
                "cpu": {"model": "CPU", "logical_cpus": 8},
                "load": {"one": 0.25},
                "memory": {"MemTotal": 1000, "MemAvailable": 400}
            },
            "network": {
                "interfaces": [{"name": "eth0", "addresses": [{"address": "192.0.2.1"}]}],
                "routes": [{"interface": "eth0", "destination": "0.0.0.0", "gateway": "192.0.2.254"}]
            },
            "filesystem": {"filesystems": [{"mount_point": "/", "capacity_bytes": 100, "used_bytes": 40}]},
            "processes": {"processes": [{"pid": 7, "name": "worker", "rss_bytes": 64}]},
            "sockets": {"sockets": [{"protocol": "tcp", "local_address": "127.0.0.1", "local_port": 8080}]},
            "docker": {"containers": [{"id": "abc", "name": "web", "status": "running"}]},
            "collector_status": [{"capability": "docker", "status": "partial", "error": "permission denied"}]
        });

        assert_eq!(normalized_host(&inventory)["cpu_count"], 8);
        assert_eq!(normalized_host(&inventory)["memory_used_bytes"], 600);
        assert_eq!(
            normalized_network(&inventory)["routes"][0]["interface_name"],
            "eth0"
        );
        assert_eq!(normalized_filesystems(&inventory)[0]["total_bytes"], 100);
        assert_eq!(normalized_processes(&inventory)[0]["memory_bytes"], 64);
        assert_eq!(decorated_sockets(&inventory)[0]["local_port"], 8080);
        assert_eq!(normalized_containers(&inventory)[0]["id"], "abc");
        assert_eq!(inventory_summary(&inventory)["filesystems"], 1);
        assert_eq!(inventory_summary(&inventory)["processes"], 1);
        assert_eq!(inventory_summary(&inventory)["sockets"], 1);
        assert_eq!(inventory_summary(&inventory)["containers"], 1);
    }

    #[tokio::test]
    async fn agent_detail_uses_current_agent_schema_heartbeat_column() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let agent_id = Uuid::new_v4();
        insert_test_agent(&pool, agent_id, false).await;
        let detail = build_agent_detail(&pool, agent_id)
            .await
            .expect("agent detail query")
            .expect("agent detail");
        assert!(detail.get("last_heartbeat_at").is_some());
        assert!(detail.get("updated_at").is_none());
    }
}
