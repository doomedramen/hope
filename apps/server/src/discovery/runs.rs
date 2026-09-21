//! Scan-run creation. A run is queued only after an operator has confirmed
//! the exact scope target count; workers resolve the run by `job_id`.

use std::net::Ipv4Addr;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use domain::discovery::ApprovedScope;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::state::AppState;

use super::ports::STANDARD_PORT_COUNT;

const FULL_TCP_PORT_COUNT: i64 = 65_535;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

pub(crate) fn run_view(mut run: Value) -> Value {
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

async fn enrich_run(pool: &PgPool, mut run: Value) -> Result<Value, sqlx::Error> {
    let Some(job_id) = run
        .get("job_id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        run["job"] = Value::Null;
        run["logs"] = json!([]);
        return Ok(run);
    };

    let job: Option<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from (select id, job_type, status, progress, attempts, \
                max_attempts, run_at, last_error, created_at, updated_at \
         from jobs where id = $1) t",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    let logs: Vec<(Value,)> = sqlx::query_as(
        "select row_to_json(t) from (select id, actor_kind, action, result, detail, occurred_at \
         from audit_events where target_kind = 'scan_runs' and target_id = $1 \
         order by occurred_at asc, id asc limit 100) t",
    )
    .bind(
        run.get("id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok()),
    )
    .fetch_all(pool)
    .await?;

    run["job"] = job.map(|(job,)| job).unwrap_or(Value::Null);
    run["logs"] = Value::Array(logs.into_iter().map(|(log,)| log).collect());
    Ok(run)
}

fn active_scan_conflict(run_id: Option<Uuid>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "a scan is already pending or running for this network",
            "active_scan_id": run_id,
        })),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateRun {
    /// Network runs use the standard plan. Full TCP uses the device route.
    pub kind: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CreateRun>,
) -> (StatusCode, Json<Value>) {
    if !matches!(request.kind.as_str(), "initial_discovery" | "change_scan") {
        return err(
            StatusCode::BAD_REQUEST,
            "kind must be initial_discovery or change_scan",
        );
    }
    create_in_scope(state, user, network_id, headers, request.kind, None, None).await
}

