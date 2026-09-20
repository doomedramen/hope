//! Durable incident notification routing.
//!
//! Incident transitions create one delivery per matching channel and event.
//! Delivery is a normal retryable job, so the monitor transaction never waits
//! on a remote provider and a provider outage cannot lose the incident.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::dependency_graph;
use crate::state::AppState;

const EVENT_OPENED: &str = "incident.opened";
const EVENT_RECOVERED: &str = "incident.recovered";
const MAX_CHANNELS: i64 = 100;
const MAX_ROUTES: i64 = 200;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn default_enabled() -> bool {
    true
}

fn default_event_types() -> Vec<String> {
    vec![EVENT_OPENED.to_string(), EVENT_RECOVERED.to_string()]
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub config: Value,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChannelListQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRouteRequest {
    pub channel_id: Uuid,
    #[serde(default = "default_min_severity")]
    pub min_severity: String,
    #[serde(default = "default_event_types")]
    pub event_types: Vec<String>,
    #[serde(default)]
    pub delay_seconds: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct RouteListQuery {
    pub limit: Option<i64>,
}

fn default_min_severity() -> String {
    "warning".to_string()
}

/// The durable explanation attached to an incident when a dependency-aware
/// policy suppresses its downstream notification.
#[derive(Debug, Clone, Serialize)]
pub struct SuppressionReason {
    pub root_incident_id: Uuid,
    pub dependency_edge_id: Uuid,
    pub dependency_path: Vec<Uuid>,
    pub provider_kind: String,
    pub provider_id: Uuid,
    pub reason: String,
}

pub struct IncidentNotification<'a> {
    pub incident_id: Uuid,
    pub monitor_id: Uuid,
    pub event_type: &'a str,
    pub severity: &'a str,
    pub result_id: Uuid,
    pub summary: &'a str,
}

/// Find open incidents on upstream dependencies of a service. The graph
/// helper bounds traversal; this function adds the operational question of
/// whether each upstream entity currently has an open monitored incident.
pub async fn find_suppressions(pool: &PgPool, service_id: Uuid) -> Result<Vec<SuppressionReason>> {
    const MAX_SUPPRESSIONS: usize = 32;
    let paths = dependency_graph::upstream_dependency_paths(pool, "services", service_id).await?;
    let mut suppressions = Vec::new();

    for path in paths {
        if path.edge_ids.is_empty() || suppressions.len() >= MAX_SUPPRESSIONS {
            continue;
        }
        let suppressible: Option<bool> = sqlx::query_scalar(
            "select bool_and(criticality = 'hard' or health_propagation in ('propagate', 'suppress_only')) \
               from dependency_edges \
              where id = any($1::uuid[]) and confirmation_state = 'confirmed'",
        )
        .bind(&path.edge_ids)
        .fetch_one(pool)
        .await?;
        if !suppressible.unwrap_or(false) {
            continue;
        }
        let root_incident: Option<(Uuid,)> = sqlx::query_as(
            r#"
            select i.id
              from incidents i
              join monitors m on m.id = i.monitor_id
              left join services s on s.id = m.service_id
             where i.state = 'open'
               and (
                   ($1 in ('service', 'services') and s.id = $2)
                   or ($1 in ('workload', 'workloads')
                       and s.owner_kind = 'workload' and s.owner_id = $2)
                   or ($1 in ('device', 'devices') and (
                       (s.owner_kind = 'device' and s.owner_id = $2)
                       or (s.owner_kind = 'workload' and exists (
                           select 1 from workloads w
                            where w.id = s.owner_id and w.host_device_id = $2
                       ))
                       or exists (
                           select 1
                             from containment_edges ce
                            where ce.parent_kind in ('device', 'devices')
                              and ce.parent_id = $2
                              and ce.child_kind in ('service', 'services')
                              and ce.child_id = s.id
                       )
                   ))
               )
             order by i.opened_at asc, i.id asc
             limit 1
            "#,
        )
        .bind(&path.provider_kind)
        .bind(path.provider_id)
        .fetch_optional(pool)
        .await?;
        let Some((root_incident_id,)) = root_incident else {
            continue;
        };

        let dependency_edge_id = path.edge_ids[0];
        if suppressions.iter().any(|existing: &SuppressionReason| {
            existing.root_incident_id == root_incident_id
                && existing.dependency_edge_id == dependency_edge_id
        }) {
            continue;
        }
        suppressions.push(SuppressionReason {
            root_incident_id,
            dependency_edge_id,
            dependency_path: path.edge_ids,
            provider_kind: path.provider_kind.clone(),
            provider_id: path.provider_id,
            reason: format!(
                "suppressed by open incident {root_incident_id} on dependency {}/{}",
                path.provider_kind, path.provider_id
            ),
        });
    }

    Ok(suppressions)
}

pub async fn list_channels(
    State(state): State<AppState>,
    Query(query): Query<ChannelListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = query.limit.unwrap_or(MAX_CHANNELS).clamp(1, MAX_CHANNELS);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (\
           select id, name, provider, config, enabled, created_at, updated_at \
             from notification_channels \
            order by created_at desc, id desc \
            limit $1 \
         ) t",
    )
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => {
            let items = rows
                .into_iter()
                .map(|(row,)| redact_channel(row))
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({ "items": items })))
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn create_channel(
    State(state): State<AppState>,
    Json(input): Json<CreateChannelRequest>,
) -> (StatusCode, Json<Value>) {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 {
        return err(
            StatusCode::BAD_REQUEST,
            "channel name must be 1-128 characters",
        );
    }
    if let Err(error) = validate_channel(&input.provider, &input.config) {
        return err(StatusCode::BAD_REQUEST, error.to_string());
    }
    let row: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        "insert into notification_channels (name, provider, config, enabled) \
         values ($1, $2, $3, $4) \
         returning id",
    )
    .bind(name)
    .bind(&input.provider)
    .bind(&input.config)
    .bind(input.enabled)
    .fetch_one(&state.pool)
    .await;
    match row {
        Ok((id,)) => {
            let created: Result<(Value,), sqlx::Error> = sqlx::query_as(
                "select row_to_json(t) from (\
                   select id, name, provider, config, enabled, created_at, updated_at \
                     from notification_channels where id = $1 \
                 ) t",
            )
            .bind(id)
            .fetch_one(&state.pool)
            .await;
            match created {
                Ok((created,)) => (StatusCode::CREATED, Json(redact_channel(created))),
                Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            }
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn list_routes(
    State(state): State<AppState>,
    Query(query): Query<RouteListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = query.limit.unwrap_or(MAX_ROUTES).clamp(1, MAX_ROUTES);
    let rows: Result<Vec<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (\
           select r.*, c.name as channel_name, c.provider \
             from notification_routes r \
             join notification_channels c on c.id = r.channel_id \
            order by r.created_at desc, r.id desc \
            limit $1 \
         ) t",
    )
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

pub async fn create_route(
    State(state): State<AppState>,
    Json(input): Json<CreateRouteRequest>,
) -> (StatusCode, Json<Value>) {
    if !severity_is_valid(&input.min_severity)
        || !(0..=86_400).contains(&input.delay_seconds)
        || !event_types_are_valid(&input.event_types)
    {
        return err(StatusCode::BAD_REQUEST, "invalid notification route");
    }
    let channel_exists: Result<bool, sqlx::Error> =
        sqlx::query_scalar("select exists(select 1 from notification_channels where id = $1)")
            .bind(input.channel_id)
            .fetch_one(&state.pool)
            .await;
    match channel_exists {
        Ok(false) => return err(StatusCode::NOT_FOUND, "notification channel not found"),
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        Ok(true) => {}
    }
    let event_types = match serde_json::to_value(&input.event_types) {
        Ok(value) => value,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let row: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        "insert into notification_routes \
             (channel_id, min_severity, event_types, delay_seconds, enabled) \
         values ($1, $2, $3, $4, $5) \
         returning id",
    )
    .bind(input.channel_id)
    .bind(&input.min_severity)
    .bind(event_types)
    .bind(input.delay_seconds)
    .bind(input.enabled)
    .fetch_one(&state.pool)
    .await;
    match row {
        Ok((id,)) => {
            let created: Result<(Value,), sqlx::Error> = sqlx::query_as(
                "select row_to_json(t) from (\
                   select r.*, c.name as channel_name, c.provider \
                     from notification_routes r \
                     join notification_channels c on c.id = r.channel_id \
                    where r.id = $1 \
                 ) t",
            )
            .bind(id)
            .fetch_one(&state.pool)
            .await;
            match created {
                Ok((created,)) => (StatusCode::CREATED, Json(created)),
                Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            }
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// Queue matching notification deliveries in the same transaction as the
/// incident transition. Replaying the transition cannot duplicate a delivery.
pub async fn enqueue_incident_notifications(
    tx: &mut Transaction<'_, Postgres>,
    notification: IncidentNotification<'_>,
    suppressions: &[SuppressionReason],
) -> Result<()> {
    if !matches!(notification.event_type, EVENT_OPENED | EVENT_RECOVERED)
        || !severity_is_valid(notification.severity)
    {
        return Err(anyhow!("invalid notification event"));
    }
    if notification.event_type == EVENT_OPENED && !suppressions.is_empty() {
        for suppression in suppressions {
            sqlx::query(
                "insert into incident_notification_suppressions \
                    (incident_id, root_incident_id, dependency_edge_id, dependency_path, \
                     provider_kind, provider_id, event_type, reason) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8) \
                 on conflict (incident_id, root_incident_id, dependency_edge_id, event_type) \
                 do nothing",
            )
            .bind(notification.incident_id)
            .bind(suppression.root_incident_id)
            .bind(suppression.dependency_edge_id)
            .bind(serde_json::to_value(&suppression.dependency_path)?)
            .bind(&suppression.provider_kind)
            .bind(suppression.provider_id)
            .bind(notification.event_type)
            .bind(&suppression.reason)
            .execute(&mut **tx)
            .await?;
        }
        return Ok(());
    }
    let payload = json!({
        "event": notification.event_type,
        "incident_id": notification.incident_id,
        "monitor_id": notification.monitor_id,
        "result_id": notification.result_id,
        "severity": notification.severity,
        "summary": notification.summary,
    });
    let routes: Vec<(Uuid, i32, String)> = sqlx::query_as(
        "select r.channel_id, r.delay_seconds, r.min_severity \
           from notification_routes r \
           join notification_channels c on c.id = r.channel_id \
          where r.enabled and c.enabled and r.event_types ? $1",
    )
    .bind(notification.event_type)
    .fetch_all(&mut **tx)
    .await?;
    for (channel_id, delay_seconds, min_severity) in routes {
        if severity_rank(notification.severity) < severity_rank(&min_severity) {
            continue;
        }
        let delivery: Option<(Uuid,)> = sqlx::query_as(
            "insert into notification_deliveries \
                 (incident_id, channel_id, event_type, severity, payload) \
             values ($1, $2, $3, $4, $5) \
             on conflict (incident_id, channel_id, event_type) do nothing \
             returning id",
        )
        .bind(notification.incident_id)
        .bind(channel_id)
        .bind(notification.event_type)
        .bind(notification.severity)
        .bind(&payload)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((delivery_id,)) = delivery else {
            continue;
        };
        let job_id = jobs::enqueue_in(
            tx,
            "notifications.deliver",
            &format!("notification:{delivery_id}"),
            json!({ "delivery_id": delivery_id }),
        )
        .await?;
        if delay_seconds > 0 {
            sqlx::query(
                "update jobs set run_at = now() + make_interval(secs => $2) \
                 where id = $1",
            )
            .bind(job_id)
            .bind(f64::from(delay_seconds))
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

#[derive(Debug, FromRow)]
struct Delivery {
    id: Uuid,
    provider: String,
    config: Value,
    payload: Value,
    status: String,
}

/// Deliver one queued notification. The job layer retries provider failures;
/// the delivery row makes the send idempotent at the application boundary.
pub async fn deliver(pool: &sqlx::PgPool, delivery_id: Uuid) -> Result<()> {
    let mut tx = pool.begin().await?;
    let Some(delivery): Option<Delivery> = sqlx::query_as(
        "select d.id, c.provider, c.config, d.payload, d.status \
           from notification_deliveries d \
           join notification_channels c on c.id = d.channel_id \
          where d.id = $1 \
          for update",
    )
    .bind(delivery_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(anyhow!("notification delivery not found"));
    };
    if delivery.status == "sent" {
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query(
        "update notification_deliveries \
            set status = 'sending', attempts = attempts + 1, updated_at = now() \
          where id = $1",
    )
    .bind(delivery.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let result = send_delivery(&delivery).await;
    match result {
        Ok(()) => {
            sqlx::query(
                "update notification_deliveries \
                    set status = 'sent', delivered_at = now(), last_error = null, \
                        updated_at = now() \
                  where id = $1",
            )
            .bind(delivery.id)
            .execute(pool)
            .await?;
            Ok(())
        }
        Err(error) => {
            let message = error.to_string();
            sqlx::query(
                "update notification_deliveries \
                    set status = 'pending', last_error = $2, updated_at = now() \
                  where id = $1",
            )
            .bind(delivery.id)
            .bind(&message)
            .execute(pool)
            .await?;
            Err(error)
        }
    }
}

async fn send_delivery(delivery: &Delivery) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    match delivery.provider.as_str() {
        "webhook" => {
            let url = config_string(&delivery.config, "url")?;
            let mut request = client
                .post(url)
                .header("x-hope-notification-id", delivery.id.to_string())
                .json(&delivery.payload);
            if let Some(token) = delivery.config.get("token").and_then(Value::as_str) {
                request = request.bearer_auth(token);
            }
            send_response(request).await
        }
        "ntfy" => {
            let base_url = config_string(&delivery.config, "url")?;
            let topic = config_string(&delivery.config, "topic")?;
            let url = format!("{}/{}", base_url.trim_end_matches('/'), topic);
            let summary = delivery
                .payload
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("Monitor incident");
            let severity = delivery
                .payload
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("warning");
            let mut request = client
                .post(url)
                .header("Title", summary)
                .header(
                    "Priority",
                    if severity == "critical" {
                        "urgent"
                    } else {
                        "default"
                    },
                )
                .header(
                    "Tags",
                    if severity == "critical" {
                        "warning"
                    } else {
                        "information_source"
                    },
                )
                .body(summary.to_string());
            if let Some(token) = delivery.config.get("token").and_then(Value::as_str) {
                request = request.bearer_auth(token);
            }
            send_response(request).await
        }
        other => Err(anyhow!("unsupported notification provider {other}")),
    }
}

async fn send_response(request: reqwest::RequestBuilder) -> Result<()> {
    let response = request.send().await?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!(
            "notification provider returned HTTP {}",
            response.status()
        ))
    }
}

fn validate_channel(provider: &str, config: &Value) -> Result<()> {
    let object = config
        .as_object()
        .ok_or_else(|| anyhow!("channel config must be an object"))?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("channel config requires url"))?;
    validate_url(url)?;
    if provider == "ntfy" {
        let topic = object
            .get("topic")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ntfy config requires topic"))?;
        if topic.is_empty()
            || topic.len() > 128
            || !topic
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(anyhow!("ntfy topic is invalid"));
        }
    } else if provider != "webhook" {
        return Err(anyhow!("unsupported notification provider"));
    }
    if let Some(token) = object.get("token").and_then(Value::as_str)
        && token.len() > 4_096
    {
        return Err(anyhow!("notification token is too long"));
    }
    Ok(())
}

fn validate_url(value: &str) -> Result<()> {
    if value.len() > 2_048 {
        return Err(anyhow!("notification URL is too long"));
    }
    let url = reqwest::Url::parse(value).context("notification URL is invalid")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(anyhow!("notification URL must be HTTP or HTTPS"));
    }
    Ok(())
}

fn config_string<'a>(config: &'a Value, key: &str) -> Result<&'a str> {
    config
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("notification config requires {key}"))
}

fn event_types_are_valid(event_types: &[String]) -> bool {
    !event_types.is_empty()
        && event_types.iter().collect::<HashSet<_>>().len() == event_types.len()
        && event_types
            .iter()
            .all(|event| matches!(event.as_str(), EVENT_OPENED | EVENT_RECOVERED))
}

fn severity_is_valid(value: &str) -> bool {
    matches!(value, "info" | "notice" | "warning" | "critical")
}

fn severity_rank(value: &str) -> i32 {
    match value {
        "critical" => 3,
        "warning" => 2,
        "notice" => 1,
        _ => 0,
    }
}

fn redact_channel(mut row: Value) -> Value {
    if let Some(object) = row.as_object_mut()
        && let Some(config) = object.get_mut("config")
        && let Some(config) = config.as_object_mut()
    {
        for key in ["token", "authorization", "secret"] {
            if config.contains_key(key) {
                config.insert(key.to_string(), Value::String("[redacted]".to_string()));
            }
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn severity_thresholds_are_ordered() {
        assert!(severity_rank("critical") > severity_rank("warning"));
        assert!(severity_rank("warning") > severity_rank("notice"));
        assert!(severity_rank("notice") > severity_rank("info"));
    }

    #[test]
    fn channel_validation_requires_provider_specific_fields() {
        let error = validate_channel("ntfy", &json!({"url": "https://ntfy.sh"}))
            .expect_err("ntfy requires a topic");
        assert!(error.to_string().contains("topic"));
        validate_channel(
            "webhook",
            &json!({"url": "https://hooks.example.test/hope"}),
        )
        .expect("webhook URL is valid");
    }

    #[tokio::test]
    async fn webhook_delivery_posts_json_without_following_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4_096];
            let length = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.contains("incident.opened"));
            assert!(request.contains("x-hope-notification-id"));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let delivery = Delivery {
            id: Uuid::new_v4(),
            provider: "webhook".to_string(),
            config: json!({"url": format!("http://{address}/hook")}),
            payload: json!({"event": "incident.opened"}),
            status: "pending".to_string(),
        };

        send_delivery(&delivery).await.unwrap();
        task.await.unwrap();
    }
}
