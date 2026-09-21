//! Authenticated host resource telemetry ingestion and read projection.
//!
//! The wire batch is deliberately small and idempotent. The API keeps raw
//! samples for the configured retention window and performs bounded numeric
//! aggregation at read time so one-hour views stay at native resolution.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::state::AppState;

pub const MAX_METRIC_MESSAGE_BYTES: usize = protocol::MAX_METRIC_BATCH_BYTES;
const FRESHNESS_STALE_AFTER_SECONDS: i64 = 45;
const MAX_SERIES_POINTS: usize = 720;

#[derive(Debug, Error)]
pub enum AgentMetricError {
    #[error("metric payload exceeds {MAX_METRIC_MESSAGE_BYTES} bytes")]
    PayloadTooLarge,
    #[error("metric payload is invalid: {0}")]
    InvalidPayload(String),
    #[error("metric protocol version {0} is unsupported")]
    UnsupportedProtocol(u32),
    #[error("metric batch agent id does not match authenticated certificate")]
    AgentIdentityMismatch,
    #[error("agent {0} is unknown")]
    UnknownAgent(Uuid),
    #[error("agent {0} is revoked")]
    RevokedAgent(Uuid),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParsedMetricSampleBatch {
    #[serde(rename = "type")]
    pub message_type: String,
    pub message_id: Uuid,
    pub protocol_version: u32,
    #[serde(flatten)]
    pub batch: protocol::MetricSampleBatch,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricIngestOutcome {
    pub batch_id: Uuid,
    pub sample_count: usize,
    pub accepted_count: usize,
    pub replayed: bool,
}

pub fn parse_metric_sample_batch_value(
    value: Value,
) -> Result<ParsedMetricSampleBatch, AgentMetricError> {
    let encoded = serde_json::to_vec(&value)?;
    if encoded.len() > MAX_METRIC_MESSAGE_BYTES {
        return Err(AgentMetricError::PayloadTooLarge);
    }
    let parsed: ParsedMetricSampleBatch = serde_json::from_value(value)?;
    validate_metric_sample_batch(&parsed)?;
    Ok(parsed)
}

fn validate_metric_sample_batch(parsed: &ParsedMetricSampleBatch) -> Result<(), AgentMetricError> {
    if parsed.message_type != "metric_sample_batch" {
        return Err(AgentMetricError::InvalidPayload(format!(
            "expected type metric_sample_batch, got {}",
            parsed.message_type
        )));
    }
    if parsed.protocol_version != protocol::PROTOCOL_VERSION {
        return Err(AgentMetricError::UnsupportedProtocol(
            parsed.protocol_version,
        ));
    }
    if parsed.message_id.is_nil() {
        return Err(AgentMetricError::InvalidPayload(
            "message_id must not be nil".to_string(),
        ));
    }
    parsed
        .batch
        .validate()
        .map_err(|error| AgentMetricError::InvalidPayload(error.to_string()))?;
    for sample in &parsed.batch.samples {
        OffsetDateTime::from_unix_timestamp(sample.collected_at_unix_secs).map_err(|error| {
            AgentMetricError::InvalidPayload(format!("invalid collected_at_unix_secs: {error}"))
        })?;
    }
    Ok(())
}

pub async fn ingest_metric_sample_batch(
    pool: &PgPool,
    authenticated_agent_id: Uuid,
    protocol_version: u32,
    parsed: &ParsedMetricSampleBatch,
) -> Result<MetricIngestOutcome, AgentMetricError> {
    validate_metric_sample_batch(parsed)?;
    if protocol_version != parsed.protocol_version {
        return Err(AgentMetricError::UnsupportedProtocol(protocol_version));
    }
    if parsed.batch.agent_id != authenticated_agent_id {
        return Err(AgentMetricError::AgentIdentityMismatch);
    }

    let agent: Option<(Option<OffsetDateTime>,)> =
        sqlx::query_as("select revoked_at from agents where id = $1")
            .bind(authenticated_agent_id)
            .fetch_optional(pool)
            .await?;
    let Some((revoked_at,)) = agent else {
        return Err(AgentMetricError::UnknownAgent(authenticated_agent_id));
    };
    if revoked_at.is_some() {
        return Err(AgentMetricError::RevokedAgent(authenticated_agent_id));
    }

    let mut tx = pool.begin().await?;
    let mut accepted_count = 0usize;
    for sample in &parsed.batch.samples {
        let collected_at = OffsetDateTime::from_unix_timestamp(sample.collected_at_unix_secs)
            .map_err(|error| AgentMetricError::InvalidPayload(error.to_string()))?;
        let result = sqlx::query(
            "insert into agent_metric_samples \
                (agent_id, batch_id, sample_id, schema_version, collected_at, metrics) \
             values ($1, $2, $3, $4, $5, $6) \
             on conflict (agent_id, sample_id) do nothing",
        )
        .bind(authenticated_agent_id)
        .bind(parsed.batch.batch_id)
        .bind(sample.sample_id)
        .bind(i32::try_from(parsed.batch.schema_version).unwrap_or(i32::MAX))
        .bind(collected_at)
        .bind(&sample.metrics)
        .execute(&mut *tx)
        .await?;
        accepted_count += usize::try_from(result.rows_affected()).unwrap_or_default();
    }
    tx.commit().await?;

    Ok(MetricIngestOutcome {
        batch_id: parsed.batch.batch_id,
        sample_count: parsed.batch.samples.len(),
        accepted_count,
        replayed: accepted_count == 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricRange {
    OneHour,
    SixHours,
    OneDay,
    SevenDays,
}

impl MetricRange {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("1h") {
            "1h" => Ok(Self::OneHour),
            "6h" => Ok(Self::SixHours),
            "24h" => Ok(Self::OneDay),
            "7d" => Ok(Self::SevenDays),
            other => Err(format!(
                "unsupported metric range `{other}`; use 1h, 6h, 24h, or 7d"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::OneDay => "24h",
            Self::SevenDays => "7d",
        }
    }

    fn duration(self) -> time::Duration {
        match self {
            Self::OneHour => time::Duration::hours(1),
            Self::SixHours => time::Duration::hours(6),
            Self::OneDay => time::Duration::hours(24),
            Self::SevenDays => time::Duration::days(7),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MetricQuery {
    pub range: Option<String>,
}

pub async fn get_agent_metrics(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<MetricQuery>,
) -> (StatusCode, Json<Value>) {
    let range = match MetricRange::parse(query.range.as_deref()) {
        Ok(range) => range,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error),
    };
    match build_metrics_response(&state.pool, agent_id, range).await {
        Ok(Some(response)) => (StatusCode::OK, Json(response)),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn build_metrics_response(
    pool: &PgPool,
    agent_id: Uuid,
    range: MetricRange,
) -> sqlx::Result<Option<Value>> {
    let exists: Option<(Uuid,)> = sqlx::query_as("select id from agents where id = $1")
        .bind(agent_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        return Ok(None);
    }

    let now = OffsetDateTime::now_utc();
    let from = now - range.duration();
    let rows: Vec<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select sample_id, collected_at, received_at, metrics \
              from agent_metric_samples \
             where agent_id = $1 and collected_at >= $2 \
             order by collected_at, sample_id \
        ) t",
    )
    .bind(agent_id)
    .bind(from)
    .fetch_all(pool)
    .await?;
    let latest: Option<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from ( \
            select sample_id, collected_at, received_at, metrics \
              from agent_metric_samples where agent_id = $1 \
             order by collected_at desc, sample_id desc limit 1 \
        ) t",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;

    let latest_value = latest.map(|(value,)| value);
    let mut dimension_rows = rows.clone();
    if let Some(latest) = latest_value.as_ref() {
        dimension_rows.push((latest.clone(),));
    }
    let dimensions = collect_dimensions(&dimension_rows);
    let series = build_series(&rows, range);
    let freshness = freshness(latest_value.as_ref(), now);
    let availability = availability(latest_value.as_ref());

    Ok(Some(json!({
        "range": range.as_str(),
        "from": from,
        "to": now,
        "latest": latest_value,
        "freshness": freshness,
        "availability": availability,
        "dimensions": dimensions,
        "series": series,
    })))
}

fn build_series(rows: &[(Value,)], range: MetricRange) -> Vec<Value> {
    if rows.is_empty() {
        return Vec::new();
    }
    let group_count = if range == MetricRange::OneHour {
        rows.len()
    } else {
        rows.len().min(MAX_SERIES_POINTS)
    };
    let mut groups = (0..group_count)
        .map(|_| SeriesGroup::default())
        .collect::<Vec<_>>();
    for (index, (row,)) in rows.iter().enumerate() {
        let group = index.saturating_mul(group_count) / rows.len();
        let group = group.min(group_count.saturating_sub(1));
        let timestamp = row
            .get("collected_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        groups[group].timestamp = timestamp;
        groups[group].sample_count += 1;
        if let Some(metrics) = row.get("metrics") {
            let mut values = BTreeMap::new();
            flatten_numeric_metrics(metrics, "", &mut values);
            for (key, value) in values {
                groups[group].values.entry(key).or_default().push(value);
            }
        }
    }
    groups
        .into_iter()
        .map(|group| {
            let values = group
                .values
                .into_iter()
                .map(|(key, values)| {
                    let latest = values.last().copied().unwrap_or_default();
                    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
                    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let average = values.iter().sum::<f64>() / values.len() as f64;
                    (
                        key,
                        json!({
                            "average": average,
                            "minimum": minimum,
                            "maximum": maximum,
                            "latest": latest,
                        }),
                    )
                })
                .collect::<Map<String, Value>>();
            json!({
                "timestamp": group.timestamp,
                "sample_count": group.sample_count,
                "values": values,
            })
        })
        .collect()
}

#[derive(Default)]
struct SeriesGroup {
    timestamp: String,
    sample_count: usize,
    values: BTreeMap<String, Vec<f64>>,
}

fn flatten_numeric_metrics(value: &Value, prefix: &str, output: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_f64().filter(|value| value.is_finite()) {
                output.insert(prefix.to_string(), value);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_numeric_metrics(value, &next, output);
            }
        }
        Value::Array(items) => {
            let dimensioned = matches!(
                prefix,
                "network.interfaces" | "disk.devices" | "gpu.devices"
            );
            if !dimensioned {
                return;
            }
            for item in items {
                let id = if prefix == "gpu.devices" {
                    item.get("id")
                        .or_else(|| item.get("index"))
                        .or_else(|| item.get("name"))
                } else {
                    item.get("name")
                        .or_else(|| item.get("id"))
                        .or_else(|| item.get("index"))
                }
                .map(value_to_dimension)
                .unwrap_or_else(|| "unknown".to_string());
                flatten_numeric_metrics(item, &format!("{prefix}.{id}"), output);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

fn value_to_dimension(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|value| value.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn collect_dimensions(rows: &[(Value,)]) -> Value {
    let mut interfaces = BTreeSet::new();
    let mut disks = BTreeSet::new();
    let mut gpus: BTreeMap<String, Value> = BTreeMap::new();
    for (row,) in rows {
        let Some(metrics) = row.get("metrics") else {
            continue;
        };
        if let Some(items) = metrics["network"]["interfaces"].as_array() {
            for item in items {
                if let Some(name) = item.get("name").and_then(Value::as_str) {
                    interfaces.insert(name.to_string());
                }
            }
        }
        if let Some(items) = metrics["disk"]["devices"].as_array() {
            for item in items {
                if let Some(name) = item.get("name").and_then(Value::as_str) {
                    disks.insert(name.to_string());
                }
            }
        }
        if let Some(items) = metrics["gpu"]["devices"].as_array() {
            for item in items {
                let id = item
                    .get("id")
                    .or_else(|| item.get("index"))
                    .map(value_to_dimension)
                    .unwrap_or_else(|| "unknown".to_string());
                gpus.entry(id.clone()).or_insert_with(|| {
                    json!({
                        "id": id,
                        "name": item.get("name").cloned().unwrap_or(Value::Null),
                        "vendor": item.get("vendor").cloned().unwrap_or(Value::Null),
                    })
                });
            }
        }
    }
    json!({
        "network_interfaces": interfaces.into_iter().collect::<Vec<_>>(),
        "disk_devices": disks.into_iter().collect::<Vec<_>>(),
        "gpu_devices": gpus.into_values().collect::<Vec<_>>(),
    })
}

fn freshness(latest: Option<&Value>, now: OffsetDateTime) -> Value {
    let collected_at = latest
        .and_then(|value| value.get("collected_at"))
        .and_then(Value::as_str)
        .and_then(parse_timestamp);
    let Some(collected_at) = collected_at else {
        return json!({"state": "empty", "age_seconds": Value::Null});
    };
    let age_seconds = (now - collected_at).whole_seconds().max(0);
    json!({
        "state": if age_seconds <= FRESHNESS_STALE_AFTER_SECONDS { "fresh" } else { "stale" },
        "age_seconds": age_seconds,
        "collected_at": collected_at,
        "received_at": latest.and_then(|value| value.get("received_at")).cloned().unwrap_or(Value::Null),
    })
}

fn availability(latest: Option<&Value>) -> Value {
    latest
        .and_then(|value| value.get("metrics"))
        .and_then(|metrics| metrics.get("availability"))
        .cloned()
        .unwrap_or_else(|| {
            if latest.is_some() {
                json!({"status": "available", "collectors": {}})
            } else {
                json!({"status": "empty", "collectors": {}})
            }
        })
}

fn parse_timestamp(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

fn api_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message.into()})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_bounded_batches_and_rejects_wrong_type() {
        let agent_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let value = json!({
            "type": "metric_sample_batch",
            "message_id": Uuid::new_v4(),
            "protocol_version": protocol::PROTOCOL_VERSION,
            "schema_version": protocol::M5_SCHEMA_VERSION,
            "batch_id": batch_id,
            "agent_id": agent_id,
            "samples": [{
                "sample_id": Uuid::new_v4(),
                "collected_at_unix_secs": 1_700_000_000,
                "metrics": {"cpu": {"usage_percent": 12.0}}
            }]
        });
        let parsed = parse_metric_sample_batch_value(value).unwrap();
        assert_eq!(parsed.batch.batch_id, batch_id);
        assert!(parse_metric_sample_batch_value(json!({"type": "observation_batch"})).is_err());
    }

    #[test]
    fn range_validation_is_explicit() {
        assert_eq!(MetricRange::parse(None).unwrap(), MetricRange::OneHour);
        assert_eq!(
            MetricRange::parse(Some("7d")).unwrap(),
            MetricRange::SevenDays
        );
        assert!(MetricRange::parse(Some("2h")).is_err());
    }

    #[test]
    fn downsampling_retains_numeric_extrema_and_latest() {
        let rows = (0..1_000)
            .map(|index| {
                (json!({
                    "collected_at": format!("2026-09-21T00:{:02}:00Z", index / 60),
                    "metrics": {"cpu": {"usage_percent": index as f64}}
                }),)
            })
            .collect::<Vec<_>>();
        let series = build_series(&rows, MetricRange::SixHours);
        assert_eq!(series.len(), MAX_SERIES_POINTS);
        let first = &series[0]["values"]["cpu.usage_percent"];
        assert_eq!(first["minimum"], 0.0);
        assert!(first["maximum"].as_f64().unwrap() >= first["average"].as_f64().unwrap());
        assert!(first["latest"].as_f64().unwrap() >= first["minimum"].as_f64().unwrap());
    }

    #[test]
    fn dimensions_are_collected_from_multiple_samples() {
        let rows = vec![(json!({
            "metrics": {
                "network": {"interfaces": [{"name": "eth0"}]},
                "disk": {"devices": [{"name": "sda"}]},
                "gpu": {"devices": [{"id": "0", "name": "NVIDIA"}]}
            }
        }),)];
        let dimensions = collect_dimensions(&rows);
        assert_eq!(dimensions["network_interfaces"][0], "eth0");
        assert_eq!(dimensions["disk_devices"][0], "sda");
        assert_eq!(dimensions["gpu_devices"][0]["id"], "0");
    }
}
