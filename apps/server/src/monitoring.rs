//! M4 monitor persistence and the proposal-to-monitor approval boundary.
//!
//! This slice creates durable monitor intent from an approved M3 proposal and
//! exposes the records. Check execution, leases, incidents, and notifications
//! consume these rows in subsequent M4 slices.

use anyhow::{Result, anyhow};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::monitor_checks;
use crate::state::AppState;

const DEFAULT_INTERVAL_SECONDS: i32 = 30;
const DEFAULT_TIMEOUT_MS: i32 = 5_000;
const DEFAULT_FAILURE_THRESHOLD: i32 = 3;
const DEFAULT_RECOVERY_THRESHOLD: i32 = 2;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

#[derive(Debug, Deserialize)]
pub struct MonitorListQuery {
    pub state: Option<String>,
    pub agent_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MonitorResultsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct IncidentListQuery {
    pub state: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AgentMonitorCreateRequest {
    /// Used by POST `/api/v1/monitors`; path-scoped creation overwrites this
    /// with the authenticated route target and rejects a conflicting value.
    pub agent_id: Option<Uuid>,
    pub monitor_type: String,
    #[serde(default = "empty_object")]
    pub config: Value,
    /// Metric fields may be sent at the request root for a compact API call;
    /// `config` remains the canonical response/storage shape.
    pub metric: Option<String>,
    pub operator: Option<String>,
    pub threshold: Option<f64>,
    pub mount_point: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub interval_seconds: Option<i32>,
    pub timeout_ms: Option<i32>,
    pub failure_threshold: Option<i32>,
    pub recovery_threshold: Option<i32>,
    pub enabled: Option<bool>,
}

fn empty_object() -> Value {
    json!({})
}

/// Create one monitor from an approved proposal inside the approval
/// transaction. Existing monitors for the proposal are returned unchanged so
/// retries cannot reset state or erase a user's monitor configuration.
pub async fn create_from_proposal_tx(
    tx: &mut Transaction<'_, Postgres>,
    proposal_id: Uuid,
    created_by: Uuid,
) -> Result<Uuid> {
    let existing: Option<(Uuid,)> =
        sqlx::query_as("select id from monitors where proposal_id = $1 for update")
            .bind(proposal_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((monitor_id,)) = existing {
        return Ok(monitor_id);
    }

    let proposal: Option<(Uuid, Uuid, String, Value, Value, String)> = sqlx::query_as(
        "select service_id, endpoint_id, check_type, check_config, user_overrides, status \
         from monitor_proposals where id = $1 for update",
    )
    .bind(proposal_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((service_id, endpoint_id, check_type, check_config, user_overrides, status)) =
        proposal
    else {
        return Err(anyhow!("monitor proposal {proposal_id} not found"));
    };
    if status != "approved" {
        return Err(anyhow!(
            "monitor proposal {proposal_id} must be approved before monitor creation"
        ));
    }
    let config = merged_config(check_config, user_overrides);
    let interval_seconds = bounded_config_i32(
        &config,
        "interval_seconds",
        DEFAULT_INTERVAL_SECONDS,
        5,
        86_400,
    )?;
    let timeout_ms = bounded_config_i32(&config, "timeout_ms", DEFAULT_TIMEOUT_MS, 50, 60_000)?;
    let failure_threshold = bounded_config_i32(
        &config,
        "failure_threshold",
        DEFAULT_FAILURE_THRESHOLD,
        1,
        20,
    )?;
    let recovery_threshold = bounded_config_i32(
        &config,
        "recovery_threshold",
        DEFAULT_RECOVERY_THRESHOLD,
        1,
        20,
    )?;

    let (monitor_id,): (Uuid,) = sqlx::query_as(
        "insert into monitors \
            (proposal_id, service_id, endpoint_id, monitor_type, config, interval_seconds, \
             timeout_ms, failure_threshold, recovery_threshold, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         on conflict (proposal_id) do update set updated_at = monitors.updated_at \
         returning id",
    )
    .bind(proposal_id)
    .bind(service_id)
    .bind(endpoint_id)
    .bind(&check_type)
    .bind(&config)
    .bind(interval_seconds)
    .bind(timeout_ms)
    .bind(failure_threshold)
    .bind(recovery_threshold)
    .bind(created_by)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow!("monitor proposal {proposal_id} was approved concurrently"))?;

    let after: Value =
        sqlx::query_scalar("select row_to_json(t) from (select * from monitors where id = $1) t")
            .bind(monitor_id)
            .fetch_one(&mut **tx)
            .await?;
    Recorder::record_change(
        tx,
        "monitors",
        monitor_id,
        "monitor.created",
        "notice",
        None,
        Some(after),
        Some("manual"),
    )
    .await?;
    Ok(monitor_id)
}

fn merged_config(base: Value, overrides: Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    if let Some(overrides) = overrides.as_object() {
        merged.extend(overrides.clone());
    }
    Value::Object(merged)
}

fn bounded_config_i32(
    config: &Value,
    key: &str,
    default: i32,
    minimum: i32,
    maximum: i32,
) -> Result<i32> {
    let Some(value) = config.get(key) else {
        return Ok(default);
    };
    let value = value
        .as_i64()
        .ok_or_else(|| anyhow!("monitor config `{key}` must be an integer"))?;
    let value =
        i32::try_from(value).map_err(|_| anyhow!("monitor config `{key}` is out of range"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(anyhow!(
            "monitor config `{key}` must be between {minimum} and {maximum}"
        ));
    }
    Ok(value)
}

fn request_agent_config(request: &AgentMonitorCreateRequest) -> Result<Value> {
    let mut config = request
        .config
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("agent monitor config must be a JSON object"))?;
    let mut add_root_value = |key: &str, value: Value| -> Result<()> {
        if config.insert(key.to_string(), value).is_some() {
            return Err(anyhow!(
                "agent monitor field `{key}` is duplicated in config"
            ));
        }
        Ok(())
    };
    if let Some(value) = request.metric.as_deref() {
        add_root_value("metric", json!(value))?;
    }
    if let Some(value) = request.operator.as_deref() {
        add_root_value("operator", json!(value))?;
    }
    if let Some(value) = request.threshold {
        if !value.is_finite() {
            return Err(anyhow!("agent metric threshold must be finite"));
        }
        add_root_value("threshold", json!(value))?;
    }
    if let Some(value) = request.mount_point.as_deref() {
        add_root_value("mount_point", json!(value))?;
    }
    if let Some(value) = request.timeout_seconds {
        add_root_value("timeout_seconds", json!(value))?;
    }
    Ok(Value::Object(config))
}

fn request_timing(request: &AgentMonitorCreateRequest) -> Result<(i32, i32, i32, i32, bool)> {
    let interval_seconds = request.interval_seconds.unwrap_or(DEFAULT_INTERVAL_SECONDS);
    let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let failure_threshold = request
        .failure_threshold
        .unwrap_or(DEFAULT_FAILURE_THRESHOLD);
    let recovery_threshold = request
        .recovery_threshold
        .unwrap_or(DEFAULT_RECOVERY_THRESHOLD);
    if !(5..=86_400).contains(&interval_seconds) {
        return Err(anyhow!("interval_seconds must be between 5 and 86400"));
    }
    if !(50..=60_000).contains(&timeout_ms) {
        return Err(anyhow!("timeout_ms must be between 50 and 60000"));
    }
    if !(1..=20).contains(&failure_threshold) {
        return Err(anyhow!("failure_threshold must be between 1 and 20"));
    }
    if !(1..=20).contains(&recovery_threshold) {
        return Err(anyhow!("recovery_threshold must be between 1 and 20"));
    }
    Ok((
        interval_seconds,
        timeout_ms,
        failure_threshold,
        recovery_threshold,
        request.enabled.unwrap_or(true),
    ))
}

/// Create an agent monitor without manufacturing a service or endpoint row.
/// Agent metric values come from `agent_inventory_current`; the gateway's
/// bounded observation envelope remains outside this slice and is not used as
/// a second metric source.
pub async fn create_agent_monitor_record(
    pool: &sqlx::PgPool,
    agent_id: Uuid,
    request: AgentMonitorCreateRequest,
    created_by: Uuid,
) -> Result<Value> {
    let default_timeout: Option<(i32,)> = sqlx::query_as(
        "select heartbeat_timeout_seconds from agents \
         where id = $1 and revoked_at is null",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let Some((default_timeout,)) = default_timeout else {
        return Err(anyhow!("agent not found or revoked"));
    };
    let default_timeout = u64::try_from(default_timeout)
        .map_err(|_| anyhow!("agent heartbeat timeout is invalid"))?;
    let config = request_agent_config(&request)?;
    monitor_checks::validate_agent_monitor_config(&request.monitor_type, &config, default_timeout)
        .map_err(|error| anyhow!(error.to_string()))?;
    let (interval_seconds, timeout_ms, failure_threshold, recovery_threshold, enabled) =
        request_timing(&request)?;

    let mut tx = pool.begin().await?;
    let monitor_id: Uuid = sqlx::query_scalar(
        "insert into monitors \
            (agent_id, monitor_type, config, interval_seconds, timeout_ms, \
             failure_threshold, recovery_threshold, enabled, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9) returning id",
    )
    .bind(agent_id)
    .bind(&request.monitor_type)
    .bind(&config)
    .bind(interval_seconds)
    .bind(timeout_ms)
    .bind(failure_threshold)
    .bind(recovery_threshold)
    .bind(enabled)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    let after: Value =
        sqlx::query_scalar("select row_to_json(t) from (select * from monitors where id = $1) t")
            .bind(monitor_id)
            .fetch_one(&mut *tx)
            .await?;
    Recorder::record_change(
        &mut tx,
        "monitors",
        monitor_id,
        "monitor.created",
        "notice",
        None,
        Some(after.clone()),
        Some("manual"),
    )
    .await?;
    tx.commit().await?;
    Ok(after)
}

fn monitor_create_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    let status = if error.downcast_ref::<sqlx::Error>().is_some() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::BAD_REQUEST
    };
    err(status, error.to_string())
}

pub async fn create_agent_monitor(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<Uuid>,
    Json(mut request): Json<AgentMonitorCreateRequest>,
) -> (StatusCode, Json<Value>) {
    if let Some(body_agent_id) = request.agent_id
        && body_agent_id != agent_id
    {
        return err(
            StatusCode::BAD_REQUEST,
            "body agent_id does not match path agent_id",
        );
    }
    request.agent_id = Some(agent_id);
    match create_agent_monitor_record(&state.pool, agent_id, request, user.0).await {
        Ok(value) => (StatusCode::CREATED, Json(value)),
        Err(error) => monitor_create_error(error),
    }
}

pub async fn create_monitor(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<AgentMonitorCreateRequest>,
) -> (StatusCode, Json<Value>) {
    let Some(agent_id) = request.agent_id else {
        return err(
            StatusCode::BAD_REQUEST,
            "agent_id is required for agent monitors",
        );
    };
    match create_agent_monitor_record(&state.pool, agent_id, request, user.0).await {
        Ok(value) => (StatusCode::CREATED, Json(value)),
        Err(error) => monitor_create_error(error),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<MonitorListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = query.limit.unwrap_or(100).clamp(1, 100);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (\
           select m.*,\
                  e.address::text as endpoint_address,\
                  e.port as endpoint_port,\
                  e.url as endpoint_url,\
                  e.dns_name as endpoint_dns_name,\
                  s.name as service_name,\
                  s.product as service_product,\
                  s.product_version as service_product_version,\
                  m.agent_id, a.hostname as agent_hostname\
             from monitors m\
             left join endpoints e on e.id = m.endpoint_id\
             left join services s on s.id = m.service_id\
             left join agents a on a.id = m.agent_id\
            where ($1::text is null or m.state = $1)\
              and ($2::uuid is null or m.agent_id = $2)\
            order by m.created_at desc, m.id desc\
            limit $3\
         ) t",
    )
    .bind(query.state)
    .bind(query.agent_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "items": rows.into_iter().map(|(row,)| row).collect::<Vec<_>>() })),
        ),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn get(State(state): State<AppState>, Path(id): Path<Uuid>) -> (StatusCode, Json<Value>) {
    let row: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (\
               select m.*,\
                      e.address::text as endpoint_address,\
                      e.port as endpoint_port,\
                      e.url as endpoint_url,\
                      e.dns_name as endpoint_dns_name,\
                      s.name as service_name,\
                      s.product as service_product,\
                      s.product_version as service_product_version,\
                      m.agent_id, a.hostname as agent_hostname\
                 from monitors m\
                 left join endpoints e on e.id = m.endpoint_id\
                 left join services s on s.id = m.service_id\
                 left join agents a on a.id = m.agent_id\
                where m.id = $1\
             ) t",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((row,))) => (StatusCode::OK, Json(row)),
        Ok(None) => err(StatusCode::NOT_FOUND, "monitor not found"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn results(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<MonitorResultsQuery>,
) -> (StatusCode, Json<Value>) {
    let exists: Result<bool, sqlx::Error> =
        sqlx::query_scalar("select exists(select 1 from monitors where id = $1)")
            .bind(id)
            .fetch_one(&state.pool)
            .await;
    match exists {
        Ok(false) => return err(StatusCode::NOT_FOUND, "monitor not found"),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        Ok(true) => {}
    }

    let limit = query.limit.unwrap_or(100).clamp(1, 100);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (\
           select * from monitor_results\
            where monitor_id = $1\
            order by observed_at desc, id desc\
            limit $2\
         ) t",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "items": rows.into_iter().map(|(row,)| row).collect::<Vec<_>>() })),
        ),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn incidents(
    State(state): State<AppState>,
    Query(query): Query<IncidentListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = query.limit.unwrap_or(100).clamp(1, 100);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        r#"
        select row_to_json(t) from (
           select i.*,
                  m.service_id,
                  m.endpoint_id,
                  m.monitor_type,
                  m.state as monitor_state,
                  e.address::text as endpoint_address,
                  e.port as endpoint_port,
                  e.url as endpoint_url,
                  e.dns_name as endpoint_dns_name,
                  s.name as service_name,
                  s.product as service_product,
                  m.agent_id, a.hostname as agent_hostname,
                  (select coalesce(jsonb_agg(row_to_json(suppression) order by suppression.created_at), '[]'::jsonb)
                     from (select sns.id, sns.root_incident_id, sns.dependency_edge_id,
                                  sns.dependency_path, sns.provider_kind, sns.provider_id,
                                  sns.event_type, sns.reason, sns.created_at
                             from incident_notification_suppressions sns
                            where sns.incident_id = i.id) suppression)
                    as notification_suppressions
             from incidents i
             join monitors m on m.id = i.monitor_id
             left join endpoints e on e.id = m.endpoint_id
             left join services s on s.id = m.service_id
             left join agents a on a.id = m.agent_id
            where ($1::text is null or i.state = $1)
            order by i.last_event_at desc, i.id desc
            limit $2
         ) t
        "#,
    )
    .bind(query.state)
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "items": rows.into_iter().map(|(row,)| row).collect::<Vec<_>>() })),
        ),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn incident(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let row: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        r#"
        select row_to_json(t) from (
           select i.*,
                  m.service_id,
                  m.endpoint_id,
                  m.monitor_type,
                  m.state as monitor_state,
                  e.address::text as endpoint_address,
                  e.port as endpoint_port,
                  e.url as endpoint_url,
                  e.dns_name as endpoint_dns_name,
                  s.name as service_name,
                  s.product as service_product,
                  m.agent_id, a.hostname as agent_hostname,
                  (select coalesce(jsonb_agg(row_to_json(suppression) order by suppression.created_at), '[]'::jsonb)
                     from (select sns.id, sns.root_incident_id, sns.dependency_edge_id,
                                  sns.dependency_path, sns.provider_kind, sns.provider_id,
                                  sns.event_type, sns.reason, sns.created_at
                             from incident_notification_suppressions sns
                            where sns.incident_id = i.id) suppression)
                    as notification_suppressions
             from incidents i
             join monitors m on m.id = i.monitor_id
             left join endpoints e on e.id = m.endpoint_id
             left join services s on s.id = m.service_id
             left join agents a on a.id = m.agent_id
            where i.id = $1
         ) t
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((row,))) => (StatusCode::OK, Json(row)),
        Ok(None) => err(StatusCode::NOT_FOUND, "incident not found"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    #[test]
    fn merged_config_keeps_proposal_values_and_applies_overrides() {
        let config = merged_config(
            json!({ "path": "/", "timeout_ms": 5000 }),
            json!({ "path": "/health", "failure_threshold": 3 }),
        );
        assert_eq!(config["path"], "/health");
        assert_eq!(config["timeout_ms"], 5000);
        assert_eq!(config["failure_threshold"], 3);
    }

    #[test]
    fn bounded_config_rejects_invalid_thresholds() {
        let error = bounded_config_i32(
            &json!({ "failure_threshold": 0 }),
            "failure_threshold",
            2,
            1,
            20,
        )
        .expect_err("zero threshold must be rejected");
        assert!(error.to_string().contains("failure_threshold"));
    }

    #[tokio::test]
    async fn incident_responses_include_suppression_reasons() {
        let Some(database_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let device_id: Uuid =
            sqlx::query_scalar("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let service_id: Uuid = sqlx::query_scalar(
            "insert into services (protocol, owner_kind, owner_id) \
             values ('http', 'device', $1) returning id",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let endpoint_id: Uuid = sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '127.0.0.1'::inet, 1) returning id",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let monitor_id: Uuid = sqlx::query_scalar(
            "insert into monitors (service_id, endpoint_id, monitor_type) \
             values ($1, $2, 'http') returning id",
        )
        .bind(service_id)
        .bind(endpoint_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let result_id: Uuid = sqlx::query_scalar(
            "insert into monitor_results (monitor_id, status, error) \
             values ($1, 'failure', 'test') returning id",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let incident_id: Uuid = sqlx::query_scalar(
            "insert into incidents (monitor_id, state, severity, last_result_id, summary) \
             values ($1, 'open', 'critical', $2, 'test incident') returning id",
        )
        .bind(monitor_id)
        .bind(result_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let edge_id: Uuid = sqlx::query_scalar(
            "insert into dependency_edges \
                (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                 criticality, origin, health_propagation, confirmation_state) \
             values ('devices', $1, 'services', $2, 'host', 'hard', 'manual', \
                     'suppress_only', 'confirmed') returning id",
        )
        .bind(device_id)
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into incident_notification_suppressions \
                (incident_id, root_incident_id, dependency_edge_id, dependency_path, \
                 provider_kind, provider_id, event_type, reason) \
             values ($1, $1, $2, $3, 'devices', $4, 'incident.opened', $5)",
        )
        .bind(incident_id)
        .bind(edge_id)
        .bind(json!([edge_id]))
        .bind(device_id)
        .bind("suppressed by test dependency")
        .execute(&pool)
        .await
        .unwrap();

        let (status, Json(body)) = incidents(
            State(AppState { pool: pool.clone() }),
            Query(IncidentListQuery {
                state: Some("open".to_string()),
                limit: Some(100),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let item = body["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == json!(incident_id)))
            .expect("incident is listed");
        let suppressions = item["notification_suppressions"]
            .as_array()
            .expect("suppression array");
        assert_eq!(suppressions.len(), 1);
        assert_eq!(suppressions[0]["dependency_edge_id"], json!(edge_id));
        assert_eq!(suppressions[0]["reason"], "suppressed by test dependency");
    }

    #[test]
    fn agent_request_accepts_root_metric_fields_and_normalizes_storage_config() {
        let request = AgentMonitorCreateRequest {
            agent_id: Some(Uuid::new_v4()),
            monitor_type: monitor_checks::AGENT_METRIC_MONITOR_TYPE.to_string(),
            config: json!({}),
            metric: Some("host.load.1".to_string()),
            operator: Some("gt".to_string()),
            threshold: Some(4.0),
            mount_point: None,
            timeout_seconds: None,
            interval_seconds: None,
            timeout_ms: None,
            failure_threshold: None,
            recovery_threshold: None,
            enabled: None,
        };
        let config = request_agent_config(&request).expect("normalize metric fields");
        assert_eq!(config["metric"], "host.load.1");
        assert_eq!(config["operator"], "gt");
        assert_eq!(config["threshold"], 4.0);
    }

    #[tokio::test]
    async fn agent_monitor_creation_uses_agent_target_without_service_endpoint() {
        let Some(database_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = sqlx::PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let agent_id: Uuid = sqlx::query_scalar(
            "insert into agents \
                (cert_fingerprint, cert_serial, hostname, heartbeat_timeout_seconds) \
             values ($1, $2, 'monitor-agent', 45) returning id",
        )
        .bind(format!("monitor-agent-{}", Uuid::new_v4()))
        .bind(format!("serial-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();
        let user_id: Uuid = sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("agent-monitor-{}@example.test", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();

        let row = create_agent_monitor_record(
            &pool,
            agent_id,
            AgentMonitorCreateRequest {
                agent_id: None,
                monitor_type: monitor_checks::AGENT_HEARTBEAT_MONITOR_TYPE.to_string(),
                config: json!({}),
                metric: None,
                operator: None,
                threshold: None,
                mount_point: None,
                timeout_seconds: Some(45),
                interval_seconds: Some(30),
                timeout_ms: Some(500),
                failure_threshold: Some(2),
                recovery_threshold: Some(2),
                enabled: Some(true),
            },
            user_id,
        )
        .await
        .unwrap();
        let monitor_id: Uuid = row["id"].as_str().unwrap().parse().unwrap();
        let persisted: (Option<Uuid>, Option<Uuid>, Uuid, String, Value) = sqlx::query_as(
            "select service_id, endpoint_id, agent_id, monitor_type, config \
             from monitors where id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted.0, None);
        assert_eq!(persisted.1, None);
        assert_eq!(persisted.2, agent_id);
        assert_eq!(persisted.3, monitor_checks::AGENT_HEARTBEAT_MONITOR_TYPE);
        assert_eq!(persisted.4["timeout_seconds"], 45);

        let change_count: i64 = sqlx::query_scalar(
            "select count(*) from change_events \
             where entity_kind = 'monitors' and entity_id = $1 \
               and category = 'monitor.created'",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(change_count, 1);
    }

    #[tokio::test]
    async fn approved_proposal_creates_one_monitor_without_resetting_it() {
        let Some(database_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

        let user_id: Uuid = sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("monitor-{}@example.test", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();
        let device_id: Uuid =
            sqlx::query_scalar("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let service_id: Uuid = sqlx::query_scalar(
            "insert into services (protocol, owner_kind, owner_id) \
             values ('http', 'device', $1) returning id",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let address_bytes = Uuid::new_v4().into_bytes();
        let endpoint_id: Uuid = sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port, is_current) \
             values ($1, 'socket', $2::inet, 8080, true) returning id",
        )
        .bind(service_id)
        .bind(format!("198.18.{}.{}", address_bytes[0], address_bytes[1]))
        .fetch_one(&pool)
        .await
        .unwrap();
        let proposal_id: Uuid = sqlx::query_scalar(
            "insert into monitor_proposals \
                (service_id, endpoint_id, rule_id, target_identity, target, protocol, \
                 check_type, check_config, confidence, status, decision_source, decided_by, decided_at) \
             values ($1, $2, 'monitor.http.generic', $3, '{}'::jsonb, 'http', 'http', \
                     $4, 0.8, 'approved', 'manual', $5, now()) returning id",
        )
        .bind(service_id)
        .bind(endpoint_id)
        .bind(format!("endpoint:{endpoint_id}"))
        .bind(json!({ "method": "GET", "path": "/health", "timeout_ms": 3000 }))
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let first_id = {
            let mut tx = pool.begin().await.unwrap();
            let id = create_from_proposal_tx(&mut tx, proposal_id, user_id)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            id
        };
        let second_id = {
            let mut tx = pool.begin().await.unwrap();
            let id = create_from_proposal_tx(&mut tx, proposal_id, user_id)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            id
        };
        assert_eq!(first_id, second_id);
        let row: (Uuid, String, Value, i32, i32, String) = sqlx::query_as(
            "select proposal_id, monitor_type, config, timeout_ms, failure_threshold, state \
             from monitors where id = $1",
        )
        .bind(first_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, proposal_id);
        assert_eq!(row.1, "http");
        assert_eq!(row.2["path"], "/health");
        assert_eq!(row.3, 3000);
        assert_eq!(row.4, DEFAULT_FAILURE_THRESHOLD);
        assert_eq!(row.5, "unknown");
        let change_count: i64 = sqlx::query_scalar(
            "select count(*) from change_events \
             where entity_kind = 'monitors' and entity_id = $1 \
               and category = 'monitor.created'",
        )
        .bind(first_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(change_count, 1);
    }
}
