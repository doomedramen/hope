// Diagnostic probes asserting the defects observed at e564e16.
// Copy beside tests/api.rs and run only the review_probe test filter.
// These assert faulty behavior; they are not release acceptance tests.
include!("api.rs");

#[tokio::test]
async fn review_probe_source_get_and_unlink() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)").execute(&state.0.db).await.unwrap();
    let bearer = format!("Bearer {credential}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(child_batch(&agent, 1, 1, true, now(), vec![child_device()])),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let source_id: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{source_id}"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut scanned = Host::new("review-scan".into(), "scanned".into(), "tcp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{source_id}/association"),
        json!({"action":"confirm", "host_id":"review-scan"}),
        Some(("cookie", &cookie)),
    )
    .await;
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{source_id}/association"),
        json!({"action":"unlink", "host_id":"review-scan"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let projected = state.hosts().await.unwrap();
    let linked = projected
        .iter()
        .find(|host| host.id == "review-scan")
        .unwrap();
    assert!(linked.source_device_id.is_some());
    println!(
        "REVIEW unlink then bootstrap: source still attached={}, associations={:?}",
        linked.source_device_id.is_some(),
        linked.associations
    );
    // Exercise exactly the projection-save pattern used by scanner/refresh_hosts.
    state.save_host(linked).await.unwrap();
    request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{source_id}/association"),
        json!({"action":"reject", "host_id":"review-scan"}),
        Some(("cookie", &cookie)),
    )
    .await;
    let projected = state.hosts().await.unwrap();
    assert_eq!(
        projected
            .iter()
            .filter(|host| host.source_device_id.as_deref() == Some(source_id.as_str()))
            .count(),
        2
    );
    println!(
        "REVIEW projection saved then rejected: {} displayed rows carry source id",
        projected
            .iter()
            .filter(|host| host.source_device_id.as_deref() == Some(source_id.as_str()))
            .count()
    );
}

#[tokio::test]
async fn review_probe_future_report_freezes_live_state() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let base = now();
    for (at, cpu) in [(base + 240.0, 10.0), (base, 99.0)] {
        let report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: agent.clone(),
            collected_at: at,
            inventory: None,
            metrics: Metrics {
                cpu_percent: Some(cpu),
                ..Default::default()
            },
            collectors: vec![],
        };
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &bearer)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let host = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.agent_id.as_deref() == Some(agent.as_str()))
        .unwrap();
    assert_eq!(host.metrics.as_ref().unwrap().cpu_percent, Some(10.0));
    println!(
        "REVIEW latest CPU after accepted 99% sample = {:?}; watermark ahead {:.0}s",
        host.metrics.unwrap().cpu_percent,
        host.last_collected_at.unwrap() - base
    );
}

#[tokio::test]
async fn review_probe_failed_notification_is_never_retried() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_dir, state, _app, _) = setup().await;
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let ntfy = Router::new().fallback(axum::routing::post(move || {
        let count = requests.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            StatusCode::SERVICE_UNAVAILABLE
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, ntfy).await.unwrap() });
    let settings = NotificationSettings {
        ntfy_enabled: true,
        ntfy_server: format!("http://{address}"),
        ntfy_topic: "review".into(),
        notify_offline: true,
        ..Default::default()
    };
    state
        .set_meta("notifications", &serde_json::to_string(&settings).unwrap())
        .await
        .unwrap();
    let mut host = Host::new("silent-host".into(), "Silent".into(), "agent");
    host.agent_id = Some("silent-agent".into());
    host.last_report = Some(now() - 1000.0);
    state.save_host(&host).await.unwrap();
    for _ in 0..3 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    println!(
        "REVIEW notification HTTP requests after 3 evaluations and persistent 503: {}",
        count.load(Ordering::SeqCst)
    );
    server.abort();
}

#[tokio::test]
async fn review_probe_sse_and_bootstrap_disagree_about_source() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let host_id: String = sqlx::query_scalar("SELECT id FROM hosts WHERE agent_id=?")
        .bind(&agent)
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let bearer = format!("Bearer {credential}");
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)").execute(&state.0.db).await.unwrap();
    let initial = AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.clone(),
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: vec![],
    };
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(initial),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut child = child_device();
    child.interfaces = inventory().interfaces;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(child_batch(&agent, 1, 1, true, now(), vec![child])),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|host| host.id == host_id)
        .unwrap();
    assert!(before.child.is_some());
    let mut events = state.0.events.subscribe();
    let report = AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent,
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: vec![],
    };
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/report",
        json!(report),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hope_server::state::Change::Host(body) = events.recv().await.unwrap() else {
        panic!("expected Host event")
    };
    let live: Host = serde_json::from_str(&body).unwrap();
    assert!(live.child.is_none());
    println!(
        "REVIEW same device: bootstrap child={}, SSE child={}, SSE source id={:?}",
        before.child.is_some(),
        live.child.is_some(),
        live.source_device_id
    );
}
