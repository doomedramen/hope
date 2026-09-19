//! Durable monitor-proposal policy and review API.
//!
//! This module reads only resolved service fields and current canonical
//! endpoints. It persists policy output for M4 and creates a monitor only
//! after an operator approves a proposal; it never probes a target or
//! schedules a check.

use anyhow::{Result, anyhow};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::inventory::pagination::{decode_cursor, effective_limit, encode_cursor};
use crate::monitoring;
use crate::state::AppState;

pub const GENERIC_HTTP_RULE_ID: &str = "monitor.http.generic";
pub const GENERIC_TCP_RULE_ID: &str = "monitor.tcp.generic";
pub const POLICY_RULE_VERSION: i32 = 1;
pub const PRODUCT_AUTO_CREATE_CONFIDENCE: f32 = 0.90;

#[derive(Debug, Clone)]
struct ResolvedEndpoint {
    service_id: Uuid,
    endpoint_id: Uuid,
    protocol: String,
    product: Option<String>,
    product_version: Option<String>,
    endpoint_type: String,
    address: Option<String>,
    port: Option<i32>,
    url: Option<String>,
    dns_name: Option<String>,
    confidence: f32,
    source_evidence_id: Option<Uuid>,
    source_rule_id: Option<String>,
}

type ResolvedEndpointRow = (
    Uuid,
    Uuid,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<Uuid>,
    Option<f32>,
    Option<String>,
);

#[derive(Debug, Clone)]
struct PolicyRule {
    rule_id: String,
    check_type: &'static str,
    check_config: Value,
    auto_create_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct MonitorProposalListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub status: Option<String>,
    pub service_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub service_id: Uuid,
}

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn status_filter(status: Option<&str>) -> Result<Option<&str>, (StatusCode, Json<Value>)> {
    match status.unwrap_or("pending") {
        "all" => Ok(None),
        status @ ("pending" | "approved" | "rejected") => Ok(Some(status)),
        _ => Err(err(
            StatusCode::BAD_REQUEST,
            "status must be pending, approved, rejected, or all",
        )),
    }
}

/// Generate or refresh proposals for one canonical service in a caller-owned
/// transaction. Existing status, decisions, and user overrides never change.
pub async fn generate_for_service_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
) -> Result<Vec<Uuid>> {
    let lock_key = format!("monitor-proposals:{service_id}");
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;

    let service_exists: bool =
        sqlx::query_scalar("select exists(select 1 from services where id = $1)")
            .bind(service_id)
            .fetch_one(&mut **tx)
            .await?;
    if !service_exists {
        return Err(anyhow!("service {service_id} not found"));
    }

    let endpoints = load_resolved_endpoints(tx, service_id).await?;
    let mut proposal_ids = Vec::new();
    for endpoint in endpoints {
        let Some(rule) = policy_rule(
            &endpoint.protocol,
            endpoint.product.as_deref(),
            endpoint.confidence,
        ) else {
            continue;
        };

        let target_identity = format!("endpoint:{}", endpoint.endpoint_id);
        let target = endpoint_target(&endpoint);
        let resolved_from = json!({
            "protocol": endpoint.protocol,
            "product": endpoint.product,
            "product_version": endpoint.product_version,
            "source_evidence_id": endpoint.source_evidence_id,
            "source_rule_id": endpoint.source_rule_id,
        });

        let existing: Option<Uuid> = sqlx::query_scalar(
            "select id from monitor_proposals where rule_id = $1 and endpoint_id = $2",
        )
        .bind(&rule.rule_id)
        .bind(endpoint.endpoint_id)
        .fetch_optional(&mut **tx)
        .await?;

        let proposal_id = if let Some(proposal_id) = existing {
            refresh_proposal_tx(
                tx,
                proposal_id,
                &endpoint,
                &rule,
                &target_identity,
                &target,
                &resolved_from,
            )
            .await?;
            proposal_id
        } else {
            let proposal_id: Uuid = sqlx::query_scalar(
                "insert into monitor_proposals \
                    (service_id, endpoint_id, rule_id, rule_version, target_identity, target, \
                     protocol, product, product_version, check_type, check_config, confidence, \
                     auto_create_allowed, resolved_from, source_evidence_id, source_rule_id) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
                 returning id",
            )
            .bind(endpoint.service_id)
            .bind(endpoint.endpoint_id)
            .bind(&rule.rule_id)
            .bind(POLICY_RULE_VERSION)
            .bind(&target_identity)
            .bind(&target)
            .bind(&endpoint.protocol)
            .bind(&endpoint.product)
            .bind(&endpoint.product_version)
            .bind(rule.check_type)
            .bind(&rule.check_config)
            .bind(endpoint.confidence)
            .bind(rule.auto_create_allowed)
            .bind(&resolved_from)
            .bind(endpoint.source_evidence_id)
            .bind(&endpoint.source_rule_id)
            .fetch_one(&mut **tx)
            .await?;

            let after = proposal_value_tx(tx, proposal_id).await?;
            Recorder::record_change(
                tx,
                "monitor_proposals",
                proposal_id,
                "monitor_proposal.created",
                "notice",
                None,
                Some(after),
                Some("policy"),
            )
            .await?;
            proposal_id
        };
        proposal_ids.push(proposal_id);
    }

    Ok(proposal_ids)
}

