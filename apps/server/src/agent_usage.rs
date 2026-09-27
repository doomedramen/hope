//! Bounded estimates from recent payloads; deliberately separate from physical disk allocation.
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn get_usage(
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match estimate(&state.pool, agent).await {
        Ok(Some(value)) => (StatusCode::OK, Json(value)),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"Agent not found"})),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Usage estimate unavailable"})),
        ),
    }
}
async fn estimate(pool: &sqlx::PgPool, agent: Uuid) -> anyhow::Result<Option<Value>> {
    let exists: bool = sqlx::query_scalar("select exists(select 1 from agents where id=$1)")
        .bind(agent)
        .fetch_one(pool)
        .await?;
    if !exists {
        return Ok(None);
    }
    let raw_days = crate::config::Config::load()?.agent_metric_retention_days;
    let log_days: i32=sqlx::query_scalar("select coalesce((select log_retention_days from agent_collection_settings where agent_id=$1),7)").bind(agent).fetch_one(pool).await?;
    let metric: (i64,Option<f64>)=sqlx::query_as("select count(*),avg(bytes)::float8 from (select octet_length(metrics::text) as bytes from agent_metric_samples where agent_id=$1 and received_at >= now()-interval '1 hour' order by received_at desc limit 240) s").bind(agent).fetch_one(pool).await?;
    let logs: Vec<(bool,i64,Option<f64>,Option<f64>)>=sqlx::query_as("select diagnostic,count(*),avg(bytes)::float8,extract(epoch from now()-min(received_at))::float8 from (select source='agent' as diagnostic,octet_length(message)+octet_length(attributes::text)+octet_length(source) as bytes,received_at from agent_log_entries where agent_id=$1 and received_at >= now()-interval '1 hour' order by received_at desc limit 1000) s group by diagnostic").bind(agent).fetch_all(pool).await?;
    let streams: Vec<Value>=logs.into_iter().map(|(diagnostic,count,average,seconds)| {
        let days=if diagnostic {30} else {log_days};
        let daily=seconds.filter(|seconds|*seconds>=60.0).map(|seconds|count as f64 / seconds*86400.0*average.unwrap_or(0.0));
        json!({"stream":if diagnostic {"agent diagnostics"} else {"host and container logs"},"sample_count":count,"window_seconds":seconds,"daily_payload_bytes":daily,"retention_days":days,"retained_payload_bytes":daily.map(|daily|daily*days as f64)})
    }).collect();
    Ok(Some(
        json!({"metrics":{"sample_count":metric.0,"average_payload_bytes":metric.1,"daily_payload_bytes":metric.1.map(|bytes|bytes*5760.0),"retained_payload_bytes":metric.1.map(|bytes|bytes*5760.0*raw_days as f64)},"logs":streams,"retention":{"raw_metric_days":raw_days,"five_minute_days":30,"hourly_days":180,"host_log_days":log_days,"diagnostic_days":30},"basis":"Recent payload samples only; excludes row overhead, indexes, rollups, WAL, backups and compression. Metrics assume 15-second collection. Log estimates use recent receipt rate and may reflect replay bursts."}),
    ))
}
