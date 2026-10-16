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
async fn password_change_requires_the_current_password_and_ends_other_sessions() {
    let (_dir, _state, app, cookie) = setup().await;
    let change = |body: Value, credential: Option<String>| {
        let app = app.clone();
        async move {
            let mut builder = Request::builder()
                .method("POST")
                .uri("/api/v1/account/password")
                .header("content-type", "application/json");
            if let Some(cookie) = credential {
                builder = builder.header("cookie", cookie);
            }
            let response = app
                .oneshot(builder.body(Body::from(body.to_string())).expect("request"))
                .await
                .expect("response");
            let status = response.status();
            let fresh = response.headers().get("set-cookie").map(|v| {
                v.to_str()
                    .expect("cookie")
                    .split(';')
                    .next()
                    .expect("session")
                    .to_string()
            });
            (status, fresh)
        }
    };
    let login = |password: &'static str| {
        let app = app.clone();
        async move {
            request(
                &app,
                "POST",
                "/api/v1/auth/login",
                json!({"password":password,"setup_token":null}),
                None,
            )
            .await
            .0
        }
    };
    // A second browser signed in with the old password.
    let other = {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"password":"a-long-test-password","setup_token":null}).to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        response.headers()["set-cookie"]
            .to_str()
            .expect("cookie")
            .split(';')
            .next()
            .expect("session")
            .to_string()
    };

    // Not signed in: refused before anything else.
    let body =
        json!({"current_password":"a-long-test-password","new_password":"a-brand-new-password"});
    assert_eq!(change(body.clone(), None).await.0, StatusCode::UNAUTHORIZED);
    // Wrong current password: refused, nothing changes.
    assert_eq!(
        change(
            json!({"current_password":"not-the-password","new_password":"a-brand-new-password"}),
            Some(cookie.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // Too short, and unchanged, new passwords are refused.
    assert_eq!(
        change(
            json!({"current_password":"a-long-test-password","new_password":"short"}),
            Some(cookie.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        change(json!({"current_password":"a-long-test-password","new_password":"a-long-test-password"}), Some(cookie.clone())).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(login("a-long-test-password").await, StatusCode::OK);

    let (status, fresh) = change(body, Some(cookie.clone())).await;
    assert_eq!(status, StatusCode::OK);
    let fresh = fresh.expect("a fresh session for this browser");
    assert_ne!(fresh, cookie);
    for (session, expected) in [
        (&cookie, StatusCode::UNAUTHORIZED),
        (&other, StatusCode::UNAUTHORIZED),
        (&fresh, StatusCode::OK),
    ] {
        assert_eq!(
            request(
                &app,
                "GET",
                "/api/v1/bootstrap",
                Value::Null,
                Some(("cookie", session))
            )
            .await
            .0,
            expected
        );
    }
    assert_eq!(
        login("a-long-test-password").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(login("a-brand-new-password").await, StatusCode::OK);
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

type Seen = std::sync::Arc<tokio::sync::Mutex<Vec<(Option<String>, Value)>>>;
/// A stand-in ntfy server that records each publish (authorization header and JSON body).
async fn fake_ntfy(status: u16) -> (String, Seen) {
    let seen: Seen = Default::default();
    let record = seen.clone();
    let app = Router::new().route(
        "/",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
            let record = record.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                record
                    .lock()
                    .await
                    .push((auth, serde_json::from_str(&body).unwrap_or(Value::Null)));
                StatusCode::from_u16(status).expect("status")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), seen)
}

fn settings(server: &str, token: Value) -> Value {
    json!({"ntfy_enabled":true,"ntfy_server":server,"ntfy_topic":"hope-alerts","ntfy_token":token,
        "ntfy_token_set":false,"notify_new_devices":true,"notify_offline":true,"notify_resources":true,
        "notify_recovery":true,"last_sent_at":null,"last_error":null})
}

#[tokio::test]
async fn notification_settings_keep_the_token_secret_and_test_messages_reach_ntfy() {
    let (_dir, _state, app, cookie) = setup().await;
    let auth = Some(("cookie", cookie.as_str()));
    let (server, seen) = fake_ntfy(200).await;
    let (status, saved) = request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, json!("tk_secret")),
        auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["ntfy_token"], Value::Null);
    assert_eq!(saved["ntfy_token_set"], true);
    let (_, read) = request(&app, "GET", "/api/v1/notifications", Value::Null, auth).await;
    assert_eq!(
        (read["ntfy_token"].clone(), read["ntfy_topic"].clone()),
        (Value::Null, json!("hope-alerts"))
    );

    let (status, _) = request(&app, "POST", "/api/v1/notifications/test", json!({}), auth).await;
    assert_eq!(status, StatusCode::OK);
    let published = seen.lock().await.clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0.as_deref(), Some("Bearer tk_secret"));
    assert_eq!(published[0].1["topic"], "hope-alerts");
    assert_eq!(published[0].1["title"], "H.O.P.E test notification");
    let (_, read) = request(&app, "GET", "/api/v1/notifications", Value::Null, auth).await;
    assert!(read["last_sent_at"].is_number());

    // null keeps the stored token, "" removes it; invalid input changes nothing.
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, Value::Null),
        auth,
    )
    .await;
    let (_, read) = request(&app, "GET", "/api/v1/notifications", Value::Null, auth).await;
    assert_eq!(read["ntfy_token_set"], true);
    let mut bad = settings(&server, Value::Null);
    bad["ntfy_topic"] = json!("not a topic");
    assert_eq!(
        request(&app, "PUT", "/api/v1/notifications", bad, auth)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, json!("")),
        auth,
    )
    .await;
    let (_, read) = request(&app, "GET", "/api/v1/notifications", Value::Null, auth).await;
    assert_eq!(
        (read["ntfy_token_set"].clone(), read["ntfy_topic"].clone()),
        (json!(false), json!("hope-alerts"))
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/notifications", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    // A refusing server is reported, and recorded for the console.
    let (refusing, _) = fake_ntfy(403).await;
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&refusing, Value::Null),
        auth,
    )
    .await;
    let (status, body) = request(&app, "POST", "/api/v1/notifications/test", json!({}), auth).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("403"),
        "{body}"
    );
    let (_, read) = request(&app, "GET", "/api/v1/notifications", Value::Null, auth).await;
    assert!(
        read["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("403")
    );
}

