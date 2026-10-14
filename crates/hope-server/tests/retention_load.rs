//! Explicit, reproducible load test; no LAN traffic or external service installation.
use axum::{Json, extract::State as Extract, http::HeaderMap};
use hope_protocol::{AgentReport, Capabilities, EnrichmentStatus, Host, Metrics, SCHEMA_VERSION};
use hope_server::{
    agents, metrics,
    state::{State, hash, now, secret},
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[tokio::test]
#[ignore = "Seeds 1.15 million samples; run explicitly in release mode"]
async fn seven_days_for_250_hosts_keep_reports_and_discovery_writes_responsive() {
    let directory = tempfile::tempdir().expect("temporary database");
    let database = directory.path().join("load.db");
    let state = State::open(
        database.to_str().expect("path"),
        "http://localhost:8080".into(),
        "releases".into(),
        Capabilities {
            platform: std::env::consts::OS.into(),
            discovery_mode: "limited".into(),
            arp: false,
            icmp: false,
            limitations: vec![],
        },
    )
    .await
    .expect("state");
    let mut identities = vec![];
    let mut tx = state.0.db.begin().await.expect("transaction");
    for index in 0..250 {
        let id = Uuid::new_v4().to_string();
        let credential = secret();
        let host = Host {
            id: Uuid::new_v4().to_string(),
            agent_id: Some(id.clone()),
            name: format!("load-{index:03}"),
            addresses: vec![],
            macs: vec![],
            network_ids: vec![],
            last_seen: now(),
            last_report: None,
            last_collected_at: None,
            agent_revoked: false,
            enrichment_status: EnrichmentStatus::Completed,
            discovery_method: "agent".into(),
            inventory: None,
            metrics: None,
            collectors: vec![],
            services: vec![],
        };
        sqlx::query("INSERT INTO agents(id,credential_hash) VALUES(?,?)")
            .bind(&id)
            .bind(hash(&credential))
            .execute(&mut *tx)
            .await
            .expect("agent");
        sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?)")
            .bind(&host.id)
            .bind(&id)
            .bind(&host.name)
            .bind(serde_json::to_string(&host).expect("host json"))
            .execute(&mut *tx)
            .await
            .expect("host");
        identities.push((id, credential));
    }
    let reference = now();
    let cutoff = ((reference - 86400.0) / 300.0).floor() * 300.0;
    let raw_start = ((reference - 86400.0) / 30.0).floor() * 30.0;
    let body = serde_json::to_string(&Metrics {
        cpu_percent: Some(42.0),
        ..Default::default()
    })
    .expect("metrics json");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<2879) INSERT INTO metrics(agent_id,report_id,collected_at,body) SELECT id,'raw-' || x,?+x*30,? FROM agents CROSS JOIN series").bind(raw_start).bind(&body).execute(&mut *tx).await.expect("24 hours raw");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<1727) INSERT INTO metrics(agent_id,report_id,collected_at,resolution,body,statistics) SELECT id,'rollup-' || CAST((?+x*300)/300 AS INTEGER),?+x*300,300,?,? FROM agents CROSS JOIN series")
        .bind(cutoff-6.0*86400.0).bind(cutoff-6.0*86400.0).bind(&body).bind(r#"{"cpu_percent":{"sum":420.0,"count":10}}"#).execute(&mut *tx).await.expect("six days aggregates");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<9) INSERT INTO metrics(agent_id,report_id,collected_at,body) SELECT id,'late-' || x,?+x*30,? FROM agents CROSS JOIN series").bind(cutoff-300.0).bind(&body).execute(&mut *tx).await.expect("retention work");
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('load-scan','fixture','198.18.1.0/24','running',254,?)").bind(reference).execute(&mut *tx).await.expect("scan fixture");
    tx.commit().await.expect("seed commit");
    let seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
        .fetch_one(&state.0.db)
        .await
        .expect("count");
    assert_eq!(seeded, 1_154_500);
    let stop = Arc::new(AtomicBool::new(false));
    let latencies = Arc::new(Mutex::new(vec![]));
    let discovery_writes = Arc::new(AtomicUsize::new(0));
    let reporter = {
        let state = state.clone();
        let stop = stop.clone();
        let latencies = latencies.clone();
        tokio::spawn(async move {
            let mut index = 0;
            while !stop.load(Ordering::SeqCst) {
                let (agent, credential) = &identities[index % identities.len()];
                index += 1;
                let report = AgentReport {
                    schema_version: SCHEMA_VERSION,
                    report_id: Uuid::new_v4().to_string(),
                    agent_id: agent.clone(),
                    collected_at: now(),
                    inventory: None,
                    metrics: Metrics::default(),
                    collectors: vec![],
                };
                let mut headers = HeaderMap::new();
                headers.insert(
                    "authorization",
                    format!("Bearer {credential}").parse().expect("header"),
                );
                let start = Instant::now();
                let response = agents::report(
                    Extract(state.clone()),
                    axum::extract::Extension(hope_server::limits::ClientKey("test".into())),
                    headers,
                    Json(report),
                )
                .await
                .expect("live report");
                assert!(response.0.accepted);
                latencies
                    .lock()
                    .await
                    .push(start.elapsed().as_secs_f64() * 1000.0);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    let discovery = {
        let state = state.clone();
        let stop = stop.clone();
        let writes = discovery_writes.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                {
                    let _guard = state.0.writes.lock().await;
                    sqlx::query("UPDATE scans SET scanned=(scanned+1)%255 WHERE id='load-scan'")
                        .execute(&state.0.db)
                        .await
                        .expect("discovery persistence");
                    writes.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let start = Instant::now();
    let mut calls = 0;
    loop {
        metrics::retain(&state).await.expect("retention");
        calls += 1;
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM metrics WHERE resolution=30 AND collected_at<?",
        )
        .bind(cutoff)
        .fetch_one(&state.0.db)
        .await
        .expect("remaining");
        if remaining == 0 {
            break;
        }
        assert!(
            calls < 20 && start.elapsed() < Duration::from_secs(30),
            "Bounded cleanup must make progress"
        );
    }
    let retention_seconds = start.elapsed().as_secs_f64();
    stop.store(true, Ordering::SeqCst);
    reporter.await.expect("reporter");
    discovery.await.expect("discovery");
    let mut latencies = latencies.lock().await.clone();
    latencies.sort_by(f64::total_cmp);
    assert!(!latencies.is_empty());
    assert!(discovery_writes.load(Ordering::SeqCst) > 0);
    let maximum = *latencies.last().expect("latency");
    assert!(
        maximum < 250.0,
        "Live reports must not wait behind the entire cleanup: {maximum}ms"
    );
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&state.0.db)
        .await
        .expect("checkpoint");
    let result = json!({"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"profile":if cfg!(debug_assertions) { "debug" } else { "release" },"hosts":250,"seeded_samples":seeded,"raw_hours":24,"aggregate_days":6,"retention_calls":calls,"retention_seconds":retention_seconds,"concurrent_reports":latencies.len(),"report_latency_median_ms":latencies[latencies.len()/2],"report_latency_max_ms":maximum,"concurrent_discovery_writes":discovery_writes.load(Ordering::SeqCst),"database_bytes":tokio::fs::metadata(&database).await.expect("size").len()});
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/retention-benchmark.json");
    tokio::fs::write(output, serde_json::to_vec_pretty(&result).expect("json"))
        .await
        .expect("results");
    println!("{result}");
    state.0.db.close().await;
}
