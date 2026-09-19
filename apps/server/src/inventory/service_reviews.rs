//! Durable review queue for ambiguous service fingerprints.
//!
//! Product candidates below PRODUCT_AUTO_APPLY_CONFIDENCE and candidates
//! that conflict with an existing non-manual product are retained here. The
//! queue stores the complete candidate/rule/evidence explanation, so review
//! actions never need to re-run a scan or a fingerprint rule.

use std::collections::HashMap;

use anyhow::{Context, Result};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use domain::fingerprinting::FingerprintCandidate;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::pagination::{decode_cursor, effective_limit, encode_cursor};
use crate::inventory::{events::Recorder, monitor_proposals};
use crate::state::AppState;

/// Product candidates at or above this confidence may be projected
/// automatically when no conflict exists. The domain engine emits 0.86 for
/// a two-field product match, so those candidates enter the review queue.
pub const PRODUCT_AUTO_APPLY_CONFIDENCE: f32 = 0.90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewReason {
    LowConfidence,
    Conflict,
}

impl ReviewReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowConfidence => "low_confidence",
            Self::Conflict => "conflict",
        }
    }
}

/// Append one review item unless this exact candidate was already observed
/// for this service. Candidate identity excludes scan instance and evidence
/// IDs, which suppresses unchanged candidates after rejection or confirmation.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_review_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    fingerprint_evidence_id: Uuid,
    source_instance: Option<&str>,
    candidate: &FingerprintCandidate,
    candidates: &[FingerprintCandidate],
    m2_evidence: &Value,
    fingerprint_evidence: &Value,
    reason: ReviewReason,
) -> Result<Option<Uuid>> {
    let mut candidate_value =
        serde_json::to_value(candidate).context("serialize service review candidate")?;
    candidate_value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("service review candidate is not an object"))?
        .insert("product_version".to_string(), json!(candidate.version));
    let candidates_value =
        serde_json::to_value(candidates).context("serialize service review candidates")?;
    let evidence_value = json!({
        "m2": m2_evidence,
        "fingerprint": fingerprint_evidence,
    });
    let candidate_key = stable_candidate_key(&candidate_value)?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "select id from service_review_items \
         where service_id = $1 and candidate_key = $2 \
         limit 1",
    )
    .bind(service_id)
    .bind(&candidate_key)
    .fetch_optional(&mut **tx)
    .await?;
    if existing.is_some() {
        return Ok(existing);
    }

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into service_review_items \
            (service_id, fingerprint_evidence_id, source_instance, candidate_key, \
             candidate, candidates, evidence, rule_id, fixture_version, confidence, \
             reason, policy_threshold) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         on conflict (service_id, candidate_key) do nothing \
         returning id",
    )
    .bind(service_id)
    .bind(fingerprint_evidence_id)
    .bind(source_instance)
    .bind(&candidate_key)
    .bind(&candidate_value)
    .bind(&candidates_value)
    .bind(&evidence_value)
    .bind(&candidate.rule_id)
    .bind(i32::try_from(candidate.fixture_version).context("fixture version exceeds i32")?)
    .bind(candidate.confidence)
    .bind(reason.as_str())
    .bind(PRODUCT_AUTO_APPLY_CONFIDENCE)
    .fetch_optional(&mut **tx)
    .await?;

    if inserted.is_some() {
        Ok(inserted)
    } else {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "select id from service_review_items \
             where service_id = $1 and candidate_key = $2 \
             limit 1",
        )
        .bind(service_id)
        .bind(candidate_key)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(existing)
    }
}

