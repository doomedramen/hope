//! Opt-in database/handler benchmark. Never run against a production database.
use crate::{
    agent_inventory::{self, AgentListQuery},
    agent_logs::{self, LogQuery},
    agent_metrics::{self, MetricQuery},
    state::AppState,
};
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

fn p95(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * 95 / 100).min(values.len() - 1)]
}
fn metrics() -> Value {
    json!({"cpu":{"usage_percent":35.0},"memory":{"used_percent":52.0,"used_bytes":4000000000u64},"network":{"interfaces":[{"name":"eth0","rx_bytes_per_sec":10000.0,"tx_bytes_per_sec":2000.0}]},"disk":{"devices":[{"name":"sda","read_iops":40.0,"utilization_percent":10.0}]}})
}
#[tokio::test]
#[ignore = "requires HOPE_CAPACITY_DATABASE_URL pointing to an empty disposable database"]
async fn mixed_500_agent_workload() {
    let url = std::env::var("HOPE_CAPACITY_DATABASE_URL")
        .expect("dedicated disposable database required");
    let pool = PgPoolOptions::new()
        .max_connections(24)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let existing: i64 = sqlx::query_scalar("select count(*) from agents")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(existing, 0, "use an empty disposable database");
    let agents: Vec<Uuid> = (0..500).map(|_| Uuid::new_v4()).collect();
    for id in &agents {
        sqlx::query("insert into agents(id,cert_fingerprint,cert_serial,hostname,last_heartbeat_at,agent_version) values($1,$2,$2,$2,now(),'capacity-test')").bind(id).bind(id.to_string()).execute(&pool).await.unwrap();
    }
    let before: (i64, String) =
        sqlx::query_as("select pg_database_size(current_database()),pg_current_wal_lsn()::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let latency = Arc::new(Mutex::new(Vec::<f64>::new()));
    let permits = Arc::new(Semaphore::new(16));
    let state = AppState { pool: pool.clone() };
    let query_agents = agents.clone();
    let query_task = tokio::spawn(async move {
        let mut fleet = Vec::new();
        let mut history = Vec::new();
        let mut logs = Vec::new();
        for index in 0..120 {
            let id = query_agents[index % query_agents.len()];
            let start = Instant::now();
            let (status, _) = agent_inventory::list_agents(
                State(state.clone()),
                Query(AgentListQuery {
                    cursor: None,
                    limit: Some(100),
                    q: None,
                    status: None,
                }),
            )
            .await;
            assert_eq!(status, 200);
            fleet.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            let (status, _) = agent_metrics::get_agent_metrics(
                State(state.clone()),
                Path(id),
                Query(MetricQuery {
                    range: Some("180d".into()),
                }),
            )
            .await;
            assert_eq!(status, 200);
            history.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            let _ = agent_logs::list(
                State(state.clone()),
                Path(id),
                Query(LogQuery {
                    q: Some("capacity".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
            logs.push(start.elapsed().as_secs_f64() * 1000.0);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        json!({"fleet_p95_ms":p95(&mut fleet),"history_p95_ms":p95(&mut history),"logs_p95_ms":p95(&mut logs)})
    });
    let started = Instant::now();
    for round in 0..5 {
        if round < 4 {
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                started + Duration::from_secs(round * 15),
            ))
            .await;
        }
        let mut tasks = tokio::task::JoinSet::new();
        for id in &agents {
            let (id, pool, latency, permits) =
                (*id, pool.clone(), latency.clone(), permits.clone());
            tasks.spawn(async move {
                let start=Instant::now();
                let _permit=permits.acquire().await.unwrap();
                let now=time::OffsetDateTime::now_utc().unix_timestamp();
                let sample_count=if round==4 {30} else {1};
                let samples:Vec<Value>=(0..sample_count).map(|index| json!({"sample_id":Uuid::new_v4(),"collected_at_unix_secs":now-index*15,"metrics":metrics()})).collect();
                let parsed=agent_metrics::parse_metric_sample_batch_value(json!({"message_id":Uuid::new_v4(),"protocol_version":2,"type":"metric_sample_batch","schema_version":1,"agent_id":id,"batch_id":Uuid::new_v4(),"samples":samples})).unwrap();
                agent_metrics::ingest_metric_sample_batch(&pool,id,2,&parsed).await.unwrap();
                let batch=protocol::LogBatch {agent_id:id,batch_id:Uuid::new_v4(),entries:(0..if round==4 {30} else {15}).map(|_|protocol::LogEntry {event_id:Uuid::new_v4().to_string(),observed_at_unix_ms:now*1000,source:"journal:capacity.service".into(),severity:"info".into(),message:format!("capacity workload {}", "x".repeat(180)),attributes:json!({})}).collect()};
                batch.validate().unwrap();
                agent_logs::ingest(&pool,id,&batch).await.unwrap();
                if round==4 { // A lost acknowledgement must not duplicate stored records.
                    agent_metrics::ingest_metric_sample_batch(&pool,id,2,&parsed).await.unwrap();
                    agent_logs::ingest(&pool,id,&batch).await.unwrap();
                }
                latency.lock().await.push(start.elapsed().as_secs_f64()*1000.0);
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    }
    let query_result = query_task.await.unwrap();
    let count:(i64,i64)=sqlx::query_as("select (select count(*) from agent_metric_samples),(select count(*) from agent_log_entries)").fetch_one(&pool).await.unwrap();
    assert_eq!(count, (17000, 45000));
    let after:(i64,i64)=sqlx::query_as("select pg_database_size(current_database()),pg_wal_lsn_diff(pg_current_wal_lsn(),$1::pg_lsn)::bigint").bind(before.1).fetch_one(&pool).await.unwrap();
    let mut latency = latency.lock().await;
    let summary = json!({"agents":500,"normal_metric_interval_seconds":15,"normal_log_events_per_agent_per_second":1,"normal_rounds":4,"reconnect_samples_per_agent":30,"metric_rows":count.0,"log_rows":count.1,"elapsed_seconds":started.elapsed().as_secs_f64(),"mixed_ingest_p95_ms_including_queue_and_replay":p95(&mut latency),"queries":query_result,"database_growth_bytes":after.0-before.0,"wal_bytes":after.1,"scope":"In-process production ingest and API handlers, PostgreSQL on Docker Desktop. Excludes TLS/WebSocket connections and browser rendering. Short workload, not sustained production capacity."});
    println!("CAPACITY_RESULT={summary}");
    if let Ok(path) = std::env::var("HOPE_CAPACITY_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
    }
}
