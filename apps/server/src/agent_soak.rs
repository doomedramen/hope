//! Opt-in, bounded local soak. Uses an empty disposable database exclusively.
use crate::{
    agent_inventory::{self, AgentListQuery},
    agent_logs::{self, LogQuery},
    agent_metrics::{self, MetricQuery},
    state::AppState,
};
use axum::extract::{Path, Query, State};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn distribution(mut values: Vec<f64>) -> Value {
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    json!({"count":values.len(),"p50_ms":values[values.len()/2],"p95_ms":values[(values.len()*95/100).min(values.len()-1)],"p99_ms":values[(values.len()*99/100).min(values.len()-1)],"max_ms":values.last()})
}

fn payload() -> Value {
    json!({
        "cpu":{"usage_percent":35.0,"load_1m":1.4,"load_5m":1.2,"load_15m":1.0,"logical_cores":8},
        "memory":{"used_percent":52.0,"used_bytes":8000000000u64,"total_bytes":16000000000u64,"available_bytes":8000000000u64,"swap_used_bytes":0},
        "network":{"interfaces":(0..4).map(|i|json!({"name":format!("eth{i}"),"rx_bytes_per_sec":100000.0,"tx_bytes_per_sec":20000.0,"rx_packets_per_sec":120.0,"tx_packets_per_sec":30.0,"rx_errors_per_sec":0.0,"tx_errors_per_sec":0.0})).collect::<Vec<_>>()},
        "disk":{"devices":(0..2).map(|i|json!({"name":format!("nvme{i}n1"),"read_iops":40.0,"write_iops":20.0,"read_bytes_per_sec":163840.0,"write_bytes_per_sec":81920.0,"utilization_percent":10.0})).collect::<Vec<_>>()},
        "pressure":{"cpu":{"some_avg10":0.1},"memory":{"some_avg10":0.0,"full_avg10":0.0},"io":{"some_avg10":0.2,"full_avg10":0.1}},
        "delivery":{"metrics":{"queued_records":1,"queued_bytes":2048},"logs":{"queued_records":15,"queued_bytes":8192}}
    })
}

async fn acknowledgements(socket: &mut Socket, kinds: &[&str]) {
    let mut needed = kinds.to_vec();
    // A deadline for the whole exchange prevents unrelated frames extending it forever.
    tokio::time::timeout(Duration::from_secs(30), async {
        while !needed.is_empty() {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                let response: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(response["type"], "protocol_error", "{response}");
                assert_ne!(response["accepted"], false, "{response}");
                if let Some(kind) = response["type"].as_str()
                    && let Some(index) = needed.iter().position(|expected| *expected == kind)
                {
                    needed.swap_remove(index);
                }
            }
        }
    })
    .await
    .expect("acknowledgement deadline exceeded");
}

