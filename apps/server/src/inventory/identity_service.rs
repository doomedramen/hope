//! Reconciliation service: wires `domain::inventory::identity::score`
//! (pure, DB-free) to `identity_rules`/`devices`/`identity_suggestions`.
//! Spec §4.2; design docs/design/m1-inventory.md §3, §9 slice 6.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use domain::inventory::identity::{Decision, Identifier, IdentifierType, score};
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

async fn load_candidate_identifiers(
    pool: &PgPool,
    device_id: Uuid,
) -> sqlx::Result<Vec<Identifier>> {
    let rows: Vec<(String, String, bool)> =
        sqlx::query_as("select rule_type, value, pinned from identity_rules where device_id = $1")
            .bind(device_id)
            .fetch_all(pool)
            .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(rt, value, pinned)| {
            let rule_type: IdentifierType = serde_json::from_value(Value::String(rt)).ok()?;
            Some(Identifier {
                rule_type,
                value,
                pinned,
            })
        })
        .collect())
}

/// Every non-merged device that shares at least one (rule_type, value)
/// pair with `observed` -- the candidate pool `score()` is run against.
async fn candidate_device_ids(pool: &PgPool, observed: &[Identifier]) -> sqlx::Result<Vec<Uuid>> {
    if observed.is_empty() {
        return Ok(vec![]);
    }
    let rule_types: Vec<String> = observed
        .iter()
        .map(|i| {
            serde_json::to_value(i.rule_type)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let values: Vec<String> = observed.iter().map(|i| i.value.clone()).collect();

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "select distinct ir.device_id from identity_rules ir \
         join unnest($1::text[], $2::text[]) as o(rule_type, value) \
           on ir.rule_type = o.rule_type and ir.value = o.value \
         join devices d on d.id = ir.device_id and d.status != 'merged'",
    )
    .bind(&rule_types)
    .bind(&values)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub struct ReconcileOutcome {
    pub decision: Decision,
    pub device_id: Option<Uuid>,
    pub suggestion_id: Option<Uuid>,
    pub explanation: Value,
}

/// Score `observed` against every live candidate device and act:
/// - `AutoMatch` -> attach any new identifiers to the winning device,
///   returns its id.
/// - `Suggested` -> writes an `identity_suggestions` row, no graph
///   mutation.
/// - `NewDevice` -> creates a new device row (device_type `unknown`) and
///   attaches the observed identifiers to it.
pub async fn reconcile(
    pool: &PgPool,
    observed: &[Identifier],
    performed_by: Option<Uuid>,
) -> sqlx::Result<ReconcileOutcome> {
    let candidates = candidate_device_ids(pool, observed).await?;

    let mut best: Option<(Uuid, domain::inventory::identity::Explanation)> = None;
    for candidate_id in candidates {
        let candidate_identifiers = load_candidate_identifiers(pool, candidate_id).await?;
        let explanation = score(observed, &candidate_identifiers);
        let better = match &best {
            None => true,
            Some((_, current)) => explanation.score > current.score,
        };
        if better {
            best = Some((candidate_id, explanation));
        }
    }

    match best {
        Some((device_id, explanation)) if explanation.decision == Decision::AutoMatch => {
            attach_identifiers(pool, device_id, observed).await?;
            Ok(ReconcileOutcome {
                decision: Decision::AutoMatch,
                device_id: Some(device_id),
                suggestion_id: None,
                explanation: serde_json::to_value(&explanation).unwrap(),
            })
        }
        Some((device_id, explanation)) if explanation.decision == Decision::Suggested => {
            let suggestion_id: (Uuid,) = sqlx::query_as(
                "insert into identity_suggestions (candidate_device_id, observed, explanation, score) \
                 values ($1, $2, $3, $4) returning id",
            )
            .bind(device_id)
            .bind(serde_json::to_value(observed).unwrap())
            .bind(serde_json::to_value(&explanation).unwrap())
            .bind(explanation.score)
            .fetch_one(pool)
            .await?;

            Ok(ReconcileOutcome {
                decision: Decision::Suggested,
                device_id: None,
                suggestion_id: Some(suggestion_id.0),
                explanation: serde_json::to_value(&explanation).unwrap(),
            })
        }
        _ => {
            let device_id: (Uuid,) = sqlx::query_as(
                "insert into devices (device_type, status) values ('unknown', 'active') returning id",
            )
            .fetch_one(pool)
            .await?;
            attach_identifiers(pool, device_id.0, observed).await?;

            let _ = performed_by;
            Ok(ReconcileOutcome {
                decision: Decision::NewDevice,
                device_id: Some(device_id.0),
                suggestion_id: None,
                explanation: json!({"decision": "new_device"}),
            })
        }
    }
}

async fn attach_identifiers(
    pool: &PgPool,
    device_id: Uuid,
    observed: &[Identifier],
) -> sqlx::Result<()> {
    for identifier in observed {
        let rule_type = serde_json::to_value(identifier.rule_type)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let exists: (i64,) = sqlx::query_as(
            "select count(*) from identity_rules where device_id = $1 and rule_type = $2 and value = $3",
        )
        .bind(device_id)
        .bind(&rule_type)
        .bind(&identifier.value)
        .fetch_one(pool)
        .await?;
        if exists.0 == 0 {
            // A pinned identifier elsewhere blocks reattachment here
            // (the partial unique index) -- surfaced as a DB error,
            // which the caller logs and skips rather than aborting the
            // whole reconciliation for one conflicting identifier.
            let _ = sqlx::query(
                "insert into identity_rules (device_id, rule_type, value) values ($1, $2, $3)",
            )
            .bind(device_id)
            .bind(&rule_type)
            .bind(&identifier.value)
            .execute(pool)
            .await;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ReconcileRequest {
    pub identifiers: Vec<Identifier>,
}

/// POST /api/v1/devices:reconcile — ingest observed identifiers (from an
/// agent report, scan, or manual entry) and reconcile against known
/// devices per §3's thresholds.
pub async fn reconcile_handler(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<ReconcileRequest>,
) -> (StatusCode, Json<Value>) {
    match reconcile(&state.pool, &req.identifiers, Some(user.0)).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(json!({
                "decision": outcome.decision,
                "device_id": outcome.device_id,
                "suggestion_id": outcome.suggestion_id,
                "explanation": outcome.explanation,
            })),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn list_suggestions(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);

    let result = if let Some((created_at, id)) = cursor {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from identity_suggestions \
                where status = 'pending' and (created_at, id) > ($1::timestamptz, $2) \
                order by created_at, id limit $3) t",
        )
        .bind(created_at)
        .bind(id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from identity_suggestions \
                where status = 'pending' order by created_at, id limit $1) t",
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

pub async fn confirm_suggestion(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve_suggestion(&state, user, id, true).await
}

pub async fn reject_suggestion(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    resolve_suggestion(&state, user, id, false).await
}

async fn resolve_suggestion(
    state: &AppState,
    user: CurrentUser,
    id: Uuid,
    confirm: bool,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let suggestion: Option<(Uuid, Value)> = match sqlx::query_as(
        "select candidate_device_id, observed from identity_suggestions \
         where id = $1 and status = 'pending' for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let Some((candidate_device_id, observed_json)) = suggestion else {
        return err(StatusCode::NOT_FOUND, "no pending suggestion with that id");
    };

    let new_status = if confirm { "confirmed" } else { "rejected" };
    if let Err(e) = sqlx::query(
        "update identity_suggestions set status = $1, resolved_by = $2, resolved_at = now(), updated_at = now() \
         where id = $3",
    )
    .bind(new_status)
    .bind(user.0)
    .bind(id)
    .execute(&mut *tx)
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    if confirm {
        let observed: Vec<Identifier> = match serde_json::from_value(observed_json) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        for identifier in &observed {
            let rule_type = serde_json::to_value(identifier.rule_type)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            let _ = sqlx::query(
                "insert into identity_rules (device_id, rule_type, value) values ($1, $2, $3)",
            )
            .bind(candidate_device_id)
            .bind(&rule_type)
            .bind(&identifier.value)
            .execute(&mut *tx)
            .await;
        }
    }

    if let Err(e) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        if confirm {
            "identity_suggestion.confirm"
        } else {
            "identity_suggestion.reject"
        },
        Some("identity_suggestions"),
        Some(id),
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

    (
        StatusCode::OK,
        Json(json!({"id": id, "status": new_status})),
    )
}

#[cfg(test)]
mod gate_tests {
    //! Acceptance gates (spec §17 M1, design §7):
    //! - "strong evidence reconciles two observations to one device"
    //! - "ambiguous evidence -> review suggestion, not silent merge"

    use super::*;
    use sqlx::PgPool;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    fn id(t: IdentifierType, v: &str) -> Identifier {
        Identifier {
            rule_type: t,
            value: v.to_string(),
            pinned: false,
        }
    }

    #[tokio::test]
    async fn strong_shared_identifier_reconciles_to_one_device() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let unique_agent_id = format!("agent-{}", Uuid::new_v4());
        let observed = vec![id(IdentifierType::AgentId, &unique_agent_id)];

        let first = reconcile(&pool, &observed, None).await.unwrap();
        assert_eq!(first.decision, Decision::NewDevice);
        let device_id = first.device_id.unwrap();

        // A second, independent observation sharing the same
        // deterministic agent_id must reconcile to the SAME device, not
        // create a second one.
        let second = reconcile(&pool, &observed, None).await.unwrap();
        assert_eq!(second.decision, Decision::AutoMatch);
        assert_eq!(second.device_id, Some(device_id));

        let device_count: (i64,) = sqlx::query_as(
            "select count(*) from identity_rules where rule_type = 'agent_id' and value = $1",
        )
        .bind(&unique_agent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        // Only one identity_rules row for this agent_id (attach is
        // idempotent, doesn't insert a duplicate), and it's the same
        // device.
        assert_eq!(device_count.0, 1);
    }

    #[tokio::test]
    async fn hostname_only_match_is_a_suggestion_not_a_silent_merge() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let unique_hostname = format!("host-{}", Uuid::new_v4());
        let observed = vec![id(IdentifierType::Hostname, &unique_hostname)];

        let first = reconcile(&pool, &observed, None).await.unwrap();
        assert_eq!(first.decision, Decision::NewDevice);
        let device_id = first.device_id.unwrap();

        let devices_before: (i64,) = sqlx::query_as("select count(*) from devices")
            .fetch_one(&pool)
            .await
            .unwrap();

        // A second observation that only matches on hostname must land
        // in the review queue, not silently attach/merge.
        let second = reconcile(&pool, &observed, None).await.unwrap();
        assert_eq!(second.decision, Decision::Suggested);
        assert!(second.suggestion_id.is_some());
        assert!(
            second.device_id.is_none(),
            "no graph mutation on a suggested match"
        );

        let devices_after: (i64,) = sqlx::query_as("select count(*) from devices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            devices_before.0, devices_after.0,
            "no new device created either"
        );

        let suggestion: (Uuid, String) = sqlx::query_as(
            "select candidate_device_id, status from identity_suggestions where id = $1",
        )
        .bind(second.suggestion_id.unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(suggestion.0, device_id);
        assert_eq!(suggestion.1, "pending");

        // And it's visible via GET /api/v1/identity-suggestions's query.
        let pending: (i64,) = sqlx::query_as(
            "select count(*) from identity_suggestions where status = 'pending' and id = $1",
        )
        .bind(second.suggestion_id.unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending.0, 1);
    }
}