fn stable_candidate_key(candidate: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(candidate).context("canonicalize service review candidate")?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[derive(Debug, Deserialize)]
pub struct ServiceReviewListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub status: Option<String>,
}

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn review_status(status: Option<&str>) -> Result<&str, (StatusCode, Json<Value>)> {
    let status = status.unwrap_or("pending");
    if matches!(status, "pending" | "confirmed" | "rejected") {
        Ok(status)
    } else {
        Err(err(
            StatusCode::BAD_REQUEST,
            "status must be pending, confirmed, or rejected",
        ))
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ServiceReviewListParams>,
) -> (StatusCode, Json<Value>) {
    let status = match review_status(params.status.as_deref()) {
        Ok(status) => status,
        Err(response) => return response,
    };
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);

    let rows = if let Some((created_at, id)) = cursor {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from service_review_items \
             where status = $1 and (created_at, id) > ($2::timestamptz, $3) \
             order by created_at, id limit $4) t",
        )
        .bind(status)
        .bind(created_at)
        .bind(id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from service_review_items \
             where status = $1 order by created_at, id limit $2) t",
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    };

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
    let row: Result<Option<(Value,)>, sqlx::Error> = sqlx::query_as(
        "select row_to_json(t) from (select * from service_review_items where id = $1) t",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;

    match row {
        Ok(Some((value,))) => (StatusCode::OK, Json(value)),
        Ok(None) => err(StatusCode::NOT_FOUND, "service review item not found"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn confirm(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve(&state, user.0, id, true).await
}

pub async fn reject(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve(&state, user.0, id, false).await
}

async fn resolve(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    confirm: bool,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    let item: Option<(Uuid, String, Uuid, Value)> = match sqlx::query_as(
        "select service_id, status, fingerprint_evidence_id, candidate \
         from service_review_items where id = $1 for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(item) => item,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    let Some((service_id, status, fingerprint_evidence_id, candidate_value)) = item else {
        return err(StatusCode::NOT_FOUND, "service review item not found");
    };
    let requested_status = if confirm { "confirmed" } else { "rejected" };
    if status != "pending" {
        if status == requested_status {
            return (
                StatusCode::OK,
                Json(json!({
                    "id": id,
                    "service_id": service_id,
                    "status": status,
                })),
            );
        }
        return err(
            StatusCode::CONFLICT,
            format!("service review item is already {status}"),
        );
    }

    if !confirm {
        if let Err(error) = sqlx::query(
            "update service_review_items set status = 'rejected', resolved_by = $2, \
             resolved_at = now(), updated_at = now() where id = $1",
        )
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
        if let Err(error) = Recorder::record_audit(
            &mut tx,
            Some(user_id),
            "operator",
            "service_review.reject",
            Some("service_review_items"),
            Some(id),
            "success",
            Some(json!({ "service_id": service_id, "decision": "rejected" })),
        )
        .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
        if let Err(error) = tx.commit().await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
        return (
            StatusCode::OK,
            Json(json!({
                "id": id,
                "service_id": service_id,
                "status": "rejected",
            })),
        );
    }

    let candidate: FingerprintCandidate = match serde_json::from_value(candidate_value.clone()) {
        Ok(candidate) => candidate,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    // Keep confirmation serialized with worker reconciliation. This also
    // ensures manual-value checks and projection happen in one transaction.
    let lock_key = format!("service-fingerprint:{service_id}");
    if let Err(error) = sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut *tx)
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    type ServiceState = (Option<String>, Option<String>, Option<String>);
    let current: Option<ServiceState> = match sqlx::query_as(
        "select product, product_version, protocol from services where id = $1 for update",
    )
    .bind(service_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(current) => current,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((current_product, current_version, current_protocol)) = current else {
        return err(StatusCode::NOT_FOUND, "service for review item not found");
    };

    let manual_rows: Vec<(String, Value)> = match sqlx::query_as(
        "select attribute, value from evidence \
         where subject_table = 'services' and subject_id = $1 \
           and source_type = 'manual' and confirmed_by is not null and not absent \
           and attribute in ('fingerprint', 'product', 'product_version', 'protocol') \
         order by created_at desc, id desc",
    )
    .bind(service_id)
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let manual_values = manual_field_values(manual_rows);

    let requested_fields = candidate_fields(&candidate);
    for (field, requested) in &requested_fields {
        if let Some(manual) = manual_values.get(*field) {
            let current =
                current_field(field, &current_product, &current_version, &current_protocol);
            if current != Some(manual.as_str()) || manual != requested {
                return err(
                    StatusCode::CONFLICT,
                    format!("service has a different confirmed manual {field} value"),
                );
            }
        }
    }

    let updates: Vec<ProjectionUpdate<'_>> = requested_fields
        .iter()
        .filter_map(|(field, requested)| {
            let before =
                current_field(field, &current_product, &current_version, &current_protocol)
                    .map(str::to_string);
            (before.as_deref() != Some(requested.as_str())).then_some(ProjectionUpdate {
                field,
                before,
                after: requested,
            })
        })
        .collect();

    if !updates.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new("update services set ");
        for (index, update) in updates.iter().enumerate() {
            if index > 0 {
                query.push(", ");
            }
            query.push(update.field).push(" = ").push_bind(update.after);
        }
        query
            .push(", version = version + 1, updated_at = now() where id = ")
            .push_bind(service_id);
        if let Err(error) = query.build().execute(&mut *tx).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }

    let mut manual_value = match serde_json::to_value(&candidate) {
        Ok(value) => value,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some(manual_object) = manual_value.as_object_mut() else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "fingerprint candidate is not a JSON object",
        );
    };
    manual_object.insert("product_version".to_string(), json!(candidate.version));
    manual_object.insert("review_id".to_string(), json!(id));
    manual_object.insert(
        "fingerprint_evidence_id".to_string(),
        json!(fingerprint_evidence_id),
    );
    if let Err(error) = sqlx::query(
        "insert into evidence \
            (subject_table, subject_id, source_type, source_instance, attribute, value, \
             confidence, confirmed_by) \
         values ('services', $1, 'manual', $2, 'fingerprint', $3, 1.0, $4)",
    )
    .bind(service_id)
    .bind(id.to_string())
    .bind(&manual_value)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    for (attribute, value) in &requested_fields {
        if let Err(error) = sqlx::query(
            "insert into evidence \
                (subject_table, subject_id, source_type, source_instance, attribute, value, \
                 confidence, confirmed_by) \
             values ('services', $1, 'manual', $2, $3, $4, 1.0, $5)",
        )
        .bind(service_id)
        .bind(id.to_string())
        .bind(attribute)
        .bind(json!(value))
        .bind(user_id)
        .execute(&mut *tx)
        .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }

    if !updates.is_empty() {
        let before = projection_snapshot(&updates, false);
        let after = projection_snapshot(&updates, true);
        if let Err(error) = Recorder::record_change(
            &mut tx,
            "services",
            service_id,
            "service.fingerprint_changed",
            "notice",
            Some(before),
            Some(after),
            Some("manual"),
        )
        .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }

    // A confirmed product/protocol is a resolved service state. Refresh
    // durable policy output without executing or creating any monitor.
    if let Err(error) = monitor_proposals::generate_for_service_tx(&mut tx, service_id).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    if let Err(error) = sqlx::query(
        "update service_review_items set status = 'confirmed', resolved_by = $2, \
         resolved_at = now(), updated_at = now() where id = $1",
    )
    .bind(id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        "service_review.confirm",
        Some("service_review_items"),
        Some(id),
        "success",
        Some(json!({
            "service_id": service_id,
            "decision": "confirmed",
            "candidate": candidate,
            "changed_fields": updates.iter().map(|update| update.field).collect::<Vec<_>>(),
        })),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "service_id": service_id,
            "status": "confirmed",
            "changed_fields": updates.iter().map(|update| update.field).collect::<Vec<_>>(),
        })),
    )
}

