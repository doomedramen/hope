//! M2 discovery scope drafts and target-count confirmation.
//!
//! This is the only route layer allowed to create a `discovery_scopes` row.
//! The scanner will consume confirmed rows only, making an accidental broad
//! CIDR impossible to enqueue through normal product paths.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use domain::discovery::ApprovedScope;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::discovery::runs::run_view;
use crate::inventory::events::Recorder;
use crate::state::AppState;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

#[derive(Debug, Deserialize)]
pub struct DraftScope {
    #[serde(default)]
    pub excluded_cidrs: Vec<String>,
    pub scan_profile: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmScope {
    pub target_count: i64,
}

/// Load the durable discovery workflow state used by Infrastructure.
/// Returning the latest run alongside the scope lets the UI recover after a
/// navigation or full reload without inviting a duplicate initial scan.
pub async fn get_state(
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

    let scope: Option<(Value,)> = match sqlx::query_as(
        "select row_to_json(discovery_scopes.*) \
         from discovery_scopes where network_id = $1",
    )
    .bind(network_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let scan_run: Option<(Value,)> = match sqlx::query_as(
        "select row_to_json(scan_runs.*) \
         from scan_runs where network_id = $1 \
         order by created_at desc, id desc limit 1",
    )
    .bind(network_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(run) => run,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    (
        StatusCode::OK,
        Json(json!({
            "scope": scope.map(|(scope,)| scope),
            "scan_run": scan_run.map(|(run,)| run_view(run)),
        })),
    )
}

pub async fn draft(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
    Json(request): Json<DraftScope>,
) -> (StatusCode, Json<Value>) {
    let network: Option<(String,)> =
        match sqlx::query_as("select cidr::text from networks where id = $1")
            .bind(network_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(row) => row,
            Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    let Some((cidr,)) = network else {
        return err(StatusCode::NOT_FOUND, "network not found");
    };
    let scope = match ApprovedScope::parse(&cidr, &request.excluded_cidrs) {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let target_count = scope.target_count() as i64;
    let scan_profile = request.scan_profile.unwrap_or_else(|| "normal".to_string());
    if !matches!(scan_profile.as_str(), "normal" | "low_impact") {
        return err(
            StatusCode::BAD_REQUEST,
            "scan_profile must be normal or low_impact",
        );
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let row: Result<(Value,), sqlx::Error> = sqlx::query_as(
        "insert into discovery_scopes (network_id, excluded_cidrs, scan_profile, target_count) \
         values ($1, $2, $3, $4) \
         on conflict (network_id) do update set \
             excluded_cidrs = excluded.excluded_cidrs, scan_profile = excluded.scan_profile, \
             target_count = excluded.target_count, confirmed_at = null, \
             confirmed_target_count = null, version = discovery_scopes.version + 1, updated_at = now() \
         returning row_to_json(discovery_scopes.*)",
    )
    .bind(network_id)
    .bind(json!(request.excluded_cidrs))
    .bind(scan_profile)
    .bind(target_count)
    .fetch_one(&mut *tx)
    .await;
    let row = match row {
        Ok((row,)) => row,
        Err(error) => return err(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if let Err(error) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "discovery_scope.draft",
        Some("networks"),
        Some(network_id),
        "success",
        Some(json!({"cidr": cidr, "target_count": target_count})),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (StatusCode::CREATED, Json(row))
}

pub async fn confirm(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
    Json(request): Json<ConfirmScope>,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let row: Option<(i64,)> = match sqlx::query_as(
        "select target_count from discovery_scopes where network_id = $1 for update",
    )
    .bind(network_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((target_count,)) = row else {
        return err(StatusCode::NOT_FOUND, "discovery scope draft not found");
    };
    if request.target_count != target_count {
        return err(StatusCode::CONFLICT, "target count does not match draft");
    }
    let scope: (Value,) = match sqlx::query_as(
        "update discovery_scopes set confirmed_at = now(), confirmed_target_count = target_count, \
         version = version + 1, updated_at = now() where network_id = $1 \
         returning row_to_json(discovery_scopes.*)",
    )
    .bind(network_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Err(error) = Recorder::record_audit(
        &mut tx,
        Some(user.0),
        "operator",
        "discovery_scope.confirm",
        Some("networks"),
        Some(network_id),
        "success",
        Some(json!({"target_count": target_count})),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = tx.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    (StatusCode::OK, Json(scope.0))
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

    #[tokio::test]
    async fn state_returns_persisted_scope_and_latest_scan() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let network_name = format!("discovery-state-{}", Uuid::new_v4());
        let (network_id,): (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('198.18.10.0/24', $1) returning id",
        )
        .bind(&network_name)
        .fetch_one(&pool)
        .await
        .expect("insert network fixture");
        sqlx::query(
            "insert into discovery_scopes \
             (network_id, excluded_cidrs, scan_profile, target_count, confirmed_target_count, confirmed_at) \
             values ($1, $2, 'low_impact', 252, 252, now())",
        )
        .bind(network_id)
        .bind(json!(["198.18.10.10/32"]))
        .execute(&pool)
        .await
        .expect("insert scope fixture");
        let (run_id,): (Uuid,) = sqlx::query_as(
            "insert into scan_runs \
             (network_id, kind, status, scope_version, targets_planned, targets_completed, ports_planned, ports_completed) \
             values ($1, 'initial_discovery', 'running', 1, 252, 12, 16_515_420, 786_420) \
             returning id",
        )
        .bind(network_id)
        .fetch_one(&pool)
        .await
        .expect("insert scan fixture");

        let (status, Json(body)) = get_state(
            State(AppState { pool: pool.clone() }),
            Extension(CurrentUser(Uuid::nil())),
            Path(network_id),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scope"]["confirmed_target_count"], json!(252));
        assert_eq!(body["scope"]["scan_profile"], json!("low_impact"));
        assert_eq!(body["scan_run"]["id"], json!(run_id));
        assert_eq!(body["scan_run"]["status"], json!("running"));
        assert_eq!(body["scan_run"]["authoritative"], json!(false));

        sqlx::query("delete from scan_runs where id = $1")
            .bind(run_id)
            .execute(&pool)
            .await
            .expect("delete scan fixture");
        sqlx::query("delete from discovery_scopes where network_id = $1")
            .bind(network_id)
            .execute(&pool)
            .await
            .expect("delete scope fixture");
        sqlx::query("delete from networks where id = $1")
            .bind(network_id)
            .execute(&pool)
            .await
            .expect("delete network fixture");
    }
}
