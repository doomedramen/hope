//! Device CRUD, aggregated (merge-redirect-aware) reads, and the
//! merge/split/undo-merge actions (design §4). Mirrors the invariant
//! proptested in `domain::inventory::merge`: merging never re-points or
//! deletes an `interfaces`/`evidence` row, only flips the absorbed
//! device's `canonical_of`/`status`, so undo is a metadata-only flip.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::inventory::pagination::{ListParams, decode_cursor, effective_limit, encode_cursor};
use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

/// Follow `canonical_of` to the survivor id (§4's `resolve_device`).
pub async fn resolve_device(pool: &PgPool, id: Uuid) -> sqlx::Result<Uuid> {
    let mut current = id;
    loop {
        let next: Option<(Option<Uuid>,)> =
            sqlx::query_as("select canonical_of from devices where id = $1")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        match next {
            Some((Some(canonical_of),)) if canonical_of != current => current = canonical_of,
            _ => return Ok(current),
        }
    }
}

/// Every device id whose merge-redirect chain resolves to `survivor`
/// (i.e. `survivor` itself plus every device merged into it, directly or
/// transitively).
async fn aggregate_member_ids(pool: &PgPool, survivor: Uuid) -> sqlx::Result<Vec<Uuid>> {
    // Bounded by device count; M1 has no expectation of deep merge chains,
    // so a recursive CTE keeps this correct without a hand-rolled loop.
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "with recursive members(id) as ( \
            select $1::uuid \
            union all \
            select d.id from devices d join members m on d.canonical_of = m.id \
         ) select id from members",
    )
    .bind(survivor)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn get(State(state): State<AppState>, Path(id): Path<Uuid>) -> (StatusCode, Json<Value>) {
    let survivor = match resolve_device(&state.pool, id).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let device: Option<(Value,)> = match sqlx::query_as(
        "select row_to_json(t) from (select * from devices t where id = $1) t",
    )
    .bind(survivor)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let Some((mut device,)) = device else {
        return err(StatusCode::NOT_FOUND, "not found");
    };

    let member_ids = match aggregate_member_ids(&state.pool, survivor).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let interfaces: Vec<(Value,)> = match sqlx::query_as(
        "select row_to_json(t) from (select * from interfaces t where device_id = any($1)) t",
    )
    .bind(&member_ids)
    .fetch_all(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let evidence: Vec<(Value,)> = match sqlx::query_as(
        "select row_to_json(t) from (select * from evidence t \
            where t.subject_table = 'devices' and t.subject_id = any($1) \
            order by t.last_seen desc) t",
    )
    .bind(&member_ids)
    .fetch_all(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let confidence =
        read_time_confidence(&evidence.iter().map(|(v,)| v.clone()).collect::<Vec<_>>());

    if let Value::Object(map) = &mut device {
        map.insert(
            "interfaces".to_string(),
            Value::Array(interfaces.into_iter().map(|(v,)| v).collect()),
        );
        map.insert(
            "evidence".to_string(),
            Value::Array(evidence.into_iter().map(|(v,)| v).collect()),
        );
        map.insert("identity_confidence".to_string(), json!(confidence));
        map.insert("merged_member_ids".to_string(), json!(member_ids));
    }

    (StatusCode::OK, Json(device))
}

/// Read-time confidence (design Decision 7): highest confidence among
/// non-absent, non-expired evidence rows; a `confirmed_by` row always
/// wins outright.
pub fn read_time_confidence(evidence: &[Value]) -> f32 {
    let mut best: f32 = 0.0;
    for row in evidence {
        let absent = row.get("absent").and_then(Value::as_bool).unwrap_or(false);
        if absent {
            continue;
        }
        if row
            .get("confirmed_by")
            .map(|v| !v.is_null())
            .unwrap_or(false)
        {
            return 1.0;
        }
        if let Some(c) = row.get("confidence").and_then(Value::as_f64) {
            best = best.max(c as f32);
        }
    }
    best
}

#[derive(Debug, Deserialize)]
pub struct CreateDevice {
    pub device_type: String,
    pub name: Option<String>,
    pub status: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateDevice>,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let row: Result<(Value,), sqlx::Error> = sqlx::query_as(
        "insert into devices (device_type, name, status) values ($1, $2, coalesce($3, 'active')) \
         returning row_to_json(devices.*)",
    )
    .bind(&req.device_type)
    .bind(&req.name)
    .bind(&req.status)
    .fetch_one(&mut *tx)
    .await;

    let row = match row {
        Ok((v,)) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let device_id = row
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap();

    if let Err(e) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "device.create",
        Some("devices"),
        Some(device_id),
        "success",
        Some(json!({"device_type": req.device_type})),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (StatusCode::CREATED, Json(row))
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);

    let result = if let Some((created_at, id)) = cursor {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from devices where status != 'merged' \
                and (created_at, id) > ($1::timestamptz, $2) order by created_at, id limit $3) t",
        )
        .bind(created_at)
        .bind(id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from devices where status != 'merged' \
                order by created_at, id limit $1) t",
        )
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    };

    let rows = match result {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let items: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
    let next_cursor = items.last().and_then(|last| {
        let created_at = last.get("created_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_cursor(created_at, Uuid::parse_str(id).ok()?))
    });

    (
        StatusCode::OK,
        Json(json!({ "items": items, "next_cursor": next_cursor })),
    )
}

#[derive(Debug, Deserialize)]
pub struct PatchDevice {
    pub version: i32,
    pub name: Option<String>,
    pub status: Option<String>,
    pub device_type: Option<String>,
}

pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchDevice>,
) -> (StatusCode, Json<Value>) {
    let row: Option<(Value,)> = match sqlx::query_as(
        "update devices set \
            name = coalesce($1, name), \
            status = coalesce($2, status), \
            device_type = coalesce($3, device_type), \
            version = version + 1, updated_at = now() \
         where id = $4 and version = $5 \
         returning row_to_json(devices.*)",
    )
    .bind(&req.name)
    .bind(&req.status)
    .bind(&req.device_type)
    .bind(id)
    .bind(req.version)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    match row {
        Some((v,)) => (StatusCode::OK, Json(v)),
        None => err(StatusCode::CONFLICT, "version mismatch or not found"),
    }
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub into: Uuid,
    pub reason: Option<String>,
    pub score: Option<f32>,
}

