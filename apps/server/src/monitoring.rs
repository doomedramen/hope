//! M4 monitor persistence and the proposal-to-monitor approval boundary.
//!
//! This slice creates durable monitor intent from an approved M3 proposal and
//! exposes the records. Check execution, leases, incidents, and notifications
//! consume these rows in subsequent M4 slices.

use anyhow::{Result, anyhow};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::inventory::events::Recorder;
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
    pub limit: Option<i64>,
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
                  s.product_version as service_product_version\
             from monitors m\
             join endpoints e on e.id = m.endpoint_id\
             join services s on s.id = m.service_id\
            where ($1::text is null or m.state = $1)\
            order by m.created_at desc, m.id desc\
            limit $2\
         ) t",
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
                      s.product_version as service_product_version\
                 from monitors m\
                 join endpoints e on e.id = m.endpoint_id\
                 join services s on s.id = m.service_id\
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