struct ProjectionUpdate<'a> {
    field: &'static str,
    before: Option<String>,
    after: &'a String,
}

fn candidate_fields(candidate: &FingerprintCandidate) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    if let Some(product) = &candidate.product {
        fields.push(("product", product.clone()));
    }
    if let Some(version) = &candidate.version {
        fields.push(("product_version", version.clone()));
    }
    fields.push(("protocol", candidate.protocol.as_str().to_string()));
    fields
}

fn current_field<'a>(
    field: &str,
    product: &'a Option<String>,
    version: &'a Option<String>,
    protocol: &'a Option<String>,
) -> Option<&'a str> {
    match field {
        "product" => product.as_deref(),
        "product_version" => version.as_deref(),
        "protocol" => protocol.as_deref(),
        _ => None,
    }
}

fn manual_field_values(rows: Vec<(String, Value)>) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for (attribute, value) in rows {
        if attribute == "fingerprint" {
            for field in ["product", "product_version", "version", "protocol"] {
                if let Some(value) = value.get(field).and_then(Value::as_str) {
                    let field = if field == "version" {
                        "product_version"
                    } else {
                        field
                    };
                    fields
                        .entry(field.to_string())
                        .or_insert_with(|| value.to_string());
                }
            }
        } else if let Some(value) = value.as_str() {
            fields.entry(attribute).or_insert_with(|| value.to_string());
        }
    }
    fields
}