/// Generate proposals in a standalone transaction for an authenticated API
/// request or a focused DB test.
pub async fn generate_for_service(pool: &PgPool, service_id: Uuid) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    let proposal_ids = generate_for_service_tx(&mut tx, service_id).await?;
    tx.commit().await?;
    Ok(proposal_ids)
}

async fn load_resolved_endpoints(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
) -> sqlx::Result<Vec<ResolvedEndpoint>> {
    let rows: Vec<ResolvedEndpointRow> = sqlx::query_as(
        "select s.id, e.id, s.protocol, s.product, s.product_version, \
                e.endpoint_type, e.address::text, e.port, e.url, e.dns_name, \
                source.id, source.confidence, source.value->>'rule_id' \
         from services s \
         join endpoints e on e.service_id = s.id and e.is_current \
         left join lateral ( \
             select ev.id, ev.confidence, ev.value \
             from evidence ev \
             where ev.subject_table = 'services' and ev.subject_id = s.id \
               and ev.source_type = 'network_scan' \
               and ev.attribute in ('fingerprint', 'protocol_classification') \
               and not ev.absent \
             order by (ev.confirmed_by is not null) desc, ev.last_seen desc, ev.id desc \
             limit 1 \
         ) source on true \
         where s.id = $1 \
         order by e.created_at, e.id",
    )
    .bind(service_id)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(
                service_id,
                endpoint_id,
                protocol,
                product,
                product_version,
                endpoint_type,
                address,
                port,
                url,
                dns_name,
                source_evidence_id,
                confidence,
                source_rule_id,
            )| {
                Some(ResolvedEndpoint {
                    service_id,
                    endpoint_id,
                    protocol: protocol?.trim().to_ascii_lowercase(),
                    product: product.filter(|value| !value.trim().is_empty()),
                    product_version,
                    endpoint_type,
                    address,
                    port,
                    url,
                    dns_name,
                    confidence: confidence.unwrap_or(1.0),
                    source_evidence_id,
                    source_rule_id,
                })
            },
        )
        .collect())
}

fn policy_rule(protocol: &str, product: Option<&str>, confidence: f32) -> Option<PolicyRule> {
    match protocol {
        "http" | "https" => {
            let rule_id = product
                .filter(|product| !product.trim().is_empty())
                .map(|product| {
                    format!(
                        "monitor.{protocol}.product.{}",
                        stable_product_slug(product)
                    )
                })
                .unwrap_or_else(|| {
                    if protocol == "http" {
                        GENERIC_HTTP_RULE_ID.to_string()
                    } else {
                        format!("monitor.{protocol}.generic")
                    }
                });
            Some(PolicyRule {
                rule_id,
                check_type: "http",
                check_config: json!({
                    "method": "GET",
                    "path": "/",
                    "expected_status": 200,
                    "timeout_ms": 5000,
                }),
                auto_create_allowed: product.is_some()
                    && confidence >= PRODUCT_AUTO_CREATE_CONFIDENCE,
            })
        }
        "tcp" => Some(PolicyRule {
            rule_id: GENERIC_TCP_RULE_ID.to_string(),
            check_type: "tcp",
            check_config: json!({ "timeout_ms": 5000 }),
            auto_create_allowed: false,
        }),
        _ => None,
    }
}

