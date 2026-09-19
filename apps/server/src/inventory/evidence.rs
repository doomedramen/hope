//! Evidence: the one polymorphic table backing every inferred fact
//! (design §2). Rows are never mutated/deleted -- absence and expiry are
//! new rows / a confidence downgrade at read time, never a destructive
//! write. `resolve_attribute` is the read-time resolution function
//! (design Decision 7, "manual outranks inferred").

use axum::Extension;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

#[derive(Debug, Deserialize)]
pub struct SubmitEvidence {
    pub subject_table: String,
    pub subject_id: Uuid,
    pub source_type: String,
    pub source_instance: Option<String>,
    pub attribute: String,
    pub value: Value,
    pub confidence: f32,
    #[serde(default)]
    pub absent: bool,
    pub expires_at: Option<String>,
}

/// POST /api/v1/evidence — append a new evidence row. Manual submissions
/// (`source_type = "manual"`) are stamped `confirmed_by` = the acting
/// user, giving them precedence at read time over any conflicting
/// automatic evidence, including evidence that arrives *later* (design
/// Decision 7: "never overwritten by conflicting automatic evidence
/// unless the user re-confirms").
pub async fn submit(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<SubmitEvidence>,
) -> (StatusCode, Json<Value>) {
    let confirmed_by = (req.source_type == "manual").then_some(user.0);

    let row: Result<(Value,), sqlx::Error> = sqlx::query_as(
        "insert into evidence \
            (subject_table, subject_id, source_type, source_instance, attribute, value, \
             confidence, absent, confirmed_by, expires_at) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::timestamptz) \
         returning row_to_json(evidence.*)",
    )
    .bind(&req.subject_table)
    .bind(req.subject_id)
    .bind(&req.source_type)
    .bind(&req.source_instance)
    .bind(&req.attribute)
    .bind(&req.value)
    .bind(req.confidence)
    .bind(req.absent)
    .bind(confirmed_by)
    .bind(&req.expires_at)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok((v,)) => (StatusCode::CREATED, Json(v)),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct EvidenceQuery {
    pub subject_table: String,
    pub subject_id: Uuid,
    pub attribute: Option<String>,
}

/// GET /api/v1/evidence?subject_table=&subject_id= — the literal "Why?"
/// query (design §2: "always `select * from evidence where
/// subject_table = $1 and subject_id = $2 order by last_seen desc`").
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<EvidenceQuery>,
) -> (StatusCode, Json<Value>) {
    let result = sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from (select * from evidence \
            where subject_table = $1 and subject_id = $2 \
              and ($3::text is null or attribute = $3) \
            order by last_seen desc limit 200) t",
    )
    .bind(&q.subject_table)
    .bind(q.subject_id)
    .bind(&q.attribute)
    .fetch_all(&state.pool)
    .await;

    match result {
        Ok(rows) => {
            let items: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
            (StatusCode::OK, Json(json!({ "items": items })))
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Read-time resolution for one (subject, attribute) pair (design
/// Decision 7): a `confirmed_by` row always wins, regardless of
/// confidence or recency, over any non-confirmed row -- expiry/absence of
/// the automatic evidence never displaces it. Absent among non-confirmed
/// candidates, the highest-confidence non-absent, non-expired row wins.
pub async fn resolve_attribute(
    pool: &PgPool,
    subject_table: &str,
    subject_id: Uuid,
    attribute: &str,
) -> sqlx::Result<Option<Value>> {
    let confirmed: Option<(Value,)> = sqlx::query_as(
        "select value from evidence \
         where subject_table = $1 and subject_id = $2 and attribute = $3 \
           and confirmed_by is not null and not absent \
         order by last_seen desc limit 1",
    )
    .bind(subject_table)
    .bind(subject_id)
    .bind(attribute)
    .fetch_optional(pool)
    .await?;

    if let Some((v,)) = confirmed {
        return Ok(Some(v));
    }

    let best: Option<(Value,)> = sqlx::query_as(
        "select value from evidence \
         where subject_table = $1 and subject_id = $2 and attribute = $3 \
           and not absent and (expires_at is null or expires_at > now()) \
         order by confidence desc, last_seen desc limit 1",
    )
    .bind(subject_table)
    .bind(subject_id)
    .bind(attribute)
    .fetch_optional(pool)
    .await?;

    Ok(best.map(|(v,)| v))
}

pub async fn resolve_handler(
    State(state): State<AppState>,
    Query(q): Query<EvidenceQuery>,
) -> (StatusCode, Json<Value>) {
    let Some(attribute) = &q.attribute else {
        return err(StatusCode::BAD_REQUEST, "attribute is required");
    };
    match resolve_attribute(&state.pool, &q.subject_table, q.subject_id, attribute).await {
        Ok(v) => (StatusCode::OK, Json(json!({ "value": v }))),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod gate_tests {
    //! Acceptance gate (spec §17 M1, design §7): "manual facts survive
    //! expiry of automatic evidence."

    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn confirmed_value_survives_automatic_evidence_expiry_and_absence() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let (device_id,): (Uuid,) = sqlx::query_as(
            "insert into devices (device_type) values ('physical_host') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let user_id: (Uuid,) = sqlx::query_as(
            "insert into users (email, password_hash) values ($1, 'x') returning id",
        )
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();

        // Automatic evidence first.
        sqlx::query(
            "insert into evidence (subject_table, subject_id, source_type, attribute, value, confidence, \
                first_seen, last_seen) \
             values ('devices', $1, 'agent', 'hostname', '\"auto-name\"'::jsonb, 0.6, now(), now())",
        )
        .bind(device_id)
        .execute(&pool)
        .await
        .unwrap();

        // Operator manually confirms a different value.
        sqlx::query(
            "insert into evidence (subject_table, subject_id, source_type, attribute, value, confidence, \
                first_seen, last_seen, confirmed_by) \
             values ('devices', $1, 'manual', 'hostname', '\"confirmed-name\"'::jsonb, 1.0, now(), now(), $2)",
        )
        .bind(device_id)
        .bind(user_id.0)
        .execute(&pool)
        .await
        .unwrap();

        let resolved = resolve_attribute(&pool, "devices", device_id, "hostname")
            .await
            .unwrap();
        assert_eq!(resolved, Some(json!("confirmed-name")));

        // The automatic evidence now expires AND a newer, higher-confidence
        // automatic row reports it absent -- neither displaces the manual
        // confirmation.
        sqlx::query(
            "update evidence set expires_at = now() - interval '1 second' \
             where subject_id = $1 and source_type = 'agent'",
        )
        .bind(device_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into evidence (subject_table, subject_id, source_type, attribute, value, confidence, \
                first_seen, last_seen, absent) \
             values ('devices', $1, 'agent', 'hostname', 'null'::jsonb, 0.9, now(), now(), true)",
        )
        .bind(device_id)
        .execute(&pool)
        .await
        .unwrap();

        let resolved_after = resolve_attribute(&pool, "devices", device_id, "hostname")
            .await
            .unwrap();
        assert_eq!(
            resolved_after,
            Some(json!("confirmed-name")),
            "manual confirmation still wins after automatic evidence expired/went absent"
        );
    }
}