/// Queue an exhaustive scan of one current device address inside a confirmed scope.
pub async fn create_device_full(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let candidates: Vec<(Uuid, String, String, Value)> = match sqlx::query_as(
        "select ds.network_id, host(a.ip), n.cidr::text, ds.excluded_cidrs \
         from addresses a \
         join interfaces i on i.id = a.interface_id \
         join devices d on d.id = i.device_id \
         join networks n on a.ip <<= n.cidr \
         join discovery_scopes ds on ds.network_id = n.id \
         where d.id = $1 and d.status != 'merged' and a.is_current \
           and ds.enabled and ds.confirmed_at is not null \
           and ds.confirmed_target_count = ds.target_count \
         order by ds.network_id, a.ip",
    )
    .bind(device_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    for (network_id, address, cidr, excluded) in candidates {
        if approved_device_address(&cidr, &excluded, &address) {
            return create_in_scope(
                state,
                user,
                network_id,
                headers,
                "full_tcp".to_owned(),
                Some(address),
                Some(device_id),
            )
            .await;
        }
    }
    err(
        StatusCode::CONFLICT,
        "device has no current address in an enabled, confirmed discovery scope",
    )
}

fn approved_device_address(cidr: &str, excluded: &Value, address: &str) -> bool {
    let Ok(exclusions) = serde_json::from_value::<Vec<String>>(excluded.clone()) else {
        return false;
    };
    let Ok(scope) = ApprovedScope::parse(cidr, &exclusions) else {
        return false;
    };
    let Ok(address) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    scope.cidr().hosts().any(|host| host == address)
        && !scope.exclusions().iter().any(|net| net.contains(&address))
}

pub async fn get_device_full(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Path(device_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let run: Option<(Value,)> = match sqlx::query_as(
        "select row_to_json(sr.*) from scan_runs sr \
         where sr.target_device_id = $1 and sr.kind = 'full_tcp' \
         order by sr.created_at desc, sr.id desc limit 1",
    )
    .bind(device_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((run,)) = run else {
        return (StatusCode::OK, Json(Value::Null));
    };
    match enrich_run(&state.pool, run_view(run)).await {
        Ok(run) => (StatusCode::OK, Json(run)),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn create_in_scope(
    state: AppState,
    user: CurrentUser,
    network_id: Uuid,
    headers: HeaderMap,
    kind: String,
    target_address: Option<String>,
    target_device_id: Option<Uuid>,
) -> (StatusCode, Json<Value>) {
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
    let scope: Option<(i64, i32, String, Value)> = match sqlx::query_as(
        "select ds.target_count, ds.version, n.cidr::text, ds.excluded_cidrs \
         from discovery_scopes ds join networks n on n.id = ds.network_id \
         where ds.network_id = $1 and ds.confirmed_at is not null and ds.enabled \
           and ds.confirmed_target_count = ds.target_count",
    )
    .bind(network_id)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((target_count, scope_version, cidr, excluded)) = scope else {
        return err(
            StatusCode::CONFLICT,
            "network needs an enabled, confirmed discovery scope",
        );
    };
    if let Some(address) = &target_address
        && !approved_device_address(&cidr, &excluded, address)
    {
        return err(
            StatusCode::CONFLICT,
            "device address is outside approved scope",
        );
    }
    if let (Some(address), Some(device_id)) = (&target_address, target_device_id) {
        let current: (bool,) = match sqlx::query_as(
            "select exists(select 1 from addresses a \
             join interfaces i on i.id = a.interface_id \
             where i.device_id = $1 and a.ip = $2::inet and a.is_current)",
        )
        .bind(device_id)
        .bind(address)
        .fetch_one(&mut *transaction)
        .await
        {
            Ok(current) => current,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        if !current.0 {
            return err(StatusCode::CONFLICT, "device address is no longer current");
        }
    }
    let job_type = "discovery.full_tcp";
    let active_scan: Option<(Uuid,)> = match sqlx::query_as(
        "select sr.id from scan_runs sr \
         join jobs active_job on active_job.id = sr.job_id \
         where sr.network_id = $1 \
           and sr.status in ('pending', 'running') and not sr.complete \
           and not (active_job.job_type = $2 and active_job.idempotency_key = $3) \
         order by sr.created_at desc, sr.id desc limit 1 for update of sr",
    )
    .bind(network_id)
    .bind(job_type)
    .bind(idempotency_key)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Some((run_id,)) = active_scan {
        return active_scan_conflict(Some(run_id));
    }
    let ports_per_target = if kind == "full_tcp" {
        FULL_TCP_PORT_COUNT
    } else {
        STANDARD_PORT_COUNT
    };
    let targets_planned = if target_address.is_some() {
        1
    } else {
        target_count
    };
    let ports_planned = match targets_planned.checked_mul(ports_per_target) {
        Some(count) => count,
        None => return err(StatusCode::BAD_REQUEST, "scope is too large to scan"),
    };
    let job_id = match jobs::enqueue_in(
        &mut transaction,
        job_type,
        idempotency_key,
        json!({"network_id": network_id, "kind": kind}),
    )
    .await
    {
        Ok(id) => id,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let run: Option<(Value,)> = match sqlx::query_as(
        "insert into scan_runs \
             (network_id, job_id, kind, scope_version, targets_planned, ports_planned, requested_by, target_address, target_device_id) \
         values ($1, $2, $3, $4, $5, $6, $7, $8::inet, $9) \
         on conflict (job_id) do nothing \
         returning row_to_json(scan_runs.*)",
    )
    .bind(network_id)
    .bind(job_id)
    .bind(&kind)
    .bind(scope_version)
    .bind(targets_planned)
    .bind(ports_planned)
    .bind(user.0)
    .bind(&target_address)
    .bind(target_device_id)
    .fetch_optional(&mut *transaction)
    .await
    {
        Ok(run) => run,
        Err(error) => {
            if error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .as_deref()
                == Some("23505")
            {
                return active_scan_conflict(None);
            }
            return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
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
            if run.get("network_id").and_then(Value::as_str)
                != Some(network_id.to_string().as_str())
                || run.get("kind").and_then(Value::as_str) != Some(kind.as_str())
                || run.get("target_device_id").and_then(Value::as_str)
                    != target_device_id.map(|id| id.to_string()).as_deref()
            {
                return err(
                    StatusCode::CONFLICT,
                    "idempotency key belongs to a different scan",
                );
            }
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
            Some(json!({"network_id": network_id, "kind": kind, "job_id": job_id, "target_address": target_address})),
        )
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = transaction.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let run = match enrich_run(&state.pool, run).await {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
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
    let run = match enrich_run(&state.pool, run_view(run)).await {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    (StatusCode::OK, Json(run))
}

/// List recent scan runs for a network, including the current job snapshot and
/// audit activity associated with each run. The response also identifies the
/// active run so callers can disable duplicate launch controls without local
/// state guesses.
pub async fn list(
    State(state): State<AppState>,
    Extension(_user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let network_exists: Option<(Uuid,)> =
        match sqlx::query_as("select id from networks where id = $1")
            .bind(network_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(network) => network,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    if network_exists.is_none() {
        return err(StatusCode::NOT_FOUND, "network not found");
    }
    let runs: Vec<(Value,)> = match sqlx::query_as(
        "select row_to_json(scan_runs.*) from scan_runs \
         where network_id = $1 order by created_at desc, id desc limit 50",
    )
    .bind(network_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(runs) => runs,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let mut items = Vec::with_capacity(runs.len());
    for (run,) in runs {
        let run = match enrich_run(&state.pool, run_view(run)).await {
            Ok(run) => run,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        items.push(run);
    }
    let active_scan = items.iter().find(|run| {
        run.get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| matches!(status, "pending" | "running"))
            && !run
                .get("complete")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    });
    (
        StatusCode::OK,
        Json(json!({
            "items": items,
            "active_scan": active_scan,
        })),
    )
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
        let run = match enrich_run(&state.pool, run_view(run)).await {
            Ok(run) => run,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        return (StatusCode::OK, Json(run));
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
    let run = match enrich_run(&state.pool, run_view(run)).await {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    (
        if cancellation_requested {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(run),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_scan_requires_approved_host_address() {
        let exclusions = json!(["192.168.40.2/32"]);
        assert!(approved_device_address(
            "192.168.40.0/29",
            &exclusions,
            "192.168.40.1"
        ));
        assert!(!approved_device_address(
            "192.168.40.0/29",
            &exclusions,
            "192.168.40.2"
        ));
        assert!(!approved_device_address(
            "192.168.40.0/29",
            &exclusions,
            "192.168.40.0"
        ));
        assert!(!approved_device_address(
            "192.168.40.0/29",
            &exclusions,
            "192.168.41.1"
        ));
    }

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
            "insert into jobs (job_type, idempotency_key, payload) \
             values ('discovery.full_tcp', $1, $2) returning id",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(json!({"network_id": network_id.0}))
        .fetch_one(&pool)
        .await
        .expect("create test job");
        let run_id: (Uuid,) = sqlx::query_as(
            "insert into scan_runs \
                (network_id, job_id, kind, scope_version, targets_planned, ports_planned, requested_by) \
             values ($1, $2, 'initial_discovery', 1, 1, 65_535, $3) returning id",
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
            "select j.cancel_requested, sr.cancellation_requested \
             from jobs j join scan_runs sr on sr.job_id = j.id where sr.id = $1",
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