fn projection_snapshot(updates: &[ProjectionUpdate<'_>], after: bool) -> Value {
    let mut object = Map::new();
    for update in updates {
        let value = if after {
            Value::String(update.after.clone())
        } else {
            update
                .before
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null)
        };
        object.insert(update.field.to_string(), value);
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::fingerprinting::reconcile_service_fingerprint;
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

    async fn fixture(pool: &PgPool, product: Option<&str>) -> (Uuid, Uuid) {
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(pool)
                .await
                .expect("create review device");
        let (service_id,): (Uuid,) = sqlx::query_as(
            "insert into services (protocol, product, owner_kind, owner_id) \
             values ('http', $1, 'device', $2) returning id",
        )
        .bind(product)
        .bind(device_id)
        .fetch_one(pool)
        .await
        .expect("create review service");
        let (endpoint_id,): (Uuid,) = sqlx::query_as(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '192.0.2.80'::inet, 18080) returning id",
        )
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("create review endpoint");
        (service_id, endpoint_id)
    }

    async fn insert_classification(pool: &PgPool, service_id: Uuid, source: &str, value: Value) {
        sqlx::query(
            "insert into evidence \
             (subject_table, subject_id, source_type, source_instance, attribute, value, confidence) \
             values ('services', $1, 'network_scan', $2, 'protocol_classification', $3, 0.98)",
        )
        .bind(service_id)
        .bind(source)
        .bind(value)
        .execute(pool)
        .await
        .expect("insert review classification");
    }

    async fn review_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("service-review-{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("create review user")
    }

    #[tokio::test]
    async fn low_confidence_candidate_is_durable_and_deduplicated() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = fixture(&pool, None).await;
        let classification = json!({
            "protocol": "http",
            "status": 200,
            "headers": {"server": "Plex Media Server"},
            "title": "Plex"
        });
        insert_classification(&pool, service_id, "review-one", classification.clone()).await;
        insert_classification(&pool, service_id, "review-two", classification).await;

        let first = reconcile_service_fingerprint(&pool, service_id, Some("review-one"))
            .await
            .expect("reconcile first review signal")
            .expect("first signal exists");
        let second = reconcile_service_fingerprint(&pool, service_id, Some("review-two"))
            .await
            .expect("reconcile repeated review signal")
            .expect("second signal exists");
        assert_eq!(first.candidate.confidence, 0.86);
        assert!(first.review_id.is_some());
        assert_eq!(first.review_id, second.review_id);
        assert!(first.changed_fields.is_empty());

        let (count,): (i64,) =
            sqlx::query_as("select count(*) from service_review_items where service_id = $1")
                .bind(service_id)
                .fetch_one(&pool)
                .await
                .expect("read low-confidence review");
        assert_eq!(count, 1);
        let review: (String, String, Value, Value) = sqlx::query_as(
            "select reason, rule_id, candidate, evidence from service_review_items \
             where service_id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read low-confidence review details");
        assert_eq!(review.0, "low_confidence");
        assert_eq!(review.1, "http.plex");
        assert_eq!(review.2["product"], json!("Plex"));
        assert_eq!(review.3["m2"]["protocol"], json!("http"));
    }

    #[tokio::test]
    async fn confirm_applies_candidate_and_writes_manual_evidence() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = fixture(&pool, None).await;
        let user_id = review_user(&pool).await;
        insert_classification(
            &pool,
            service_id,
            "confirm-one",
            json!({
                "protocol": "http",
                "status": 200,
                "headers": {"server": "Plex Media Server"},
                "title": "Plex"
            }),
        )
        .await;
        let outcome = reconcile_service_fingerprint(&pool, service_id, Some("confirm-one"))
            .await
            .expect("create confirm review")
            .expect("confirmation signal exists");
        let review_id = outcome.review_id.expect("review created");

        let response = resolve(&AppState { pool: pool.clone() }, user_id, review_id, true).await;
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(response.1["status"], json!("confirmed"));

        let service: (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as("select product, product_version, protocol from services where id = $1")
                .bind(service_id)
                .fetch_one(&pool)
                .await
                .expect("read confirmed service");
        assert_eq!(service.0.as_deref(), Some("Plex"));
        assert_eq!(service.1, None);
        assert_eq!(service.2.as_deref(), Some("http"));

        let manual: (i64, Option<String>) = sqlx::query_as(
            "select count(*), max(value->>'product') from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'manual' and attribute = 'fingerprint'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read confirmed fingerprint evidence");
        assert_eq!(manual, (1, Some("Plex".to_string())));
        let confirmed_by: Uuid = sqlx::query_scalar(
            "select confirmed_by from evidence where subject_table = 'services' \
             and subject_id = $1 and source_type = 'manual' and attribute = 'fingerprint'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read confirming user");
        assert_eq!(confirmed_by, user_id);

        let repeat = resolve(&AppState { pool: pool.clone() }, user_id, review_id, true).await;
        assert_eq!(repeat.0, StatusCode::OK);
        let (manual_count,): (i64,) = sqlx::query_as(
            "select count(*) from evidence where subject_table = 'services' \
             and subject_id = $1 and source_type = 'manual' and attribute = 'fingerprint'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("count repeated manual evidence");
        assert_eq!(manual_count, 1);

        insert_classification(
            &pool,
            service_id,
            "confirm-conflict",
            json!({
                "protocol": "http",
                "status": 200,
                "headers": {"server": "Jellyfin"},
                "title": "Jellyfin",
                "body_sample": "Jellyfin"
            }),
        )
        .await;
        let later = reconcile_service_fingerprint(&pool, service_id, Some("confirm-conflict"))
            .await
            .expect("reconcile after manual confirmation")
            .expect("later signal exists");
        assert!(later.review_id.is_none());
        let (product,): (Option<String>,) =
            sqlx::query_as("select product from services where id = $1")
                .bind(service_id)
                .fetch_one(&pool)
                .await
                .expect("read protected confirmed product");
        assert_eq!(product.as_deref(), Some("Plex"));
    }

    #[tokio::test]
    async fn rejection_suppresses_unchanged_candidate_on_later_scan() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = fixture(&pool, None).await;
        let user_id = review_user(&pool).await;
        let classification = json!({
            "protocol": "http",
            "status": 200,
            "headers": {"server": "Plex Media Server"},
            "title": "Plex"
        });
        insert_classification(&pool, service_id, "reject-one", classification.clone()).await;
        let first = reconcile_service_fingerprint(&pool, service_id, Some("reject-one"))
            .await
            .expect("create rejection review")
            .expect("rejection signal exists");
        let review_id = first.review_id.expect("review created");
        let response = resolve(&AppState { pool: pool.clone() }, user_id, review_id, false).await;
        assert_eq!(response.0, StatusCode::OK);

        insert_classification(&pool, service_id, "reject-two", classification).await;
        let second = reconcile_service_fingerprint(&pool, service_id, Some("reject-two"))
            .await
            .expect("reconcile rejected candidate")
            .expect("later signal exists");
        assert_eq!(second.review_id, Some(review_id));
        let (count, status): (i64, String) = sqlx::query_as(
            "select count(*), max(status) from service_review_items where service_id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read rejected review");
        assert_eq!((count, status), (1, "rejected".to_string()));
    }

    #[tokio::test]
    async fn product_conflict_enters_review_without_overwrite() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = fixture(&pool, Some("Existing Product")).await;
        insert_classification(
            &pool,
            service_id,
            "conflict-one",
            json!({
                "protocol": "http",
                "status": 200,
                "headers": {"server": "Plex Media Server/1.32.5"},
                "title": "Plex",
                "body_sample": "Plex Media Server"
            }),
        )
        .await;
        let outcome = reconcile_service_fingerprint(&pool, service_id, Some("conflict-one"))
            .await
            .expect("reconcile product conflict")
            .expect("conflict signal exists");
        assert!(outcome.review_id.is_some());
        let (product, reason): (Option<String>, String) = sqlx::query_as(
            "select s.product, r.reason from services s \
             join service_review_items r on r.service_id = s.id where s.id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read product conflict");
        assert_eq!(product.as_deref(), Some("Existing Product"));
        assert_eq!(reason, "conflict");
    }
}