#[tokio::test]
async fn alerts_notify_once_when_they_open_and_again_when_they_clear() {
    let (_dir, state, app, cookie) = setup().await;
    let (server, seen) = fake_ntfy(200).await;
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, Value::Null),
        Some(("cookie", cookie.as_str())),
    )
    .await;
    let mut host = Host::new("pve".into(), "pve1".into(), "agent");
    host.agent_id = Some("agent-1".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.expect("host");
    hope_server::notify::evaluate(&state)
        .await
        .expect("evaluate");
    hope_server::notify::evaluate(&state)
        .await
        .expect("evaluate again");
    {
        let published = seen.lock().await;
        assert_eq!(published.len(), 1, "an open alert is sent once");
        assert_eq!(
            published[0].1["title"],
            "The agent on pve1 stopped reporting"
        );
        assert_eq!(published[0].1["priority"], 4);
        assert!(
            published[0].1["click"]
                .as_str()
                .unwrap_or_default()
                .ends_with("/?host=pve")
        );
    }
    host.last_report = Some(now());
    state.save_host(&host).await.expect("host");
    hope_server::notify::evaluate(&state)
        .await
        .expect("evaluate");
    let published = seen.lock().await;
    assert_eq!(published.len(), 2);
    assert_eq!(
        published[1].1["title"],
        "Resolved: The agent on pve1 stopped reporting"
    );
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts")
        .fetch_one(&state.0.db)
        .await
        .expect("alerts");
    assert_eq!(open, 0);
}

#[tokio::test]
async fn device_types_can_be_set_by_hand_and_scan_intervals_are_validated() {
    let (_dir, state, app, cookie) = setup().await;
    let auth = Some(("cookie", cookie.as_str()));
    let mut host = Host::new("box".into(), "192.168.1.9".into(), "icmp");
    host.addresses = vec!["192.168.1.9".into()];
    host.services = vec![Service {
        port: 8006,
        name: "Proxmox VE".into(),
        observed_at: now(),
    }];
    state.save_host(&host).await.expect("host");
    let (status, set) = request(
        &app,
        "PUT",
        "/api/v1/hosts/box/type",
        json!({"kind":"nas"}),
        auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            set["device"]["kind"].clone(),
            set["device"]["manual"].clone()
        ),
        (json!("nas"), json!(true))
    );
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/hosts/box/type",
            json!({"kind":"toaster"}),
            auth
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, automatic) = request(
        &app,
        "PUT",
        "/api/v1/hosts/box/type",
        json!({"kind":null}),
        auth,
    )
    .await;
    assert_eq!(automatic["device"]["kind"], "hypervisor");
    assert_eq!(automatic["device"]["manual"], false);
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/hosts/missing/type",
            json!({"kind":"nas"}),
            auth
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (status, network) = request(
        &app,
        "POST",
        "/api/v1/networks",
        json!({"name":"IoT","cidr":"192.168.30.0/24","enabled":false,"interval_minutes":15}),
        auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{network}");
    assert_eq!(network["interval_minutes"], 15);
    let id = network["id"].as_str().expect("id").to_string();
    let path = format!("/api/v1/networks/{id}");
    assert_eq!(
        request(&app, "PUT", &path, json!({"interval_minutes":7}), auth)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, "PUT", &path, json!({"interval_minutes":60}), auth)
            .await
            .0,
        StatusCode::OK
    );
    let (_, bootstrap) = request(&app, "GET", "/api/v1/bootstrap", Value::Null, auth).await;
    let saved = &bootstrap["networks"][0];
    assert_eq!(
        (saved["interval_minutes"].clone(), saved["enabled"].clone()),
        (json!(60), json!(false))
    );
    let (_, default) = request(
        &app,
        "POST",
        "/api/v1/networks",
        json!({"name":"LAN","cidr":"10.9.0.0/24","enabled":false}),
        auth,
    )
    .await;
    assert_eq!(default["interval_minutes"], 5);
}