async fn connect(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    tls: Arc<rustls::ClientConfig>,
    id: Uuid,
    key: &SigningKey,
) -> Socket {
    let challenge: Value = client
        .get(format!("https://{addr}/agent/v1/challenge/{id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let nonce = challenge["nonce"].as_str().unwrap();
    let signature = hex::encode(
        key.sign(format!("hope-agent-connect-v1\n{id}\n{nonce}").as_bytes())
            .to_bytes(),
    );
    let mut request = format!("wss://{addr}/agent/v1/connect")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-hope-agent-id", id.to_string().parse().unwrap());
    request
        .headers_mut()
        .insert("x-hope-challenge", nonce.parse().unwrap());
    request
        .headers_mut()
        .insert("x-hope-signature", signature.parse().unwrap());
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(Connector::Rustls(tls)),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    socket.send(Message::Text(json!({"type":"hello","message_id":Uuid::new_v4(),"protocol_version":2,"agent_id":id,"agent_version":"soak-test","hostname":id.to_string(),"os":"linux","arch":"arm64","capabilities":["resource_metrics","log_streaming"]}).to_string())).await.unwrap();
    acknowledgements(&mut socket, &["hello_ack"]).await;
    socket.send(Message::Text(json!({"type":"capability_offer","message_id":Uuid::new_v4(),"protocol_version":2,"supported_protocol_versions":[2],"capabilities":["resource_metrics","log_streaming"]}).to_string())).await.unwrap();
    acknowledgements(&mut socket, &["capability_ack"]).await;
    socket
}

async fn maintenance(pool: &PgPool) -> (u64, f64) {
    // Old raw samples need real backfill before deletion. Seed across every agent.
    sqlx::query("insert into agent_metric_samples(agent_id,batch_id,sample_id,schema_version,collected_at,received_at,metrics) select id,gen_random_uuid(),gen_random_uuid(),1,now()-interval '8 days',now()-interval '8 days','{\"cpu\":{\"usage_percent\":35}}'::jsonb from agents cross join generate_series(1,2)").execute(pool).await.unwrap();
    sqlx::query("insert into agent_log_entries(agent_id,event_id,observed_at,received_at,source,severity,message,attributes) select id,gen_random_uuid()::text,now()-interval '8 days',now()-interval '8 days','journal:expired.service','info','expired soak fixture','{}' from agents cross join generate_series(1,2)").execute(pool).await.unwrap();
    let started = Instant::now();
    crate::metric_rollups::backfill_and_retain(pool)
        .await
        .unwrap();
    let deleted = crate::inventory::retention::purge_old_agent_metric_samples(pool, 7)
        .await
        .unwrap();
    agent_logs::retention(pool).await.unwrap();
    // Dedicated test DB requires a role allowed to checkpoint, like the Docker fixture.
    sqlx::query("checkpoint").execute(pool).await.unwrap();
    assert_eq!(deleted, 1000);
    (deleted, started.elapsed().as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires HOPE_SOAK_DATABASE_URL pointing to an empty disposable database"]
async fn sustained_500_agent_tls_workload() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url =
        std::env::var("HOPE_SOAK_DATABASE_URL").expect("dedicated disposable database required");
    let rounds: u64 = std::env::var("HOPE_SOAK_ROUNDS")
        .map(|s| s.parse().unwrap())
        .unwrap_or(40);
    assert!(
        (4..=1440).contains(&rounds),
        "4 to 1440 rounds allowed (1 minute to 6 hours)"
    );
    let pool = PgPoolOptions::new()
        .max_connections(32)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let existing: i64 = sqlx::query_scalar("select count(*) from agents")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(existing, 0, "use an empty disposable database");
    let mut identities = Vec::new();
    for _ in 0..500 {
        let id = Uuid::new_v4();
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        sqlx::query("insert into agents(id,cert_fingerprint,cert_serial,public_key_hex) values($1,$2,$2,$3)").bind(id).bind(id.to_string()).bind(hex::encode(key.verifying_key().to_bytes())).execute(&pool).await.unwrap();
        identities.push((id, key));
    }
    // Bounded older history exercises range queries. This is not a full retention dataset.
    sqlx::query("insert into agent_metric_rollups(agent_id,resolution,bucket_start,sample_count,stats) select id,3600,date_trunc('hour',now()-make_interval(days=>day)),240,jsonb_build_object('cpu.usage_percent',jsonb_build_object('count',240,'sum',8400,'average',35,'minimum',10,'maximum',95,'latest',35,'last_at',extract(epoch from now()-make_interval(days=>day))::bigint)) from agents cross join generate_series(1,30) day").execute(&pool).await.unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let server_tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let tls = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let app = axum::Router::new()
        .route(
            "/agent/v1/challenge/{id}",
            axum::routing::get(crate::agent_web::challenge),
        )
        .route(
            "/agent/v1/connect",
            axum::routing::get(crate::agent_web::connect),
        )
        .with_state(AppState { pool: pool.clone() });
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, server_tls)
            .serve(app.into_make_service())
            .await
            .unwrap()
    });
    let before: (i64, String) =
        sqlx::query_as("select pg_database_size(current_database()),pg_current_wal_lsn()::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(501));
    let mut clients = tokio::task::JoinSet::new();
    let agent_ids: Vec<_> = identities.iter().map(|(id, _)| *id).collect();
    for (index, (id, key)) in identities.into_iter().enumerate() {
        let (client, tls, barrier) = (client.clone(), tls.clone(), barrier.clone());
        clients.spawn(async move {
            let mut socket = connect(&client,addr,tls.clone(),id,&key).await;
            barrier.wait().await;
            let started = Instant::now();
            let (mut normal,mut recovery) = (Vec::new(),Vec::new());
            for round in 0..rounds {
                tokio::time::sleep_until(tokio::time::Instant::from_std(started+Duration::from_secs(round*15))).await;
                let start = Instant::now();
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let metric = json!({"type":"metric_sample_batch","message_id":Uuid::new_v4(),"protocol_version":2,"schema_version":1,"agent_id":id,"batch_id":Uuid::new_v4(),"samples":[{"sample_id":Uuid::new_v4(),"collected_at_unix_secs":now,"metrics":payload()}]}).to_string();
                let entries: Vec<_> = (0..if round%10==0 {150} else {15}).map(|n|json!({"event_id":Uuid::new_v4(),"observed_at_unix_ms":now*1000,"source":if n%2==0 {"journal:soak.service"} else {"docker:soak"},"severity":if n%10==0 {"warning"} else {"info"},"message":format!("soak request completed host={index} round={round} {}","x".repeat(300)),"attributes":{"request_id":Uuid::new_v4(),"component":"acceptance"}})).collect();
                let logs: Vec<_> = entries.chunks(protocol::MAX_LOG_RECORDS).map(|entries| json!({"type":"log_batch","message_id":Uuid::new_v4(),"protocol_version":2,"agent_id":id,"batch_id":Uuid::new_v4(),"entries":entries}).to_string()).collect();
                socket.send(Message::Text(metric.clone())).await.unwrap();
                for batch in &logs { socket.send(Message::Text(batch.clone())).await.unwrap(); }
                let reconnect = round>0 && round%10==0;
                if reconnect {
                    // Drop without reading ACKs, then replay the same identities through a new authenticated TLS connection.
                    drop(socket);
                    socket = connect(&client,addr,tls.clone(),id,&key).await;
                    socket.send(Message::Text(metric)).await.unwrap();
                    for batch in &logs { socket.send(Message::Text(batch.clone())).await.unwrap(); }
                }
                socket.send(Message::Text(json!({"type":"heartbeat","message_id":Uuid::new_v4(),"protocol_version":2,"agent_id":id,"uptime_secs":round*15}).to_string())).await.unwrap();
                // A rotating subset reads slowly, without changing the producer cadence.
                if round%10==5 && index%10==(round/10%10) as usize {tokio::time::sleep(Duration::from_secs(2)).await;}
                let mut expected = vec!["log_batch_ack";logs.len()];
                expected.extend(["metric_sample_batch_ack","heartbeat_ack"]);
                acknowledgements(&mut socket,&expected).await;
                let elapsed = start.elapsed().as_secs_f64()*1000.0;
                if reconnect {recovery.push(elapsed)} else {normal.push(elapsed)}
            }
            tokio::time::sleep_until(tokio::time::Instant::from_std(started+Duration::from_secs(rounds*15))).await;
            socket.close(None).await.unwrap();
            (normal,recovery)
        });
    }
    tokio::time::timeout(Duration::from_secs(60), barrier.wait())
        .await
        .expect("500 clients must connect within 60 seconds");
    let started = Instant::now();
    println!(
        "SOAK_STARTED pid={} rounds={rounds} seconds={}",
        std::process::id(),
        rounds * 15
    );
    let query_pool = pool.clone();
    let queries = tokio::spawn(async move {
        let state = AppState { pool: query_pool };
        let (mut fleet, mut history, mut logs) = (Vec::new(), Vec::new(), Vec::new());
        let mut index = 0;
        while started.elapsed() < Duration::from_secs(rounds * 15) {
            let id = agent_ids[index % agent_ids.len()];
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
                    q: Some("soak".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
            logs.push(start.elapsed().as_secs_f64() * 1000.0);
            index += 1;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        json!({"fleet":distribution(fleet),"history":distribution(history),"log_search":distribution(logs)})
    });
    let maintenance_pool = pool.clone();
    let retention = tokio::spawn(async move {
        let (mut deleted, mut times) = (0, Vec::new());
        for cycle in 0..rounds.div_ceil(4) {
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                started + Duration::from_secs(cycle * 60),
            ))
            .await;
            let (count, time) = maintenance(&maintenance_pool).await;
            deleted += count;
            times.push(time);
            println!("SOAK_MAINTENANCE cycle={cycle} deleted={count} elapsed_ms={time:.1}");
        }
        json!({"cycles":times.len(),"expired_metrics_deleted":deleted,"expired_logs_seeded":deleted,"backfill_retention_checkpoint":distribution(times)})
    });
    let (mut normal, mut recovery) = (Vec::new(), Vec::new());
    while let Some(result) = clients.join_next().await {
        let (a, b) = result.unwrap();
        normal.extend(a);
        recovery.extend(b);
    }
    let queries = queries.await.unwrap();
    let retention = retention.await.unwrap();
    let rows: (i64,i64,i64) = sqlx::query_as("select (select count(*) from agent_metric_samples),(select count(*) from agent_log_entries),(select count(*) from agent_log_entries where source='journal:expired.service')").fetch_one(&pool).await.unwrap();
    assert_eq!(
        rows,
        (
            500 * rounds as i64,
            500 * (rounds * 15 + rounds.div_ceil(10) * 135) as i64,
            0
        ),
        "retention and reconnect replay must preserve exactly the generated live records"
    );
    let after: (i64,i64) = sqlx::query_as("select pg_database_size(current_database()),pg_wal_lsn_diff(pg_current_wal_lsn(),$1::pg_lsn)::bigint").bind(before.1).fetch_one(&pool).await.unwrap();
    let summary = json!({"agents":500,"pid":std::process::id(),"rounds":rounds,"elapsed_seconds":started.elapsed().as_secs_f64(),"tls_certificate_verification":true,"metric_payload_bytes":payload().to_string().len(),"metric_rows":rows.0,"log_rows":rows.1,"sampling_seconds":15,"normal_logs_per_agent_per_second":1,"burst_logs_per_agent":150,"seeded_hourly_history_rows":15000,"reconnections":recovery.len(),"normal_ack":distribution(normal),"reconnect_replay_ack":if recovery.is_empty(){Value::Null}else{distribution(recovery)},"queries":queries,"maintenance":retention,"database_growth_bytes":after.0-before.0,"wal_bytes":after.1,"scope":"500 simulated clients and server in one native process; PostgreSQL in Docker Desktop; verified loopback TLS. Bounded historical fixture, not full retained history. Includes tenfold log bursts, lost-ACK reconnect replay, slow readers, retention/backfill and checkpoints."});
    println!("SOAK_RESULT={summary}");
    if let Ok(path) = std::env::var("HOPE_SOAK_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
    }
    server.abort();
    assert!(
        summary["normal_ack"]["p95_ms"].as_f64().unwrap() < 5000.0,
        "normal ACK p95 must stay below 5 seconds"
    );
    if !summary["reconnect_replay_ack"].is_null() {
        assert!(
            summary["reconnect_replay_ack"]["max_ms"].as_f64().unwrap() < 15000.0,
            "reconnect recovery must fit one collection interval"
        );
    }
    for kind in ["fleet", "history", "log_search"] {
        assert!(
            summary["queries"][kind]["p95_ms"].as_f64().unwrap() < 1000.0,
            "{kind} p95 must stay below one second"
        );
    }
}