/// POST /api/v1/devices/{id}:merge — absorb `id` into `into`. Only flips
/// `devices.canonical_of`/`status` on the absorbed row; every
/// `interfaces`/`evidence` row keeps its original `device_id`/`subject_id`
/// (design §4), so `undo-merge` can restore it losslessly.
pub async fn merge(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(absorbed_id): Path<Uuid>,
    Json(req): Json<MergeRequest>,
) -> (StatusCode, Json<Value>) {
    if absorbed_id == req.into {
        return err(StatusCode::BAD_REQUEST, "cannot merge a device into itself");
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let updated = sqlx::query(
        "update devices set canonical_of = $1, status = 'merged', version = version + 1, updated_at = now() \
         where id = $2 and status != 'merged'",
    )
    .bind(req.into)
    .bind(absorbed_id)
    .execute(&mut *tx)
    .await;

    match updated {
        Ok(r) if r.rows_affected() == 0 => {
            return err(StatusCode::CONFLICT, "device not found or already merged");
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        _ => {}
    }

    let merge_event: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        "insert into merge_events (kind, survivor_device_id, absorbed_device_id, performed_by, reason, score, explanation) \
         values ('merge', $1, $2, $3, $4, $5, $6) returning id",
    )
    .bind(req.into)
    .bind(absorbed_id)
    .bind(user.0)
    .bind(&req.reason)
    .bind(req.score)
    .bind(json!({"manual": true, "reason": req.reason}))
    .fetch_one(&mut *tx)
    .await;

    if let Err(e) = merge_event {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = Recorder::record_change(
        &mut tx,
        "devices",
        req.into,
        "device",
        "notice",
        Some(json!({"absorbed": absorbed_id})),
        Some(json!({"merged_with": absorbed_id})),
        Some("manual"),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "device.merge",
        Some("devices"),
        Some(absorbed_id),
        "success",
        Some(json!({"survivor": req.into})),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (
        StatusCode::OK,
        Json(json!({"survivor": req.into, "absorbed": absorbed_id})),
    )
}

/// POST /api/v1/devices/{id}:undo-merge — reverse the most recent
/// not-yet-undone merge that absorbed `id`. O(1) metadata flip: no
/// interface/evidence row was ever re-pointed, so nothing to restore.
pub async fn undo_merge(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(absorbed_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let merge_event: Option<(Uuid, Uuid)> = match sqlx::query_as(
        "select id, survivor_device_id from merge_events \
         where absorbed_device_id = $1 and kind = 'merge' and undone_at is null \
         order by created_at desc limit 1",
    )
    .bind(absorbed_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let Some((merge_event_id, survivor_id)) = merge_event else {
        return err(
            StatusCode::NOT_FOUND,
            "no active merge to undo for this device",
        );
    };

    if let Err(e) = sqlx::query(
        "update devices set canonical_of = null, status = 'active', version = version + 1, updated_at = now() \
         where id = $1",
    )
    .bind(absorbed_id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) =
        sqlx::query("update merge_events set undone_at = now(), undone_by = $1 where id = $2")
            .bind(user.0)
            .bind(merge_event_id)
            .execute(&mut *tx)
            .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = sqlx::query(
        "insert into merge_events (kind, survivor_device_id, absorbed_device_id, performed_by, reason) \
         values ('undo_merge', $1, $2, $3, 'operator undo')",
    )
    .bind(survivor_id)
    .bind(absorbed_id)
    .bind(user.0)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = Recorder::record_change(
        &mut tx,
        "devices",
        absorbed_id,
        "device",
        "notice",
        Some(json!({"status": "merged"})),
        Some(json!({"status": "active"})),
        Some("manual"),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "device.undo_merge",
        Some("devices"),
        Some(absorbed_id),
        "success",
        None,
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (StatusCode::OK, Json(json!({"restored": absorbed_id})))
}

#[derive(Debug, Deserialize)]
pub struct SplitRequest {
    pub interface_ids: Vec<Uuid>,
    pub device_type: String,
    pub name: Option<String>,
}

/// POST /api/v1/devices/{id}:split — per-interface split (design Decision
/// 3): move whole interfaces (and their evidence) to a newly created
/// device row. Destructive to the source device's aggregate by operator
/// intent, unlike merge/undo.
pub async fn split(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(source_id): Path<Uuid>,
    Json(req): Json<SplitRequest>,
) -> (StatusCode, Json<Value>) {
    if req.interface_ids.is_empty() {
        return err(StatusCode::BAD_REQUEST, "interface_ids must not be empty");
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let new_device: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        "insert into devices (device_type, name, status) values ($1, $2, 'active') returning id",
    )
    .bind(&req.device_type)
    .bind(&req.name)
    .fetch_one(&mut *tx)
    .await;

    let new_device_id = match new_device {
        Ok((id,)) => id,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    if let Err(e) = sqlx::query(
        "update interfaces set device_id = $1, version = version + 1, updated_at = now() \
         where id = any($2) and device_id = $3",
    )
    .bind(new_device_id)
    .bind(&req.interface_ids)
    .bind(source_id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    // Evidence rows whose subject is an interface (`subject_table =
    // 'interfaces'`) already travel with the interface for free: the
    // interface's own id doesn't change, only its `device_id`. Nothing to
    // re-point here -- this is the same non-destructive-by-default
    // property merge/undo relies on (§4).

    if let Err(e) = sqlx::query(
        "insert into merge_events (kind, survivor_device_id, absorbed_device_id, performed_by, reason) \
         values ('split', $1, $2, $3, 'operator split')",
    )
    .bind(source_id)
    .bind(new_device_id)
    .bind(user.0)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "device.split",
        Some("devices"),
        Some(source_id),
        "success",
        Some(json!({"new_device": new_device_id, "interface_ids": req.interface_ids})),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if let Err(e) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (
        StatusCode::CREATED,
        Json(json!({"new_device_id": new_device_id})),
    )
}

#[derive(Debug, Deserialize)]
pub struct PinIdentifierRequest {
    pub rule_type: String,
    pub value: String,
}

/// POST /api/v1/devices/{id}/identity-rules — attach (and optionally pin)
/// an identifier. A pinned identifier can never be outscored by
/// conflicting automatic evidence (design §4.2).
pub async fn pin_identifier(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(device_id): Path<Uuid>,
    Json(req): Json<PinIdentifierRequest>,
) -> (StatusCode, Json<Value>) {
    // No ON CONFLICT DO UPDATE here deliberately: the partial unique index
    // (migration 0005) means a conflict means *another* device already
    // pinned this identifier, which must surface as a real conflict, not
    // silently reassign it.
    let row: Result<(Value,), sqlx::Error> = sqlx::query_as(
        "insert into identity_rules (device_id, rule_type, value, pinned, pinned_by, pinned_at) \
         values ($1, $2, $3, true, $4, now()) \
         returning row_to_json(identity_rules.*)",
    )
    .bind(device_id)
    .bind(&req.rule_type)
    .bind(&req.value)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok((v,)) => (StatusCode::CREATED, Json(v)),
        Err(e) => err(StatusCode::CONFLICT, e.to_string()),
    }
}

#[cfg(test)]
mod gate_tests {
    //! Acceptance gate (spec §17 M1, design §7): "a merge can be undone
    //! without losing observations."

    use super::*;
    use axum::extract::Path as AxumPath;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    async fn make_device_with_interface_and_evidence(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
        let (device_id,): (Uuid,) = sqlx::query_as(
            "insert into devices (device_type) values ('physical_host') returning id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let (interface_id,): (Uuid,) = sqlx::query_as(
            "insert into interfaces (device_id, description) values ($1, 'eth0') returning id",
        )
        .bind(device_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let (evidence_id,): (Uuid,) = sqlx::query_as(
            "insert into evidence (subject_table, subject_id, source_type, attribute, value, confidence, first_seen, last_seen) \
             values ('devices', $1, 'agent', 'hostname', '\"h\"'::jsonb, 0.9, now(), now()) returning id",
        )
        .bind(device_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (device_id, interface_id, evidence_id)
    }

    #[tokio::test]
    async fn merge_then_undo_loses_no_observations() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let state = AppState { pool: pool.clone() };
        let user = CurrentUser(Uuid::new_v4());
        sqlx::query("insert into users (id, email, password_hash) values ($1, $2, 'x') on conflict do nothing")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&pool)
            .await
            .unwrap();

        let (survivor_id, survivor_iface, survivor_evidence) =
            make_device_with_interface_and_evidence(&pool).await;
        let (absorbed_id, absorbed_iface, absorbed_evidence) =
            make_device_with_interface_and_evidence(&pool).await;

        // Merge.
        let (status, _) = merge(
            State(state.clone()),
            Extension(user),
            AxumPath(absorbed_id),
            Json(MergeRequest {
                into: survivor_id,
                reason: Some("test".into()),
                score: Some(0.9),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // While merged: aggregate GET must show both interfaces.
        let member_ids = aggregate_member_ids(&pool, survivor_id).await.unwrap();
        assert!(member_ids.contains(&survivor_id));
        assert!(member_ids.contains(&absorbed_id));

        // Row-level check: the absorbed device's interface/evidence rows
        // must NOT have been re-pointed to the survivor -- only
        // `devices.canonical_of`/`status` changed.
        let (iface_owner,): (Uuid,) =
            sqlx::query_as("select device_id from interfaces where id = $1")
                .bind(absorbed_iface)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            iface_owner, absorbed_id,
            "interface row not re-pointed on merge"
        );

        let (evidence_subject,): (Uuid,) =
            sqlx::query_as("select subject_id from evidence where id = $1")
                .bind(absorbed_evidence)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            evidence_subject, absorbed_id,
            "evidence row not re-pointed on merge"
        );

        // Undo.
        let (status, _) =
            undo_merge(State(state.clone()), Extension(user), AxumPath(absorbed_id)).await;
        assert_eq!(status, StatusCode::OK);

        // After undo: every original row, both devices, byte-identical to
        // before the merge -- nothing lost, nothing duplicated.
        let (iface_owner,): (Uuid,) =
            sqlx::query_as("select device_id from interfaces where id = $1")
                .bind(absorbed_iface)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(iface_owner, absorbed_id);
        let (iface_owner2,): (Uuid,) =
            sqlx::query_as("select device_id from interfaces where id = $1")
                .bind(survivor_iface)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(iface_owner2, survivor_id);

        let interface_count: (i64,) =
            sqlx::query_as("select count(*) from interfaces where id in ($1, $2)")
                .bind(survivor_iface)
                .bind(absorbed_iface)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(interface_count.0, 2, "no interface row lost or duplicated");

        let evidence_count: (i64,) =
            sqlx::query_as("select count(*) from evidence where id in ($1, $2)")
                .bind(survivor_evidence)
                .bind(absorbed_evidence)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(evidence_count.0, 2, "no evidence row lost or duplicated");

        let (status_after,): (String,) = sqlx::query_as("select status from devices where id = $1")
            .bind(absorbed_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status_after, "active");
    }
}
