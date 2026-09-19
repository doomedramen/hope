//! Global change feed and (session-authenticated, no separate public API
//! per design §5) audit log reads. Both cursor-paginate on `occurred_at`
//! rather than `created_at` (neither table has the latter).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::inventory::pagination::effective_limit;
use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

fn decode_occurred_cursor(cursor: &Option<String>) -> Option<(String, Uuid)> {
    let raw = cursor.as_ref()?;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (occurred_at, id) = text.split_once('|')?;
    Some((occurred_at.to_string(), Uuid::parse_str(id).ok()?))
}

fn encode_occurred_cursor(occurred_at: &str, id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(format!("{occurred_at}|{id}"))
}

#[derive(Debug, Deserialize)]
pub struct ChangesQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub category: Option<String>,
    pub severity: Option<String>,
}

/// GET /api/v1/changes — global change feed, cursor-paginated, optional
/// category/severity filter (design §6, §11).
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ChangesQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(q.limit);
    let cursor = decode_occurred_cursor(&q.cursor);

    let result = sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from (select * from change_events \
            where ($1::text is null or category = $1) \
              and ($2::text is null or severity = $2) \
              and ($3::timestamptz is null or (occurred_at, id) > ($3::timestamptz, $4)) \
            order by occurred_at desc, id desc limit $5) t",
    )
    .bind(&q.category)
    .bind(&q.severity)
    .bind(cursor.as_ref().map(|(c, _)| c.clone()))
    .bind(cursor.as_ref().map(|(_, id)| *id))
    .bind(limit)
    .fetch_all(&state.pool)
    .await;

    let rows = match result {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let items: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
    let next_cursor = items.last().and_then(|last| {
        let occurred_at = last.get("occurred_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_occurred_cursor(
            occurred_at,
            Uuid::parse_str(id).ok()?,
        ))
    });

    (
        StatusCode::OK,
        Json(json!({ "items": items, "next_cursor": next_cursor })),
    )
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/v1/audit — session-authenticated only, not documented as a
/// stable public contract (design §5: "no public audit API in M1").
pub async fn list_audit(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(q.limit);
    let cursor = decode_occurred_cursor(&q.cursor);

    let result = sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from (select * from audit_events \
            where ($1::timestamptz is null or (occurred_at, id) > ($1::timestamptz, $2)) \
            order by occurred_at desc, id desc limit $3) t",
    )
    .bind(cursor.as_ref().map(|(c, _)| c.clone()))
    .bind(cursor.as_ref().map(|(_, id)| *id))
    .bind(limit)
    .fetch_all(&state.pool)
    .await;

    let rows = match result {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let items: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
    let next_cursor = items.last().and_then(|last| {
        let occurred_at = last.get("occurred_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_occurred_cursor(
            occurred_at,
            Uuid::parse_str(id).ok()?,
        ))
    });

    (
        StatusCode::OK,
        Json(json!({ "items": items, "next_cursor": next_cursor })),
    )
}