fn stable_product_slug(product: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in product.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

fn endpoint_target(endpoint: &ResolvedEndpoint) -> Value {
    json!({
        "service_id": endpoint.service_id,
        "endpoint_id": endpoint.endpoint_id,
        "endpoint_type": endpoint.endpoint_type,
        "address": endpoint.address,
        "port": endpoint.port,
        "url": endpoint.url,
        "dns_name": endpoint.dns_name,
    })
}

async fn refresh_proposal_tx(
    tx: &mut Transaction<'_, Postgres>,
    proposal_id: Uuid,
    endpoint: &ResolvedEndpoint,
    rule: &PolicyRule,
    target_identity: &str,
    target: &Value,
    resolved_from: &Value,
) -> sqlx::Result<()> {
    let before = proposal_value_tx(tx, proposal_id).await?;
    let updated = sqlx::query(
        "update monitor_proposals set \
            service_id = $2, target_identity = $3, target = $4, protocol = $5, product = $6, \
            product_version = $7, check_type = $8, check_config = $9, confidence = $10, \
            auto_create_allowed = $11, rule_version = $12, resolved_from = $13, \
            source_evidence_id = $14, source_rule_id = $15, version = version + 1, updated_at = now() \
         where id = $1 and ( \
            service_id is distinct from $2 or target_identity is distinct from $3 \
            or target is distinct from $4 or protocol is distinct from $5 \
            or product is distinct from $6 or product_version is distinct from $7 \
            or check_type is distinct from $8 or check_config is distinct from $9 \
            or confidence is distinct from $10 or auto_create_allowed is distinct from $11 \
            or rule_version is distinct from $12 or resolved_from is distinct from $13 \
            or source_evidence_id is distinct from $14 or source_rule_id is distinct from $15 \
         )",
    )
    .bind(proposal_id)
    .bind(endpoint.service_id)
    .bind(target_identity)
    .bind(target)
    .bind(&endpoint.protocol)
    .bind(&endpoint.product)
    .bind(&endpoint.product_version)
    .bind(rule.check_type)
    .bind(&rule.check_config)
    .bind(endpoint.confidence)
    .bind(rule.auto_create_allowed)
    .bind(POLICY_RULE_VERSION)
    .bind(resolved_from)
    .bind(endpoint.source_evidence_id)
    .bind(&endpoint.source_rule_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 1 {
        let after = proposal_value_tx(tx, proposal_id).await?;
        Recorder::record_change(
            tx,
            "monitor_proposals",
            proposal_id,
            "monitor_proposal.refreshed",
            "notice",
            Some(before),
            Some(after),
            Some("policy"),
        )
        .await?;
    }
    Ok(())
}

async fn proposal_value_tx(
    tx: &mut Transaction<'_, Postgres>,
    proposal_id: Uuid,
) -> sqlx::Result<Value> {
    sqlx::query_scalar(
        "select row_to_json(t) from (select * from monitor_proposals where id = $1) t",
    )
    .bind(proposal_id)
    .fetch_one(&mut **tx)
    .await
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<MonitorProposalListParams>,
) -> (StatusCode, Json<Value>) {
    let status = match status_filter(params.status.as_deref()) {
        Ok(status) => status,
        Err(response) => return response,
    };
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);

    let mut query: QueryBuilder<Postgres> = QueryBuilder::new(
        "select row_to_json(t) from (select mp.* from monitor_proposals mp where true",
    );
    if let Some(status) = status {
        query.push(" and mp.status = ").push_bind(status);
    }
    if let Some(service_id) = params.service_id {
        query.push(" and mp.service_id = ").push_bind(service_id);
    }
    if let Some((created_at, id)) = cursor {
        query
            .push(" and (mp.created_at, mp.id) > (")
            .push_bind(created_at)
            .push("::timestamptz, ")
            .push_bind(id)
            .push(")");
    }
    query
        .push(" order by mp.created_at, mp.id limit ")
        .push_bind(limit)
        .push(") t");

    let rows: Result<Vec<(Value,)>, sqlx::Error> =
        query.build_query_as().fetch_all(&state.pool).await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let items: Vec<Value> = rows.into_iter().map(|(value,)| value).collect();
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

pub async fn get(State(state): State<AppState>, Path(id): Path<Uuid>) -> (StatusCode, Json<Value>) {
    match proposal_value_pool(&state.pool, id).await {
        Ok(Some(value)) => (StatusCode::OK, Json(value)),
        Ok(None) => err(StatusCode::NOT_FOUND, "monitor proposal not found"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn proposal_value_pool(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Value>> {
    sqlx::query_scalar(
        "select row_to_json(t) from (select * from monitor_proposals where id = $1) t",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn generate(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Json(request): Json<GenerateRequest>,
) -> (StatusCode, Json<Value>) {
    match generate_for_service(&state.pool, request.service_id).await {
        Ok(proposal_ids) => (
            StatusCode::OK,
            Json(json!({
                "service_id": request.service_id,
                "proposal_ids": proposal_ids,
            })),
        ),
        Err(error) if error.to_string().contains("not found") => {
            err(StatusCode::NOT_FOUND, error.to_string())
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn generate_for_path(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Path(service_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match generate_for_service(&state.pool, service_id).await {
        Ok(proposal_ids) => (
            StatusCode::OK,
            Json(json!({ "service_id": service_id, "proposal_ids": proposal_ids })),
        ),
        Err(error) if error.to_string().contains("not found") => {
            err(StatusCode::NOT_FOUND, error.to_string())
        }
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn approve(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve(&state, user.0, id, "approved").await
}

pub async fn reject(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve(&state, user.0, id, "rejected").await
}

async fn resolve(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    requested_status: &str,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    let current: Result<Option<(Uuid, String)>, sqlx::Error> =
        sqlx::query_as("select service_id, status from monitor_proposals where id = $1 for update")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await;
    let Some((service_id, status)) = (match current {
        Ok(current) => current,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }) else {
        return err(StatusCode::NOT_FOUND, "monitor proposal not found");
    };

    if status != "pending" {
        if status == requested_status {
            if requested_status == "approved"
                && let Err(error) = monitoring::create_from_proposal_tx(&mut tx, id, user_id).await
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
            }
            match proposal_value_tx(&mut tx, id).await {
                Ok(value) => {
                    if let Err(error) = tx.commit().await {
                        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
                    }
                    return (StatusCode::OK, Json(value));
                }
                Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            }
        }
        return err(
            StatusCode::CONFLICT,
            format!("monitor proposal is already {status}"),
        );
    }

    let before = match proposal_value_tx(&mut tx, id).await {
        Ok(value) => value,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Err(error) = sqlx::query(
        "update monitor_proposals set status = $2, decision_source = 'manual', \
            decided_by = $3, decided_at = now(), version = version + 1, updated_at = now() \
         where id = $1",
    )
    .bind(id)
    .bind(requested_status)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if requested_status == "approved"
        && let Err(error) = monitoring::create_from_proposal_tx(&mut tx, id, user_id).await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let after = match proposal_value_tx(&mut tx, id).await {
        Ok(value) => value,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let action = format!("monitor_proposal.{requested_status}");
    if let Err(error) = Recorder::record_change(
        &mut tx,
        "monitor_proposals",
        id,
        &action,
        "notice",
        Some(before),
        Some(after.clone()),
        Some("manual"),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        &action,
        Some("monitor_proposals"),
        Some(id),
        "success",
        Some(json!({
            "service_id": service_id,
            "decision": requested_status,
            "provenance": "manual",
        })),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (StatusCode::OK, Json(after))
}

pub async fn patch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let Value::Object(map) = body else {
        return err(StatusCode::BAD_REQUEST, "body must be a JSON object");
    };
    let Some(expected_version) = map.get("version").and_then(Value::as_i64) else {
        return err(StatusCode::BAD_REQUEST, "version is required");
    };
    let Some(overrides) = map.get("overrides").or_else(|| map.get("user_overrides")) else {
        return err(StatusCode::BAD_REQUEST, "overrides is required");
    };
    let Value::Object(incoming) = overrides else {
        return err(StatusCode::BAD_REQUEST, "overrides must be a JSON object");
    };
    if incoming.is_empty() {
        return err(StatusCode::BAD_REQUEST, "overrides must not be empty");
    }
    if incoming.keys().any(|key| {
        matches!(
            key.as_str(),
            "service_id"
                | "endpoint_id"
                | "rule_id"
                | "target_identity"
                | "target"
                | "protocol"
                | "product"
                | "product_version"
                | "check_type"
        )
    }) {
        return err(
            StatusCode::BAD_REQUEST,
            "overrides cannot change service, endpoint, rule, target, or resolved identity",
        );
    }

    let expected_version = match i32::try_from(expected_version) {
        Ok(version) => version,
        Err(_) => return err(StatusCode::BAD_REQUEST, "version is out of range"),
    };
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let current: Result<Option<(Uuid, i32, Value)>, sqlx::Error> = sqlx::query_as(
        "select service_id, version, user_overrides from monitor_proposals \
         where id = $1 for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await;
    let Some((service_id, current_version, current_overrides)) = (match current {
        Ok(current) => current,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }) else {
        return err(StatusCode::NOT_FOUND, "monitor proposal not found");
    };
    if current_version != expected_version {
        return err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        );
    }

    let mut merged = current_overrides.as_object().cloned().unwrap_or_default();
    merged.extend(incoming.clone());
    let merged = Value::Object(merged);
    if merged == current_overrides {
        let value = match proposal_value_tx(&mut tx, id).await {
            Ok(value) => value,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        if let Err(error) = tx.commit().await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
        return (StatusCode::OK, Json(value));
    }

    let before = match proposal_value_tx(&mut tx, id).await {
        Ok(value) => value,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Err(error) = sqlx::query(
        "update monitor_proposals set user_overrides = $2, override_by = $3, \
            override_at = now(), version = version + 1, updated_at = now() \
         where id = $1 and version = $4",
    )
    .bind(id)
    .bind(&merged)
    .bind(user.0)
    .bind(expected_version)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let after = match proposal_value_tx(&mut tx, id).await {
        Ok(value) => value,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Err(error) = Recorder::record_change(
        &mut tx,
        "monitor_proposals",
        id,
        "monitor_proposal.override_changed",
        "notice",
        Some(before),
        Some(after.clone()),
        Some("manual"),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "monitor_proposal.override_changed",
        Some("monitor_proposals"),
        Some(id),
        "success",
        Some(json!({
            "service_id": service_id,
            "overrides": merged,
            "provenance": "manual",
        })),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (StatusCode::OK, Json(after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    async fn fixture(pool: &PgPool, protocol: &str, product: Option<&str>) -> (Uuid, Uuid) {
        let device_id: Uuid =
            sqlx::query_scalar("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(pool)
                .await
                .expect("create proposal device");
        let service_id: Uuid = sqlx::query_scalar(
            "insert into services (protocol, product, owner_kind, owner_id) \
             values ($1, $2, 'device', $3) returning id",
        )
        .bind(protocol)
        .bind(product)
        .bind(device_id)
        .fetch_one(pool)
        .await
        .expect("create proposal service");
        let endpoint_id: Uuid = sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '192.0.2.120'::inet, 18080) returning id",
        )
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("create proposal endpoint");
        (service_id, endpoint_id)
    }

    async fn proposal_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("proposal-{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("create proposal user")
    }

    #[tokio::test]
    async fn generic_http_and_tcp_proposals_are_current_endpoint_scoped_and_idempotent() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (http_service, http_endpoint) = fixture(&pool, "http", None).await;
        let first = generate_for_service(&pool, http_service)
            .await
            .expect("generate generic HTTP proposal");
        let second = generate_for_service(&pool, http_service)
            .await
            .expect("repeat generic HTTP proposal");
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);

        let row: (String, String, Uuid, String, bool, String) = sqlx::query_as(
            "select rule_id, status, endpoint_id, check_type, auto_create_allowed, target_identity \
             from monitor_proposals where id = $1",
        )
        .bind(first[0])
        .fetch_one(&pool)
        .await
        .expect("read generic HTTP proposal");
        assert_eq!(row.0, GENERIC_HTTP_RULE_ID);
        assert_eq!(row.1, "pending");
        assert_eq!(row.2, http_endpoint);
        assert_eq!(row.3, "http");
        assert!(!row.4);
        assert_eq!(row.5, format!("endpoint:{http_endpoint}"));

        let (tcp_service, _) = fixture(&pool, "tcp", None).await;
        let tcp_ids = generate_for_service(&pool, tcp_service)
            .await
            .expect("generate generic TCP proposal");
        let tcp_rule: String =
            sqlx::query_scalar("select rule_id from monitor_proposals where id = $1")
                .bind(tcp_ids[0])
                .fetch_one(&pool)
                .await
                .expect("read generic TCP proposal");
        assert_eq!(tcp_rule, GENERIC_TCP_RULE_ID);

        let (events,): (i64,) = sqlx::query_as(
            "select count(*) from change_events where entity_kind = 'monitor_proposals' \
             and entity_id = $1 and category = 'monitor_proposal.created'",
        )
        .bind(first[0])
        .fetch_one(&pool)
        .await
        .expect("read proposal change events");
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn approving_proposal_creates_monitor_and_reapproval_keeps_state() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, endpoint_id) = fixture(&pool, "http", None).await;
        let proposal_id = generate_for_service(&pool, service_id)
            .await
            .expect("generate monitor proposal")[0];
        let user_id = proposal_user(&pool).await;
        let state = AppState { pool: pool.clone() };

        let approved = approve(
            State(state.clone()),
            Extension(CurrentUser(user_id)),
            Path(proposal_id),
        )
        .await;
        assert_eq!(approved.0, StatusCode::OK);

        let monitor_id: Uuid = sqlx::query_scalar(
            "select id from monitors where proposal_id = $1 and service_id = $2 \
             and endpoint_id = $3",
        )
        .bind(proposal_id)
        .bind(service_id)
        .bind(endpoint_id)
        .fetch_one(&pool)
        .await
        .expect("approval creates monitor");
        sqlx::query(
            "update monitors set state = 'down', consecutive_failures = 4 \
             where id = $1",
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .expect("set monitor state for retry");

        let reapproved = approve(
            State(state),
            Extension(CurrentUser(user_id)),
            Path(proposal_id),
        )
        .await;
        assert_eq!(reapproved.0, StatusCode::OK);

        let (monitor_count, state, failures): (i64, String, i32) = sqlx::query_as(
            "select count(*), max(state), max(consecutive_failures)::integer \
             from monitors where proposal_id = $1",
        )
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .expect("read idempotent monitor");
        assert_eq!(monitor_count, 1);
        assert_eq!(state, "down");
        assert_eq!(failures, 4);

        let change_count: i64 = sqlx::query_scalar(
            "select count(*) from change_events where entity_kind = 'monitors' \
             and entity_id = $1 and category = 'monitor.created'",
        )
        .bind(monitor_id)
        .fetch_one(&pool)
        .await
        .expect("read monitor creation event");
        assert_eq!(change_count, 1);
    }

    #[tokio::test]
    async fn product_proposal_policy_and_user_decision_survive_refresh() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = fixture(&pool, "http", Some("Grafana")).await;
        let ids = generate_for_service(&pool, service_id)
            .await
            .expect("generate product proposal");
        assert_eq!(ids.len(), 1);
        let user_id = proposal_user(&pool).await;

        let response = reject(
            State(AppState { pool: pool.clone() }),
            Extension(CurrentUser(user_id)),
            Path(ids[0]),
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);

        let version: i32 =
            sqlx::query_scalar("select version from monitor_proposals where id = $1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .expect("read proposal version");
        let body = json!({ "version": version, "overrides": { "path": "/health" } });
        let patched = patch(
            State(AppState { pool: pool.clone() }),
            Extension(CurrentUser(user_id)),
            Path(ids[0]),
            Json(body),
        )
        .await;
        assert_eq!(patched.0, StatusCode::OK);

        generate_for_service(&pool, service_id)
            .await
            .expect("refresh product proposal");
        let row: (String, String, Uuid, Value, Uuid, String) = sqlx::query_as(
            "select status, decision_source, decided_by, user_overrides, override_by, rule_id \
             from monitor_proposals where id = $1",
        )
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .expect("read preserved proposal");
        assert_eq!(row.0, "rejected");
        assert_eq!(row.1, "manual");
        assert_eq!(row.2, user_id);
        assert_eq!(row.3, json!({ "path": "/health" }));
        assert_eq!(row.4, user_id);
        assert_eq!(row.5, "monitor.http.product.grafana");

        let (audit_count, change_count): (i64, i64) = sqlx::query_as(
            "select \
                (select count(*) from audit_events where target_kind = 'monitor_proposals' \
                    and target_id = $1 and action = 'monitor_proposal.rejected'), \
                (select count(*) from change_events where entity_kind = 'monitor_proposals' \
                    and entity_id = $1 and category = 'monitor_proposal.override_changed')",
        )
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .expect("read proposal audit records");
        assert_eq!(audit_count, 1);
        assert_eq!(change_count, 1);
    }

    #[tokio::test]
    async fn unsupported_protocol_and_historical_endpoint_create_no_proposal() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, endpoint_id) = fixture(&pool, "ssh", None).await;
        let ids = generate_for_service(&pool, service_id)
            .await
            .expect("ignore unsupported protocol");
        assert!(ids.is_empty());

        sqlx::query("update endpoints set is_current = false where id = $1")
            .bind(endpoint_id)
            .execute(&pool)
            .await
            .expect("retire endpoint");
        sqlx::query("update services set protocol = 'tcp' where id = $1")
            .bind(service_id)
            .execute(&pool)
            .await
            .expect("resolve service protocol");
        let ids = generate_for_service(&pool, service_id)
            .await
            .expect("skip service without current endpoint");
        assert!(ids.is_empty());
    }
}
