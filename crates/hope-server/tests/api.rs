use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use hope_protocol::*;
use hope_server::{
    agents, metrics, router,
    state::{State, hash, now},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup() -> (TempDir, State, Router, String) {
    let dir = tempfile::tempdir().expect("temporary database");
    let state = State::open(
        dir.path().join("test.db").to_str().expect("path"),
        "http://localhost:8080".into(),
        "releases".into(),
        Capabilities {
            platform: "test".into(),
            discovery_mode: "limited".into(),
            arp: false,
            icmp: false,
            limitations: vec![],
        },
    )
    .await
    .expect("state");
    state
        .set_meta("setup_token_hash", &hash("test-setup-token-long-enough"))
        .await
        .expect("setup token");
    let app = router(state.clone(), "web/out");
    let response=app.clone().oneshot(Request::builder().method("POST").uri("/api/v1/auth/setup").header("content-type","application/json").body(Body::from(json!({"password":"a-long-test-password","setup_token":"test-setup-token-long-enough"}).to_string())).expect("request")).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("session")
        .to_string();
    (dir, state, app, cookie)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    credential: Option<(&str, &str)>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some((name, value)) = credential {
        builder = builder.header(name, value);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn inventory() -> Inventory {
    Inventory {
        hostname: "test-host".into(),
        os: "Linux test".into(),
        architecture: "aarch64".into(),
        cpu_model: "Test CPU".into(),
        cpu_cores: 4,
        memory_total_bytes: 8e9,
        interfaces: vec![Interface {
            name: "eth0".into(),
            mac: Some("02:00:00:00:00:01".into()),
            addresses: vec!["192.168.1.10".into()],
            gateway: Some("192.168.1.1".into()),
        }],
        disks: vec![],
        gpus: vec![],
        collectors: vec![],
    }
}
async fn enrolled(state: &State, app: &Router) -> (String, String, String) {
    let token = agents::issue_token(state).await.expect("token");
    let agent = Uuid::new_v4().to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/agent/enroll",
        json!(EnrollmentRequest {
            schema_version: 1,
            token: token.clone(),
            agent_id: agent.clone(),
            credential: hope_server::state::secret(),
            inventory: inventory()
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        agent,
        body["credential"].as_str().expect("credential").into(),
        token,
    )
}
#[tokio::test]
async fn authentication_setup_and_cross_site_protection() {
    let (_dir, state, app, cookie) = setup().await;
    assert_eq!(
        request(&app, "GET", "/api/v1/bootstrap", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/bootstrap",
            Value::Null,
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        state
            .meta("setup_token_hash")
            .await
            .expect("meta")
            .is_none()
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/setup",
            json!({"password":"another-password","setup_token":"test-setup-token-long-enough"}),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/networks")
                .header("origin", "https://evil.example")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name":"LAN","cidr":"192.168.1.0/24"}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn token_is_single_use_and_expired_token_cannot_enroll() {
    let (_dir, state, app, _) = setup().await;
    let (_, _, token) = enrolled(&state, &app).await;
    let input = EnrollmentRequest {
        schema_version: 1,
        token,
        agent_id: Uuid::new_v4().to_string(),
        credential: hope_server::state::secret(),
        inventory: inventory(),
    };
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(input), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let token = agents::issue_token(&state).await.expect("token");
    sqlx::query("UPDATE enrollment_tokens SET expires_at=0 WHERE token_hash=?")
        .bind(hash(&token))
        .execute(&state.0.db)
        .await
        .expect("expire token");
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/enroll",
            json!(EnrollmentRequest {
                schema_version: 1,
                token,
                agent_id: Uuid::new_v4().to_string(),
                credential: hope_server::state::secret(),
                inventory: inventory()
            }),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn reporting_deduplicates_and_keeps_identity_after_dhcp_and_rejects_revocation() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    let mut inv = inventory();
    inv.interfaces[0].addresses = vec!["192.168.1.22".into()];
    let mut report = AgentReport {
        schema_version: 1,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.clone(),
        collected_at: now(),
        inventory: Some(inv),
        metrics: Metrics {
            cpu_percent: Some(42.0),
            ..Default::default()
        },
        collectors: vec![],
    };
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    let duplicate = request(
        &app,
        "POST",
        "/api/v1/agent/report",
        json!(report),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(duplicate.1["duplicate"], true);
    let hosts = state.hosts().await.expect("hosts");
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, host_id);
    assert_eq!(hosts[0].addresses, vec!["192.168.1.22"]);
    report.report_id = Uuid::new_v4().to_string();
    report.collected_at -= 300.0;
    report.metrics.cpu_percent = Some(10.0);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        state
            .host(&host_id)
            .await
            .expect("host")
            .expect("exists")
            .metrics
            .expect("metrics")
            .cpu_percent,
        Some(42.0)
    );
    let history = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{host_id}/metrics?hours=1"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(history.1.as_array().expect("history").len(), 2);
    request(
        &app,
        "POST",
        &format!("/api/v1/agents/{agent}/revoke"),
        json!({}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn retention_averages_old_samples_and_deletes_expired_history() {
    let (_dir, state, _app, _) = setup().await;
    let old = ((now() - 90000.0) / 300.0).floor() * 300.0;
    for (id, time, cpu) in [
        ("one", old + 10.0, Some(10.0)),
        ("two", old + 40.0, Some(30.0)),
        ("missing", old + 70.0, None),
        ("expired", now() - 8.0 * 86400.0, Some(90.0)),
    ] {
        sqlx::query(
            "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent',?,?,?)",
        )
        .bind(id)
        .bind(time)
        .bind(
            serde_json::to_string(&Metrics {
                cpu_percent: cpu,
                ..Default::default()
            })
            .expect("json"),
        )
        .execute(&state.0.db)
        .await
        .expect("insert");
    }
    metrics::retain(&state).await.expect("retention");
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT resolution,body FROM metrics")
        .fetch_all(&state.0.db)
        .await
        .expect("metrics");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 300);
    assert_eq!(
        serde_json::from_str::<Metrics>(&rows[0].1)
            .expect("metrics")
            .cpu_percent,
        Some(20.0)
    );
}
#[tokio::test]
async fn deleting_a_network_keeps_hosts_and_cancels_its_scan() {
    let (_dir, state, app, cookie) = setup().await;
    let (status, created) = request(
        &app,
        "POST",
        "/api/v1/networks",
        json!({"name":"LAN","cidr":"192.168.1.0/24","enabled":false}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().expect("id").to_string();
    // An enrolled agent inside the range is a member of it.
    let (agent, _credential, _token) = enrolled(&state, &app).await;
    let host = state
        .hosts()
        .await
        .expect("hosts")
        .into_iter()
        .find(|h| h.agent_id.as_deref() == Some(&agent))
        .expect("agent host");
    assert_eq!(host.network_ids, vec![id.clone()]);
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('active','net-to-delete','192.168.1.0/24','running',254,?)")
        .bind(now()).execute(&state.0.db).await.expect("scan");
    sqlx::query("UPDATE scans SET network_id=? WHERE id='active'")
        .bind(&id)
        .execute(&state.0.db)
        .await
        .expect("scan network");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/networks/{id}"),
        json!({}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, bootstrap) = request(
        &app,
        "GET",
        "/api/v1/bootstrap",
        json!({}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        bootstrap["networks"]
            .as_array()
            .expect("networks")
            .is_empty()
    );
    assert!(bootstrap["scans"].as_array().expect("scans").is_empty());
    let hosts = bootstrap["hosts"].as_array().expect("hosts");
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["network_ids"], json!([]));
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/networks/{id}"),
            json!({}),
            Some(("cookie", &cookie)),
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/networks/{id}"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn network_validation_and_restart_recovery() {
    let (dir, state, app, cookie) = setup().await;
    for cidr in ["0.0.0.0/0", "192.168.0.0/16", "not-a-cidr", "224.0.0.0/24"] {
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/v1/networks",
                json!({"name":"Bad","cidr":cidr}),
                Some(("cookie", &cookie))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/networks",
            json!({"name":"LAN","cidr":"192.168.1.12/24"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/networks",
            json!({"name":"Duplicate","cidr":"192.168.1.0/24"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('interrupted','net','192.168.1.0/24','running',254,?)").bind(now()).execute(&state.0.db).await.expect("scan");
    let reopened = State::open(
        dir.path().join("test.db").to_str().expect("path"),
        "http://localhost:8080".into(),
        "releases".into(),
        state.0.capabilities.clone(),
    )
    .await
    .expect("reopen");
    let status: String = sqlx::query_scalar("SELECT status FROM scans WHERE id='interrupted'")
        .fetch_one(&reopened.0.db)
        .await
        .expect("status");
    assert_eq!(status, "interrupted");
}

#[tokio::test]
async fn invalid_schema_and_metrics_are_rejected_without_storage() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let mut report = AgentReport {
        schema_version: 999,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent,
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: vec![],
    };
    let result = request(
        &app,
        "POST",
        "/api/v1/agent/report",
        json!(report),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(result.0, StatusCode::BAD_REQUEST);
    assert_eq!(result.1["code"], "invalid_request");
    report.schema_version = SCHEMA_VERSION;
    report.metrics.cpu_percent = Some(101.0);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
        .fetch_one(&state.0.db)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn enrollment_retry_recovers_lost_response_without_duplicate_identity() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, token) = enrolled(&state, &app).await;
    let mut body = EnrollmentRequest {
        schema_version: SCHEMA_VERSION,
        token,
        agent_id: agent,
        credential: credential.clone(),
        inventory: inventory(),
    };
    let result = request(&app, "POST", "/api/v1/agent/enroll", json!(body), None).await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["credential"], credential);
    assert_eq!(state.hosts().await.expect("hosts").len(), 1);
    body.credential = hope_server::state::secret();
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(body), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn concurrent_enrollment_uses_token_for_only_one_identity() {
    let (_dir, state, app, _) = setup().await;
    let token = agents::issue_token(&state).await.expect("token");
    let first = json!(EnrollmentRequest {
        schema_version: SCHEMA_VERSION,
        token: token.clone(),
        agent_id: Uuid::new_v4().to_string(),
        credential: hope_server::state::secret(),
        inventory: inventory()
    });
    let second = json!(EnrollmentRequest {
        schema_version: SCHEMA_VERSION,
        token,
        agent_id: Uuid::new_v4().to_string(),
        credential: hope_server::state::secret(),
        inventory: inventory()
    });
    let (one, two) = tokio::join!(
        request(&app, "POST", "/api/v1/agent/enroll", first, None),
        request(&app, "POST", "/api/v1/agent/enroll", second, None)
    );
    assert!(
        (one.0 == StatusCode::OK && two.0 == StatusCode::UNAUTHORIZED)
            || (two.0 == StatusCode::OK && one.0 == StatusCode::UNAUTHORIZED)
    );
    assert_eq!(state.hosts().await.expect("hosts").len(), 1);
}

#[tokio::test]
async fn queued_report_cannot_commit_after_revocation() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let held = state.0.writes.lock().await;
    let revoke = {
        let app = app.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            request(
                &app,
                "POST",
                &format!("/api/v1/agents/{agent}/revoke"),
                json!({}),
                Some(("cookie", &cookie)),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let report = {
        let app = app.clone();
        tokio::spawn(async move {
            request(
                &app,
                "POST",
                "/api/v1/agent/report",
                json!(AgentReport {
                    schema_version: SCHEMA_VERSION,
                    report_id: Uuid::new_v4().to_string(),
                    agent_id: agent,
                    collected_at: now(),
                    inventory: None,
                    metrics: Metrics::default(),
                    collectors: vec![]
                }),
                Some(("authorization", &format!("Bearer {credential}"))),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    drop(held);
    assert_eq!(revoke.await.expect("revoke").0, StatusCode::OK);
    assert_eq!(report.await.expect("report").0, StatusCode::UNAUTHORIZED);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
        .fetch_one(&state.0.db)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn revoked_identity_requires_bound_token_and_new_secret_for_recovery() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, old, old_token) = enrolled(&state, &app).await;
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    request(
        &app,
        "POST",
        &format!("/api/v1/agents/{agent}/revoke"),
        json!({}),
        Some(("cookie", &cookie)),
    )
    .await;
    let mut body = EnrollmentRequest {
        schema_version: SCHEMA_VERSION,
        token: old_token,
        agent_id: agent.clone(),
        credential: old.clone(),
        inventory: inventory(),
    };
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(body), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let token = agents::issue_token_for_reset(&state, Some(&agent))
        .await
        .expect("reset token");
    body.token = token;
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(body), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    body.credential = hope_server::state::secret();
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(body), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "POST", "/api/v1/agent/enroll", json!(body), None)
            .await
            .0,
        StatusCode::OK
    );
    let hosts = state.hosts().await.expect("hosts");
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, host_id);
    assert!(!hosts[0].agent_revoked);
    let report = AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent,
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
            json!(report),
            Some(("authorization", &format!("Bearer {old}")))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &format!("Bearer {}", body.credential)))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn retention_merges_late_samples_without_replacing_completed_bucket() {
    let (_dir, state, _, _) = setup().await;
    let old = ((now() - 90000.0) / 300.0).floor() * 300.0;
    for (id, time, cpu) in [("first", old + 10.0, 10.0), ("second", old + 30.0, 30.0)] {
        sqlx::query(
            "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent',?,?,?)",
        )
        .bind(id)
        .bind(time)
        .bind(
            serde_json::to_string(&Metrics {
                cpu_percent: Some(cpu),
                ..Default::default()
            })
            .expect("json"),
        )
        .execute(&state.0.db)
        .await
        .expect("insert");
    }
    metrics::retain(&state).await.expect("first cleanup");
    sqlx::query(
        "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent','late',?,?)",
    )
    .bind(old + 50.0)
    .bind(
        serde_json::to_string(&Metrics {
            cpu_percent: Some(80.0),
            ..Default::default()
        })
        .expect("json"),
    )
    .execute(&state.0.db)
    .await
    .expect("late insert");
    metrics::retain(&state).await.expect("merge");
    metrics::retain(&state).await.expect("repeat");
    let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM metrics")
        .fetch_all(&state.0.db)
        .await
        .expect("body");
    assert_eq!(bodies.len(), 1);
    assert_eq!(
        serde_json::from_str::<Metrics>(&bodies[0])
            .expect("metrics")
            .cpu_percent,
        Some(40.0)
    );
}

fn discovered(id: &str, address: &str, mac: &str) -> Host {
    serde_json::from_value(json!({"id":id,"agent_id":null,"name":id,"addresses":[address],"macs":[mac],"network_ids":[],"last_seen":now(),"last_report":null,"discovery_method":"arp","inventory":null,"metrics":null,"collectors":[],"services":[{"port":22,"name":"SSH","observed_at":now()}]})).expect("discovered host")
}

#[tokio::test]
async fn enrollment_requires_unique_mac_evidence_not_an_ip_match() {
    let (_dir, state, app, _) = setup().await;
    let source = discovered("same-ip-other-device", "192.168.1.10", "02:00:00:00:00:02");
    state.save_host(&source).await.expect("source");
    let (agent, _, _) = enrolled(&state, &app).await;
    let hosts = state.hosts().await.expect("hosts");
    assert_eq!(hosts.len(), 2);
    assert!(
        hosts
            .iter()
            .find(|h| h.id == source.id)
            .expect("source")
            .agent_id
            .is_none()
    );
    assert_ne!(
        hosts
            .iter()
            .find(|h| h.agent_id.as_ref() == Some(&agent))
            .expect("agent")
            .id,
        source.id
    );

    let (_dir, state, app, _) = setup().await;
    let source = discovered("same-device", "192.168.1.99", "02:00:00:00:00:01");
    state.save_host(&source).await.expect("source");
    let (agent, _, _) = enrolled(&state, &app).await;
    let hosts = state.hosts().await.expect("hosts");
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, source.id);
    assert_eq!(hosts[0].agent_id.as_ref(), Some(&agent));
    assert_eq!(hosts[0].addresses, vec!["192.168.1.10"]);
}

#[tokio::test]
async fn ambiguous_mac_requires_manual_merge_and_preserves_history_and_map() {
    let (_dir, state, app, cookie) = setup().await;
    for host in [
        discovered("source", "192.168.1.10", "02:00:00:00:00:01"),
        discovered("other", "192.168.1.11", "02:00:00:00:00:01"),
    ] {
        state.save_host(&host).await.expect("source");
    }
    let (agent, credential, _) = enrolled(&state, &app).await;
    let hosts = state.hosts().await.expect("hosts");
    assert_eq!(hosts.len(), 3);
    let target = hosts
        .iter()
        .find(|h| h.agent_id.as_ref() == Some(&agent))
        .expect("target")
        .id
        .clone();
    let report = AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.clone(),
        collected_at: now(),
        inventory: None,
        metrics: Metrics {
            cpu_percent: Some(42.0),
            ..Default::default()
        },
        collectors: vec![],
    };
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/report",
            json!(report),
            Some(("authorization", &format!("Bearer {credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let map = MapState {
        revision: 0,
        positions: ["topology", "subnets", "health"]
            .into_iter()
            .map(|mode| Position {
                id: format!("{mode}:source"),
                x: 1.0,
                y: 2.0,
            })
            .chain(std::iter::once(Position {
                id: format!("health:{target}"),
                x: 3.0,
                y: 4.0,
            }))
            .collect(),
        links: vec![MapLink {
            id: "manual".into(),
            source: "source".into(),
            target: "other".into(),
            label: "Known relationship".into(),
        }],
    };
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/map",
            json!(map),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/v1/hosts/{target}/merge"),
            json!({"source_id":"source"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(state.hosts().await.expect("hosts").len(), 2);
    let map: MapState =
        serde_json::from_str(&state.meta("map").await.expect("map").expect("saved"))
            .expect("map json");
    assert_eq!(map.positions.len(), 1);
    assert_eq!(map.links[0].source, target);
    assert_eq!(map.links[0].target, "other");
    let host = state.host(&target).await.expect("host").expect("exists");
    assert_eq!(host.agent_id.as_ref(), Some(&agent));
    assert_eq!(host.metrics.expect("metrics").cpu_percent, Some(42.0));
    assert!(host.services.iter().any(|s| s.port == 22));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics WHERE agent_id=?")
        .bind(agent)
        .fetch_one(&state.0.db)
        .await
        .expect("history");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn independent_map_edits_are_atomic_and_retryable() {
    let (_directory, state, app, cookie) = setup().await;
    let (agent_id, _, _) = enrolled(&state, &app).await;
    let host_id = state
        .hosts()
        .await
        .expect("hosts")
        .into_iter()
        .find(|host| host.agent_id.as_deref() == Some(&agent_id))
        .expect("enrolled host")
        .id;
    let mut other = state.host(&host_id).await.expect("query").expect("host");
    other.id = Uuid::new_v4().to_string();
    other.agent_id = None;
    state.save_host(&other).await.expect("other host");
    let mut edits = tokio::task::JoinSet::new();
    for index in 0..30 {
        let app = app.clone();
        let cookie = cookie.clone();
        edits.spawn(async move {
            request(&app, "PATCH", "/api/v1/map", json!({"operation":"set_position","position":{"id":format!("topology:node-{index}"),"x":index,"y":0}}), Some(("cookie", &cookie))).await.0
        });
    }
    while let Some(result) = edits.join_next().await {
        assert_eq!(result.expect("edit task"), StatusCode::OK);
    }
    let link = json!({"operation":"add_link","link":{"id":"manual-fixture","source":host_id,"target":other.id,"label":"Known cable"}});
    for _ in 0..2 {
        let (status, map) = request(
            &app,
            "PATCH",
            "/api/v1/map",
            link.clone(),
            Some(("cookie", &cookie)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(map["positions"].as_array().expect("positions").len(), 30);
        assert_eq!(map["links"].as_array().expect("links").len(), 1);
    }
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/map",
            json!({"revision":0,"positions":[],"links":[]}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut conflicting = link;
    conflicting["link"]["label"] = json!("Different meaning");
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/map",
            conflicting,
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, map) = request(
        &app,
        "PATCH",
        "/api/v1/map",
        json!({"operation":"remove_link","id":"manual-fixture"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(map["positions"].as_array().expect("positions").len(), 30);
    assert!(map["links"].as_array().expect("links").is_empty());
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/map",
            json!({"operation":"set_position","position":{"id":"bad","x":1e7,"y":0}}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
