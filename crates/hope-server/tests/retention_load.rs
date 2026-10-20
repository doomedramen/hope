//! Explicit, reproducible load test; no LAN traffic or external service installation.
use axum::{Json, extract::State as Extract, http::HeaderMap};
use hope_protocol::{
    AgentReport, Capabilities, ChildMetrics, Container, Disk, DiskIo, EnrichmentStatus, Host,
    Metrics, NetworkCounter, SCHEMA_VERSION, Temperature,
};
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
#[ignore = "Seeds 1.15 million host samples plus source history; run explicitly in release mode"]
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
        let mut host = Host::new(
            Uuid::new_v4().to_string(),
            format!("load-{index:03}"),
            "agent",
        );
        host.agent_id = Some(id.clone());
        host.last_seen = now();
        host.enrichment_status = EnrichmentStatus::Completed;
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
    let rich_metrics = Metrics {
        cpu_percent: Some(42.0),
        memory_used_bytes: Some(3.0 * 1024.0 * 1024.0 * 1024.0),
        memory_total_bytes: Some(8.0 * 1024.0 * 1024.0 * 1024.0),
        network_rx_bytes_per_second: Some(1200.0),
        network_tx_bytes_per_second: Some(850.0),
        network_interfaces: vec![NetworkCounter {
            name: "eth0".into(),
            received_bytes: 1_000_000.0,
            transmitted_bytes: 800_000.0,
            rx_bytes_per_second: Some(1200.0),
            tx_bytes_per_second: Some(850.0),
            included_in_total: true,
        }],
        disk_read_bytes_per_second: Some(4096.0),
        disk_write_bytes_per_second: Some(8192.0),
        disks: vec![Disk {
            name: "nvme0n1p2".into(),
            mount: "/".into(),
            filesystem: "ext4".into(),
            total_bytes: 64.0 * 1024.0 * 1024.0 * 1024.0,
            available_bytes: 41.0 * 1024.0 * 1024.0 * 1024.0,
        }],
        disk_io: vec![DiskIo {
            device: "nvme0n1".into(),
            read_bytes_per_second: Some(4096.0),
            write_bytes_per_second: Some(8192.0),
            busy_percent: Some(3.0),
            root: true,
        }],
        temperatures: vec![Temperature {
            sensor: "nvme0/composite".into(),
            label: "NVMe Composite".into(),
            celsius: 39.0,
        }],
        ..Default::default()
    };
    let body = serde_json::to_string(&rich_metrics).expect("metrics json");
    let container_body = serde_json::to_string(&Metrics {
        containers: vec![Container {
            id: "container-01".into(),
            name: "api".into(),
            image: "example/api:1".into(),
            state: "running".into(),
            status: "Up".into(),
            ports: "8080/tcp".into(),
            created_at: Some(reference - 86400.0),
            health: Some("healthy".into()),
            compose_project: Some("hope".into()),
            compose_service: Some("api".into()),
            networks: vec![],
            published_ports: vec![],
            restart_count: Some(0),
            cpu_percent: Some(4.2),
            cpu_capacity_cores: Some(1.0),
            memory_used_bytes: Some(96.0 * 1024.0 * 1024.0),
            memory_limit_bytes: Some(512.0 * 1024.0 * 1024.0),
            memory_percent: Some(18.75),
            network_rx_bytes: Some(2_000_000.0),
            network_tx_bytes: Some(1_000_000.0),
            block_read_bytes: Some(4_000_000.0),
            block_write_bytes: Some(2_000_000.0),
            pids: Some(12),
        }],
        ..rich_metrics.clone()
    })
    .expect("container metrics json");
    let source_device_body = serde_json::json!({
        "identity": {
            "kind": "proxmox_vm",
            "scope": "load-scope",
            "key": "guest",
            "incarnation": "initial"
        },
        "name": "load-guest",
        "lifecycle": "running"
    })
    .to_string();
    let source_metrics_body = serde_json::to_string(&ChildMetrics {
        cpu_percent: Some(25.0),
        memory_used_bytes: Some(1024.0 * 1024.0 * 1024.0),
        memory_total_bytes: Some(4.0 * 1024.0 * 1024.0 * 1024.0),
        disk_read_bytes: Some(10_000_000.0),
        disk_write_bytes: Some(8_000_000.0),
        disk_read_bytes_per_second: Some(2048.0),
        disk_write_bytes_per_second: Some(1024.0),
        uptime_seconds: Some(86_400.0),
        network_rx_bytes_per_second: Some(300.0),
        network_tx_bytes_per_second: Some(200.0),
        ..Default::default()
    })
    .expect("source metrics json");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<2879) INSERT INTO metrics(agent_id,report_id,collected_at,body) SELECT id,'raw-' || x,?+x*30,? FROM agents CROSS JOIN series").bind(raw_start).bind(&body).execute(&mut *tx).await.expect("24 hours raw");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<1727) INSERT INTO metrics(agent_id,report_id,collected_at,resolution,body,statistics) SELECT id,'rollup-' || CAST((?+x*300)/300 AS INTEGER),?+x*300,300,?,? FROM agents CROSS JOIN series")
        .bind(cutoff-6.0*86400.0).bind(cutoff-6.0*86400.0).bind(&body).bind(r#"{"cpu_percent":{"sum":420.0,"count":10}}"#).execute(&mut *tx).await.expect("six days aggregates");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<9) INSERT INTO metrics(agent_id,report_id,collected_at,body) SELECT a.id,'late-' || x,?+x*30,CASE WHEN CAST(substr(h.name,6) AS INTEGER)%10=0 THEN ? ELSE ? END FROM agents a JOIN hosts h ON h.agent_id=a.id CROSS JOIN series")
        .bind(cutoff - 300.0)
        .bind(&container_body)
        .bind(&body)
        .execute(&mut *tx)
        .await
        .expect("retention work");
    sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,body) SELECT a.id,'container-' || x,?+x*30,? FROM agents a JOIN hosts h ON h.agent_id=a.id CROSS JOIN (SELECT 0 AS x UNION ALL SELECT 1) WHERE CAST(substr(h.name,6) AS INTEGER)%10=0")
        .bind(raw_start + 600.0)
        .bind(&container_body)
        .execute(&mut *tx)
        .await
        .expect("container history");
    sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,host_id,lifecycle,name,observed_at,inventory_generation,missing_complete_count,missing_since,missing_inventory_generation,body) SELECT 'source-'||a.id,a.id,'proxmox','load-scope','guest-'||a.id,'initial','proxmox_vm',h.id,'running',h.name||' guest',?,1,0,NULL,1,? FROM agents a JOIN hosts h ON h.agent_id=a.id")
        .bind(reference)
        .bind(&source_device_body)
        .execute(&mut *tx)
        .await
        .expect("source devices");
    sqlx::query("INSERT INTO child_batches(batch_id,parent_agent_id,source,source_scope,sequence,inventory_generation,observed_at,complete,received_at) SELECT 'load-batch-'||id,id,'proxmox','load-scope',1,1,?,1,? FROM agents")
        .bind(reference)
        .bind(reference)
        .execute(&mut *tx)
        .await
        .expect("source batches");
    sqlx::query("INSERT INTO source_observations(source_device_id,batch_id,observed_at,inventory_generation,body) SELECT 'source-'||id,'load-batch-'||id,?,1,? FROM agents")
        .bind(reference)
        .bind(&source_device_body)
        .execute(&mut *tx)
        .await
        .expect("source observations");
    sqlx::query("WITH RECURSIVE series(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM series WHERE x<9) INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) SELECT 'source-'||id,'source-raw-'||x,?+x*30,30,? FROM agents CROSS JOIN series")
        .bind(cutoff - 300.0)
        .bind(&source_metrics_body)
        .execute(&mut *tx)
        .await
        .expect("source metric history");
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('load-scan','fixture','198.18.1.0/24','running',254,?)").bind(reference).execute(&mut *tx).await.expect("scan fixture");
    tx.commit().await.expect("seed commit");
    let seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
        .fetch_one(&state.0.db)
        .await
        .expect("count");
    assert_eq!(seeded, 1_154_550);
    let seeded_source: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics")
        .fetch_one(&state.0.db)
        .await
        .expect("source metric count");
    assert_eq!(seeded_source, 2500);
    let stop = Arc::new(AtomicBool::new(false));
    let latencies = Arc::new(Mutex::new(vec![]));
    let discovery_writes = Arc::new(AtomicUsize::new(0));
    let history_reads = Arc::new(AtomicUsize::new(0));
    let history_latencies = Arc::new(Mutex::new(vec![]));
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
                    sequence: None,
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
    let history_reader = {
        let state = state.clone();
        let stop = stop.clone();
        let reads = history_reads.clone();
        let latencies = history_latencies.clone();
        let host_id: String = sqlx::query_scalar("SELECT id FROM hosts ORDER BY name LIMIT 1")
            .fetch_one(&state.0.db)
            .await
            .expect("history host");
        tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                let start = Instant::now();
                let host_history = metrics::history(
                    Extract(state.clone()),
                    axum::extract::Path(host_id.clone()),
                    axum::extract::Query(metrics::Window { hours: Some(24) }),
                )
                .await
                .expect("host history");
                assert!(!host_history.0.is_empty(), "host history is seeded");
                let source_history = metrics::source_history(
                    Extract(state.clone()),
                    axum::extract::Path(host_id.clone()),
                    axum::extract::Query(metrics::Window { hours: Some(168) }),
                )
                .await
                .expect("source history");
                assert!(!source_history.0.is_empty(), "source history is seeded");
                latencies
                    .lock()
                    .await
                    .push(start.elapsed().as_secs_f64() * 1000.0);
                reads.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
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
        let source_remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM source_metrics WHERE resolution=30 AND observed_at<?",
        )
        .bind(cutoff)
        .fetch_one(&state.0.db)
        .await
        .expect("remaining source metrics");
        if remaining == 0 && source_remaining == 0 {
            break;
        }
        assert!(
            calls < 20 && start.elapsed() < Duration::from_secs(30),
            "Bounded cleanup must make progress"
        );
    }
    let retention_seconds = start.elapsed().as_secs_f64();
    let source_rollups: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics WHERE resolution=300")
            .fetch_one(&state.0.db)
            .await
            .expect("source rollups");
    assert_eq!(source_rollups, 250);
    stop.store(true, Ordering::SeqCst);
    reporter.await.expect("reporter");
    discovery.await.expect("discovery");
    history_reader.await.expect("history reader");
    let mut latencies = latencies.lock().await.clone();
    latencies.sort_by(f64::total_cmp);
    assert!(!latencies.is_empty());
    assert!(discovery_writes.load(Ordering::SeqCst) > 0);
    assert!(history_reads.load(Ordering::SeqCst) > 0);
    let mut history_latency_values = history_latencies.lock().await.clone();
    history_latency_values.sort_by(f64::total_cmp);
    let maximum = *latencies.last().expect("latency");
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&state.0.db)
        .await
        .expect("checkpoint");
    let result = json!({"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"profile":if cfg!(debug_assertions) { "debug" } else { "release" },"hosts":250,"seeded_host_samples":seeded,"seeded_source_samples":seeded_source,"raw_hours":24,"aggregate_days":6,"hosts_with_container_metrics":25,"retention_calls":calls,"retention_seconds":retention_seconds,"concurrent_reports":latencies.len(),"report_latency_median_ms":latencies[latencies.len()/2],"report_latency_max_ms":maximum,"concurrent_discovery_writes":discovery_writes.load(Ordering::SeqCst),"concurrent_history_reads":history_reads.load(Ordering::SeqCst),"history_read_pair_median_ms":history_latency_values[history_latency_values.len()/2],"history_read_pair_max_ms":history_latency_values.iter().copied().fold(0.0, f64::max),"database_bytes":tokio::fs::metadata(&database).await.expect("size").len()});
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/retention-benchmark.json");
    tokio::fs::write(output, serde_json::to_vec_pretty(&result).expect("json"))
        .await
        .expect("results");
    println!("{result}");
    state.0.db.close().await;
}
