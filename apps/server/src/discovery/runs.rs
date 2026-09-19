//! Scan-run creation. A run is queued only after an operator has confirmed
//! the exact scope target count; workers resolve the run by `job_id`.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::state::AppState;

const PORTS_PER_TARGET: i64 = 65_535;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn run_view(mut run: Value) -> Value {
    let complete = run
        .get("complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let succeeded = run
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status == "succeeded");
    run["authoritative"] = Value::Bool(complete && succeeded);
    run
}

fn can_request_cancellation(status: &str, complete: bool) -> bool {
    !complete && matches!(status, "pending" | "running")
}

#[derive(Debug, Deserialize)]
pub struct CreateRun {
    /// A first discovery run and a periodic change scan both use a complete
    /// TCP range in M2. Fingerprinting begins in M3.
    pub kind: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CreateRun>,
) -> (StatusCode, Json<Value>) {
    if !matches!(
        request.kind.as_str(),
        "initial_discovery" | "change_scan" | "full_tcp"
    ) {
        return err(
            StatusCode::BAD_REQUEST,
            "kind must be initial_discovery, change_scan, or full_tcp",
        );
    }
    let Some(idempotency_key) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
    else {
        return err(
            StatusCode::BAD_REQUEST,
            "Idempotency-Key header is required",
        );
    };
    if idempotency_key.len() > 255 {
        return err(StatusCode::BAD_REQUEST, "Idempotency-Key exceeds 255 bytes");
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let scope: Option<(i64, i32)> = match sqlx::query_as(
        "select target_count, version from discovery_scopes \
         where network_id = $1 and confirmed_at is not null and enabled",
    )
    .bind(network_id)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((target_count, scope_version)) = scope else {
        return err(
            StatusCode::CONFLICT,
            "network needs an enabled, confirmed discovery scope",
        );
    };
    let ports_planned = match target_count.checked_mul(PORTS_PER_TARGET) {
        Some(count) => count,
        None => return err(StatusCode::BAD_REQUEST, "scope is too large to scan"),
    };
    let job_type = "discovery.full_tcp";
    let job_id = match jobs::enqueue_in(
        &mut transaction,
        job_type,
        idempotency_key,
        json!({"network_id": network_id, "kind": request.kind}),
    )
    .await
    {
        Ok(id) => id,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let run: Option<(Value,)> = match sqlx::query_as(
        "insert into scan_runs \
             (network_id, job_id, kind, scope_version, targets_planned, ports_planned, requested_by) \
         values ($1, $2, $3, $4, $5, $6, $7) \
         on conflict (job_id) do nothing \
         returning row_to_json(scan_runs.*)",
    )
    .bind(network_id)
    .bind(job_id)
    .bind(&request.kind)
    .bind(scope_version)
    .bind(target_count)
    .bind(ports_planned)
    .bind(user.0)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let (run, created) = match run {
        Some((run,)) => (run, true),
        None => {
            let existing: Option<(Value,)> = match sqlx::query_as(
                "select row_to_json(scan_runs.*) from scan_runs where job_id = $1",
            )
            .bind(job_id)
            .fetch_optional(&mut *transaction)
            .await
            {
                Ok(run) => run,
                Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            };
            let Some((run,)) = existing else {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "scan run was not created",
                );
            };
            (run, false)
        }
    };
    let run = run_view(run);
    if created
        && let Err(error) = Recorder::record_audit(
            &mut transaction,
            Some(user.0),
            "operator",
            "scan_run.create",
            Some("scan_runs"),
            run.get("id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok()),
            "success",
            Some(json!({"network_id": network_id, "kind": request.kind, "job_id": job_id})),
        )
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = transaction.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(run),
    )
}

/// Retrieve one scan run. The response keeps partial progress visible while
/// exposing an explicit authority bit that is true only for a complete,
/// succeeded run.
pub async fn get(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Path(run_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let run: Option<(Value,)> =
        match sqlx::query_as("select row_to_json(scan_runs.*) from scan_runs where id = $1")
            .bind(run_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(run) => run,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    let Some((run,)) = run else {
        return err(StatusCode::NOT_FOUND, "scan run not found");
    };
    (StatusCode::OK, Json(run_view(run)))
}

/// Request cooperative cancellation for one pending or running scan.
/// Cancellation intent is written to both the scan run and its job in one
/// transaction; the worker owns the terminal `cancelled` transition.
pub async fn cancel(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(run_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let row: Option<(Value, Uuid, String, bool, bool)> = match sqlx::query_as(
        "select row_to_json(sr.*), sr.job_id, sr.status, sr.complete, \
                sr.cancellation_requested \
         from scan_runs sr \
         join jobs j on j.id = sr.job_id \
         where sr.id = $1 \
         for update of sr, j",
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(row) => row,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((run, job_id, status, complete, cancellation_requested)) = row else {
        return err(StatusCode::NOT_FOUND, "scan run not found");
    };

    if status == "cancelled" {
        if let Err(error) = transaction.commit().await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
        return (StatusCode::OK, Json(run_view(run)));
    }
    if !can_request_cancellation(&status, complete) {
        return err(
            StatusCode::CONFLICT,
            "scan run is no longer pending or running",
        );
    }

    let job_updated = match jobs::request_cancel_in(&mut transaction, job_id).await {
        Ok(updated) => updated,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if !job_updated {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "scan job not found");
    }
    if !cancellation_requested {
        let updated = match sqlx::query(
            "update scan_runs set cancellation_requested = true, updated_at = now() \
             where id = $1 and status in ('pending', 'running') and not complete",
        )
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        {
            Ok(result) => result,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        if updated.rows_affected() != 1 {
            return err(
                StatusCode::CONFLICT,
                "scan run changed before cancellation was requested",
            );
        }
        if let Err(error) = Recorder::record_audit(
            &mut transaction,
            Some(user.0),
            "operator",
            "scan_run.cancel",
            Some("scan_runs"),
            Some(run_id),
            "success",
            Some(json!({"job_id": job_id, "previous_status": status})),
        )
        .await
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }

    let refreshed: Option<(Value,)> =
        match sqlx::query_as("select row_to_json(scan_runs.*) from scan_runs where id = $1")
            .bind(run_id)
            .fetch_optional(&mut *transaction)
            .await
        {
            Ok(run) => run,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    let Some((run,)) = refreshed else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "scan run disappeared");
    };
    if let Err(error) = transaction.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (
        if cancellation_requested {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(run_view(run)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_run_is_never_authoritative() {
        let run = run_view(json!({"status": "failed", "complete": false}));
        assert_eq!(run["authoritative"], false);
    }

    #[test]
    fn complete_run_is_authoritative_only_after_success() {
        let failed = run_view(json!({"status": "failed", "complete": true}));
        let succeeded = run_view(json!({"status": "succeeded", "complete": true}));

        assert_eq!(failed["authoritative"], false);
        assert_eq!(succeeded["authoritative"], true);
    }

    #[test]
    fn only_pending_and_running_incomplete_runs_can_be_cancelled() {
        assert!(can_request_cancellation("pending", false));
        assert!(can_request_cancellation("running", false));
        assert!(!can_request_cancellation("succeeded", true));
        assert!(!can_request_cancellation("failed", false));
        assert!(!can_request_cancellation("cancelled", false));
        assert!(!can_request_cancellation("running", true));
    }

    #[tokio::test]
    async fn db_cancel_request_updates_job_run_and_audit_atomically() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let user_id: (Uuid,) = sqlx::query_as(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("scan-cancel-{}@example.test", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create test user");
        let network_id: (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('192.168.40.0/30', $1) returning id",
        )
        .bind(format!("scan-cancel-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create test network");
        let job_id: (Uuid,) = sqlx::query_as(
            "insert into jobs (job_type, idempotency_key, payload) +             values ('discovery.full_tcp', $1, $2) returning id",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(json!({"network_id": network_id.0}))
        .fetch_one(&pool)
        .await
        .expect("create test job");
        let run_id: (Uuid,) = sqlx::query_as(
            "insert into scan_runs +                (network_id, job_id, kind, scope_version, targets_planned, ports_planned, requested_by) +             values ($1, $2, 'initial_discovery', 1, 1, 65_535, $3) returning id",
        )
        .bind(network_id.0)
        .bind(job_id.0)
        .bind(user_id.0)
        .fetch_one(&pool)
        .await
        .expect("create test scan run");

        let state = AppState { pool: pool.clone() };
        let (status, Json(view)) = cancel(
            State(state.clone()),
            Extension(CurrentUser(user_id.0)),
            Path(run_id.0),
        )
        .await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(view["status"], "pending");
        assert_eq!(view["cancellation_requested"], true);
        assert_eq!(view["complete"], false);
        assert_eq!(view["authoritative"], false);

        let flags: (bool, bool) = sqlx::query_as(
            "select j.cancel_requested, sr.cancellation_requested +             from jobs j join scan_runs sr on sr.job_id = j.id where sr.id = $1",
        )
        .bind(run_id.0)
        .fetch_one(&pool)
        .await
        .expect("read cancellation flags");
        assert_eq!(flags, (true, true));

        let (audit_count,): (i64,) = sqlx::query_as(
            "select count(*) from audit_events where action = 'scan_run.cancel' and target_id = $1",
        )
        .bind(run_id.0)
        .fetch_one(&pool)
        .await
        .expect("read cancellation audit");
        assert_eq!(audit_count, 1);

        let (repeat_status, Json(repeat_view)) = cancel(
            State(state),
            Extension(CurrentUser(user_id.0)),
            Path(run_id.0),
        )
        .await;
        assert_eq!(repeat_status, StatusCode::OK);
        assert_eq!(repeat_view["authoritative"], false);

        let (get_status, Json(fetched_view)) = get(
            State(AppState { pool: pool.clone() }),
            Extension(CurrentUser(user_id.0)),
            Path(run_id.0),
        )
        .await;
        assert_eq!(get_status, StatusCode::OK);
        assert_eq!(fetched_view["complete"], false);
        assert!(fetched_view.get("error").is_some());
    }

    async fn pool_or_skip() -> Option<sqlx::PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }
}
