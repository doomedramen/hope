//! Outbound update policy and durable operations. Reuses verified release bundles.
use crate::{auth_mw::CurrentUser, release_repository::ReleaseRepository, state::AppState};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error":message})))
}
fn db(e: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::error!(error=%e,"managed update database failure");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Update service temporarily unavailable",
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub mode: String,
    pub channel: String,
    pub pinned_version: Option<String>,
    pub rollout_percent: i32,
    pub window_start_utc: i32,
    pub window_end_utc: i32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: "notify".into(),
            channel: "stable".into(),
            pinned_version: None,
            rollout_percent: 5,
            window_start_utc: 0,
            window_end_utc: 0,
        }
    }
}
impl Policy {
    fn valid(&self) -> bool {
        ["manual", "notify", "automatic"].contains(&self.mode.as_str())
            && ["stable", "canary"].contains(&self.channel.as_str())
            && (0..=100).contains(&self.rollout_percent)
            && (0..24).contains(&self.window_start_utc)
            && (0..24).contains(&self.window_end_utc)
            && self
                .pinned_version
                .as_ref()
                .is_none_or(|v| semver::Version::parse(v).is_ok())
    }
    fn in_window(&self, hour: i32) -> bool {
        if self.window_start_utc == self.window_end_utc {
            true
        } else if self.window_start_utc < self.window_end_utc {
            hour >= self.window_start_utc && hour < self.window_end_utc
        } else {
            hour >= self.window_start_utc || hour < self.window_end_utc
        }
    }
}
async fn policy(pool: &PgPool, agent: Uuid) -> Result<Policy, sqlx::Error> {
    let value: Option<Value> = sqlx::query_scalar(
        "select row_to_json(p) from agent_managed_update_policies p where agent_id=$1",
    )
    .bind(agent)
    .fetch_optional(pool)
    .await?;
    value
        .map(serde_json::from_value)
        .transpose()
        .map(|p| p.unwrap_or_default())
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}
async fn candidate(
    pool: &PgPool,
    agent: Uuid,
    policy: &Policy,
    exact: Option<&str>,
) -> Result<(String, String, Value), String> {
    let row = sqlx::query("select agent_version,os,arch,protocol_version,capabilities from agents where id=$1 and revoked_at is null").bind(agent).fetch_optional(pool).await.map_err(|_|"Agent unavailable")?.ok_or("Active agent not found")?;
    let capabilities: Value = row.get("capabilities");
    if !capabilities
        .as_array()
        .is_some_and(|list| list.iter().any(|c| c == "managed_updates"))
    {
        return Err("Bootstrap required: install a signed agent with the updater service".into());
    }
    let os: String = row.try_get("os").unwrap_or_default();
    let arch: String = row.try_get("arch").unwrap_or_default();
    let arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let protocol: i32 = row.try_get("protocol_version").unwrap_or(2);
    let current: String = row.try_get("agent_version").unwrap_or_default();
    let repo =
        ReleaseRepository::from_environment().map_err(|_| "Verified releases are unavailable")?;
    let release = if let Some(version) = exact.or(policy.pinned_version.as_deref()) {
        repo.load(version)
            .map_err(|_| "Selected release is unavailable or invalid")?
    } else {
        repo.latest_compatible_for_channel(
            &os,
            arch,
            protocol as u32,
            policy.channel.parse().map_err(|_| "Invalid channel")?,
        )
        .map_err(|_| "No compatible release is available")?
        .0
    };
    repo.artifact_for(&release, &os, arch, protocol as u32)
        .map_err(|_| "Release is incompatible")?;
    Ok((
        current,
        release.manifest.version.clone(),
        serde_json::to_value(release.manifest).unwrap(),
    ))
}
pub async fn get(State(state): State<AppState>, Path(agent): Path<Uuid>) -> ApiResult {
    let policy = policy(&state.pool, agent).await.map_err(db)?;
    let candidate = candidate(&state.pool, agent, &policy, None).await;
    let operations: Vec<Value> = sqlx::query_scalar("select row_to_json(t) from (select id,target_version,previous_version,state,transport,progress,last_error,rollback_reason,created_at,started_at,finished_at from agent_update_operations where agent_id=$1 order by created_at desc limit 50) t").bind(agent).fetch_all(&state.pool).await.map_err(db)?;
    match candidate {
        Ok((current, target, manifest)) => {
            let reason = automatic_block(&state.pool, agent, &policy, &target)
                .await
                .map_err(db)?;
            Ok(Json(
                json!({"policy":policy,"current_version":current,"target_version":target,"manifest":manifest,"operations":operations,"blocked_reason":reason}),
            ))
        }
        Err(reason) => Ok(Json(
            json!({"policy":policy,"operations":operations,"blocked_reason":reason,"target_version":null}),
        )),
    }
}
pub async fn put(
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
    Extension(CurrentUser(actor)): Extension<CurrentUser>,
    Json(policy): Json<Policy>,
) -> ApiResult {
    if !policy.valid() {
        return Err(error(StatusCode::BAD_REQUEST, "Invalid update policy"));
    }
    let mut tx = state.pool.begin().await.map_err(db)?;
    sqlx::query("insert into agent_managed_update_policies(agent_id,mode,channel,pinned_version,rollout_percent,window_start_utc,window_end_utc,updated_by) values($1,$2,$3,$4,$5,$6,$7,$8) on conflict(agent_id) do update set mode=$2,channel=$3,pinned_version=$4,rollout_percent=$5,window_start_utc=$6,window_end_utc=$7,updated_by=$8,updated_at=now()")
        .bind(agent).bind(&policy.mode).bind(&policy.channel).bind(&policy.pinned_version).bind(policy.rollout_percent).bind(policy.window_start_utc).bind(policy.window_end_utc).bind(actor).execute(&mut *tx).await.map_err(db)?;
    sqlx::query("update agent_update_operations set state='cancelled',finished_at=now(),updated_at=now() where agent_id=$1 and transport='agent' and state='pending' and created_by is null").bind(agent).execute(&mut *tx).await.map_err(db)?;
    crate::inventory::events::Recorder::record_audit(
        &mut tx,
        Some(actor),
        "operator",
        "agent.update.policy",
        Some("agents"),
        Some(agent),
        "success",
        Some(json!(policy)),
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Json(json!({"policy":policy})))
}
#[derive(Deserialize)]
pub struct Request {
    pub version: String,
}
pub async fn start(
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
    Extension(CurrentUser(actor)): Extension<CurrentUser>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> ApiResult {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Idempotency-Key is required"))?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "select id from agent_update_operations where agent_id=$1 and request_key=$2",
    )
    .bind(agent)
    .bind(key)
    .fetch_optional(&state.pool)
    .await
    .map_err(db)?;
    if let Some(id) = existing {
        return Ok(Json(json!({"operation_id":id})));
    }
    let policy = policy(&state.pool, agent).await.map_err(db)?;
    let (current, target, _) = candidate(&state.pool, agent, &policy, Some(&request.version))
        .await
        .map_err(|reason| error(StatusCode::CONFLICT, &reason))?;
    if current == target {
        return Err(error(
            StatusCode::CONFLICT,
            "Agent already runs this version",
        ));
    }
    let id = enqueue(
        &state.pool,
        agent,
        &current,
        &target,
        Some(actor),
        Some(key),
    )
    .await
    .map_err(db)?;
    Ok(Json(json!({"operation_id":id})))
}
async fn enqueue(
    pool: &PgPool,
    agent: Uuid,
    current: &str,
    target: &str,
    actor: Option<Uuid>,
    key: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("select pg_advisory_xact_lock(17037)")
        .execute(&mut *tx)
        .await?;
    if let Some(id) = sqlx::query_scalar::<_,Uuid>("select id from agent_update_operations where agent_id=$1 and state in ('pending','verifying','downloading','installing','restarting','awaiting_health')").bind(agent).fetch_optional(&mut *tx).await? { return Ok(id); }
    let id: Uuid = sqlx::query_scalar("insert into agent_update_operations(agent_id,target_version,previous_version,transport,created_by,request_key) values($1,$2,$3,'agent',$4,$5) returning id")
        .bind(agent).bind(target).bind(current).bind(actor).bind(key).fetch_one(&mut *tx).await?;
    crate::inventory::events::Recorder::record_audit(
        &mut tx,
        actor,
        if actor.is_some() {
            "operator"
        } else {
            "system"
        },
        "agent.update.requested",
        Some("agents"),
        Some(agent),
        "success",
        Some(json!({"operation_id":id,"version":target,"transport":"agent"})),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}
async fn automatic_block(
    pool: &PgPool,
    agent: Uuid,
    policy: &Policy,
    target: &str,
) -> Result<Option<String>, sqlx::Error> {
    if policy.mode != "automatic" {
        return Ok(None);
    }
    if !policy.in_window(i32::from(time::OffsetDateTime::now_utc().hour())) {
        return Ok(Some("Outside automatic update window (UTC)".into()));
    }
    let rank = (agent.as_u128() % 100) as i32;
    if rank >= policy.rollout_percent {
        return Ok(Some("Outside selected rollout percentage".into()));
    }
    let failed: bool = sqlx::query_scalar("select exists(select 1 from agent_update_operations where transport='agent' and target_version=$1 and (state in ('failed','rolled_back') or (state in ('downloading','installing','restarting','awaiting_health') and deadline_at < now())) and created_at > now()-interval '24 hours')").bind(target).fetch_one(pool).await?;
    if failed {
        return Ok(Some("Automatic rollout paused for 24 hours after a failure; explicit retry remains available".into()));
    }
    if rank >= 5 {
        let canary: bool = sqlx::query_scalar("select exists(select 1 from agent_update_operations where transport='agent' and target_version=$1 and state='succeeded' and finished_at < now()-interval '10 minutes')").bind(target).fetch_one(pool).await?;
        if !canary {
            return Ok(Some(
                "Waiting for a healthy canary and 10-minute observation period".into(),
            ));
        }
    }
    Ok(None)
}
pub async fn next_command(
    pool: &PgPool,
    agent: Uuid,
) -> anyhow::Result<Option<protocol::UpdateCommand>> {
    let policy = policy(pool, agent).await?;
    if policy.mode == "automatic"
        && let Ok((current, target, _)) = candidate(pool, agent, &policy, None).await
        && current != target
        && automatic_block(pool, agent, &policy, &target)
            .await?
            .is_none()
    {
        // Never retry a failed version automatically, even after the fleet pause expires.
        let attempted: bool = sqlx::query_scalar("select exists(select 1 from agent_update_operations where agent_id=$1 and target_version=$2 and transport='agent')").bind(agent).bind(&target).fetch_one(pool).await?;
        if !attempted {
            enqueue(pool, agent, &current, &target, None, None).await?;
        }
    }
    let mut tx = pool.begin().await?;
    sqlx::query("select pg_advisory_xact_lock(17037)")
        .execute(&mut *tx)
        .await?;
    let row: Option<(Uuid,String,String,Option<Uuid>)> = sqlx::query_as("select id,target_version,state,created_by from agent_update_operations where agent_id=$1 and transport='agent' and state in ('pending','downloading') order by created_at limit 1 for update").bind(agent).fetch_optional(&mut *tx).await?;
    let Some((operation_id, version, state, actor)) = row else {
        return Ok(None);
    };
    if state == "pending" {
        if actor.is_none()
            && (policy.mode != "automatic"
                || automatic_block(pool, agent, &policy, &version)
                    .await?
                    .is_some())
        {
            return Ok(None);
        }
        let active: i64 = sqlx::query_scalar("select count(*) from agent_update_operations where transport='agent' and state in ('downloading','installing','restarting') and deadline_at > now()").fetch_one(&mut *tx).await?;
        if active >= 3 {
            return Ok(None);
        }
        if candidate(pool, agent, &policy, Some(&version))
            .await
            .is_err()
        {
            return Ok(None);
        }
        begin_delivery(&mut tx, operation_id).await?;
    }
    tx.commit().await?;
    Ok(Some(protocol::UpdateCommand {
        operation_id,
        version,
        deadline_seconds: 120,
    }))
}
async fn begin_delivery(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query("update agent_update_operations set state='downloading',started_at=now(),deadline_at=now()+interval '10 minutes',updated_at=now(),progress=jsonb_build_object('required_log_sources',coalesce((select jsonb_object_agg(key,value) from jsonb_each(coalesce((select metrics #> '{delivery,logs,sources}' from agent_metric_samples where agent_id=agent_update_operations.agent_id order by collected_at desc limit 1),'{}'::jsonb)) where jsonb_typeof(value)='number' and value::text::numeric > extract(epoch from now()-interval '5 minutes')),'{}'::jsonb)) where id=$1").bind(operation_id).execute(&mut **tx).await?;
    Ok(())
}
pub async fn report(
    pool: &PgPool,
    agent: Uuid,
    report: &protocol::UpdateReport,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        [
            "downloading",
            "installing",
            "restarting",
            "awaiting_health",
            "failed",
            "rolled_back"
        ]
        .contains(&report.state.as_str()),
        "invalid update report"
    );
    let detail: String = report.detail.chars().take(512).collect();
    sqlx::query("update agent_update_operations set state=$3,progress=progress || $4,last_error=case when $3 in ('failed','rolled_back') then $5 else last_error end,rollback_reason=case when $3='rolled_back' then $5 else rollback_reason end,finished_at=case when $3 in ('failed','rolled_back') then now() else null end,updated_at=now() where id=$1 and agent_id=$2 and transport='agent' and state not in ('succeeded','failed','rolled_back','cancelled')")
        .bind(report.operation_id).bind(agent).bind(&report.state).bind(json!({"phase":report.state,"detail":detail})).bind(&detail).execute(pool).await?;
    Ok(())
}
pub async fn verify_health(pool: &PgPool, agent: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("update agent_update_operations o set state='succeeded',finished_at=now(),updated_at=now() from agents a where o.agent_id=$1 and a.id=o.agent_id and o.transport='agent' and o.state in ('restarting','awaiting_health') and a.agent_version=o.target_version and a.last_heartbeat_at > o.started_at and exists(select 1 from agent_metric_samples m where m.agent_id=a.id and m.collected_at>o.started_at and m.received_at>o.started_at and not exists(select 1 from jsonb_object_keys(coalesce(o.progress->'required_log_sources','{}'::jsonb)) source where coalesce((m.metrics #> '{delivery,logs,sources}' ->> source)::numeric,0) <= extract(epoch from o.started_at)))").bind(agent).execute(pool).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn durable_operation_requires_resumed_streams_and_ignores_late_reports() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let agent = Uuid::new_v4();
        sqlx::query("insert into agents(id,cert_fingerprint,cert_serial,agent_version,last_heartbeat_at) values($1,$2,$2,'0.1.1',now())").bind(agent).bind(agent.to_string()).execute(&pool).await.unwrap();
        let at = time::OffsetDateTime::now_utc().unix_timestamp();
        let sample = |at, sources| json!({"message_id":Uuid::new_v4(),"protocol_version":2,"type":"metric_sample_batch","schema_version":1,"agent_id":agent,"batch_id":Uuid::new_v4(),"samples":[{"sample_id":Uuid::new_v4(),"collected_at_unix_secs":at,"metrics":{"cpu":{"usage_percent":10},"delivery":{"logs":{"sources":sources}}}}]});
        let parsed = crate::agent_metrics::parse_metric_sample_batch_value(sample(
            at,
            json!({"journal:app.service":at}),
        ))
        .unwrap();
        crate::agent_metrics::ingest_metric_sample_batch(&pool, agent, 2, &parsed)
            .await
            .unwrap();
        let id = enqueue(&pool, agent, "0.1.1", "0.1.2", None, Some("request-1"))
            .await
            .unwrap();
        assert_eq!(
            id,
            enqueue(&pool, agent, "0.1.1", "0.1.2", None, Some("request-1"))
                .await
                .unwrap()
        );
        let mut tx = pool.begin().await.unwrap();
        begin_delivery(&mut tx, id).await.unwrap();
        tx.commit().await.unwrap();
        let progress: Value =
            sqlx::query_scalar("select progress from agent_update_operations where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(progress["required_log_sources"]["journal:app.service"], at);
        report(
            &pool,
            agent,
            &protocol::UpdateReport {
                operation_id: id,
                state: "awaiting_health".into(),
                detail: "Local collection resumed".into(),
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "update agent_update_operations set started_at=now()-interval '2 seconds' where id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("update agents set agent_version='0.1.2',last_heartbeat_at=now() where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        // Missing log health cannot complete an otherwise healthy update.
        sqlx::query("update agent_metric_samples set metrics='{\"cpu\":{\"usage_percent\":10}}' where agent_id=$1").bind(agent).execute(&pool).await.unwrap();
        verify_health(&pool, agent).await.unwrap();
        let state: String =
            sqlx::query_scalar("select state from agent_update_operations where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "awaiting_health");
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let parsed = crate::agent_metrics::parse_metric_sample_batch_value(sample(
            now,
            json!({"journal:app.service":now}),
        ))
        .unwrap();
        crate::agent_metrics::ingest_metric_sample_batch(&pool, agent, 2, &parsed)
            .await
            .unwrap();
        verify_health(&pool, agent).await.unwrap();
        report(
            &pool,
            agent,
            &protocol::UpdateReport {
                operation_id: id,
                state: "installing".into(),
                detail: "late report".into(),
            },
        )
        .await
        .unwrap();
        let state: String =
            sqlx::query_scalar("select state from agent_update_operations where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "succeeded");
        sqlx::query("delete from agents where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
    }
    #[test]
    fn overnight_windows_and_policy_bounds() {
        let mut p = Policy {
            window_start_utc: 22,
            window_end_utc: 6,
            ..Default::default()
        };
        assert!(p.in_window(23));
        assert!(p.in_window(5));
        assert!(!p.in_window(12));
        p.rollout_percent = 101;
        assert!(!p.valid());
    }
}
