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
