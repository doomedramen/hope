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
    setup_with("releases".into()).await
}
async fn setup_with(release_dir: String) -> (TempDir, State, Router, String) {
    setup_with_ca(release_dir, None).await
}
async fn setup_with_ca(
    release_dir: String,
    ca: Option<String>,
) -> (TempDir, State, Router, String) {
    let dir = tempfile::tempdir().expect("temporary database");
    let state = State::open_with_agent_ca(
        dir.path().join("test.db").to_str().expect("path"),
        "http://localhost:8080".into(),
        release_dir,
        Capabilities {
            platform: "test".into(),
            discovery_mode: "limited".into(),
            arp: false,
            icmp: false,
            limitations: vec![],
        },
        ca,
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
async fn uninstall_instructions_require_admin_and_work_without_published_releases() {
    for ca in [None, Some("/etc/hope/owner's CA.pem".to_string())] {
        let absent = tempfile::tempdir().unwrap();
        let (_dir, state, app, cookie) =
            setup_with_ca(absent.path().to_str().unwrap().into(), ca.clone()).await;
        let (agent, _, _) = enrolled(&state, &app).await;
        let path = format!("/api/v1/agents/{agent}/uninstall-instructions");
        assert_eq!(
            request(&app, "GET", &path, Value::Null, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) =
            request(&app, "GET", &path, Value::Null, Some(("cookie", &cookie))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["custom_ca_required"], ca.is_some());
        let command = body["command"].as_str().unwrap();
        assert!(command.contains("/agent/uninstall"));
        assert!(command.contains("sh \"$installer\" uninstall"));
        assert!(!command.contains("HOPE_ENROLLMENT_TOKEN"));
        assert!(!command.contains("HOPE_PURGE_IDENTITY"));
        if let Some(ca) = ca {
            assert!(command.contains(&format!("--cacert {}", agents::shell_quote(&ca))));
        }
        // Merely opening the dialog must not revoke or otherwise alter the agent.
        assert!(!state.hosts().await.unwrap()[0].agent_revoked);
        assert_eq!(
            request(
                &app,
                "GET",
                "/api/v1/agents/missing/uninstall-instructions",
                Value::Null,
                Some(("cookie", &cookie))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let script = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/agent/uninstall")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(script.headers()["cache-control"], "no-store");
        let script = String::from_utf8(
            script
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains("phase uninstall"));
        assert!(!script.contains("@@HOPE_PUBLIC_KEY@@"));
    }
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
    let (status, error) = request(
        &app,
        "POST",
        "/api/v1/agent/report",
        json!(report),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["code"], "agent_revoked");
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
async fn legacy_container_rollups_are_unknown_while_raw_gauges_remain_available() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, _, _) = enrolled(&state, &app).await;
    let host: String = sqlx::query_scalar("SELECT id FROM hosts WHERE agent_id=?")
        .bind(&agent)
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let metrics = Metrics {
        containers: vec![
            serde_json::from_value(json!({
                "id":"container","name":"fixture","image":"fixture:latest",
                "state":"running","status":"Up","ports":"", "cpu_percent":35.0,
                "memory_used_bytes":100.0,"network_rx_bytes":500.0
            }))
            .unwrap(),
        ],
        ..Default::default()
    };
    for (report, age, resolution) in [("legacy", 90000.0, 300), ("raw", 10.0, 30)] {
        sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,resolution,body) VALUES(?,?,?,?,?)")
            .bind(&agent).bind(report).bind(now()-age).bind(resolution)
            .bind(serde_json::to_string(&metrics).unwrap()).execute(&state.0.db).await.unwrap();
    }
    let (status, points) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{host}/metrics?hours=168"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(points.as_array().unwrap().len(), 2);
    assert!(points[0]["metrics"]["containers"][0]["cpu_percent"].is_null());
    assert!(points[0]["metrics"]["containers"][0]["memory_used_bytes"].is_null());
    assert_eq!(
        points[0]["metrics"]["containers"][0]["network_rx_bytes"],
        500.0
    );
    assert_eq!(points[1]["metrics"]["containers"][0]["cpu_percent"], 35.0);
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
async fn missing_storage_telemetry_does_not_send_a_false_recovery() {
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
    let mut host = Host::new("storage".into(), "storage".into(), "agent");
    host.agent_id = Some("storage-agent".into());
    host.last_report = Some(now());
    host.metrics = Some(Metrics {
        disks: vec![Disk {
            name: "rpool/data/subvol-122-disk-0".into(),
            mount: "/".into(),
            filesystem: "zfs".into(),
            total_bytes: 100.0,
            available_bytes: 1.0,
        }],
        ..Default::default()
    });
    host.collectors = vec![CollectorStatus {
        name: "storage".into(),
        available: true,
        detail: None,
    }];
    state.save_host(&host).await.expect("host");
    hope_server::notify::evaluate(&state)
        .await
        .expect("open disk alert");
    assert_eq!(seen.lock().await.len(), 1);
    host.metrics.as_mut().unwrap().disks.clear();
    host.collectors[0].available = false;
    state.save_host(&host).await.expect("unknown storage");
    hope_server::notify::evaluate(&state)
        .await
        .expect("keep unknown alert");
    assert_eq!(seen.lock().await.len(), 1, "unknown is not recovery");
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE kind='resource'")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(open, 1);
    host.metrics.as_mut().unwrap().disks.push(Disk {
        name: "rpool/data/subvol-122-disk-0".into(),
        mount: "/".into(),
        filesystem: "zfs".into(),
        total_bytes: 100.0,
        available_bytes: 50.0,
    });
    host.collectors[0].available = true;
    state.save_host(&host).await.expect("healthy storage");
    hope_server::notify::evaluate(&state)
        .await
        .expect("recover");
    assert_eq!(
        seen.lock().await.len(),
        2,
        "a healthy reading resolves the alert"
    );
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
    let (status, identity) = request(
        &app,
        "PUT",
        "/api/v1/hosts/box/identity",
        json!({"name":"  Plex  ","icon":"selfhst:plex"}),
        auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["name_override"], "Plex");
    assert_eq!(identity["icon_override"], "selfhst:plex");
    assert_eq!(
        request(
            &app,
            "PUT",
            "/api/v1/hosts/box/identity",
            json!({"name":"Plex","icon":"https://example.com/icon.png"}),
            auth,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, cleared) = request(
        &app,
        "PUT",
        "/api/v1/hosts/box/identity",
        json!({"name":null,"icon":null}),
        auth,
    )
    .await;
    assert_eq!(cleared["name_override"], Value::Null);
    assert_eq!(cleared["icon_override"], Value::Null);
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

#[tokio::test]
async fn reports_record_the_agent_version_and_bootstrap_names_the_release_on_offer() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let report = AgentReport {
        schema_version: 1,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent,
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: vec![],
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agent/report")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {credential}"))
                .header("user-agent", "hope-agent/0.1.57")
                .body(Body::from(json!(report).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let (_, bootstrap) = request(
        &app,
        "GET",
        "/api/v1/bootstrap",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(bootstrap["hosts"][0]["agent_version"], "0.1.57");
    // No signed release in the test release directory, so none is on offer.
    assert_eq!(bootstrap["agent_release"], Value::Null);
}
#[tokio::test]
async fn live_reports_update_the_host_and_stream_it_while_history_keeps_its_spacing() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    let mut events = state.0.events.subscribe();
    let start = now() - 100.0;
    // Reports ten seconds apart: every one is live, one in three reaches history.
    for (step, cpu) in [10.0, 20.0, 30.0, 40.0, 50.0].into_iter().enumerate() {
        let report = AgentReport {
            schema_version: 1,
            report_id: Uuid::new_v4().to_string(),
            agent_id: agent.clone(),
            collected_at: start + 10.0 * step as f64,
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
        assert_eq!(body["duplicate"], false);
        assert_eq!(
            body["report_interval_seconds"],
            DEFAULT_REPORT_INTERVAL_SECONDS
        );
        match events.try_recv().expect("live event") {
            hope_server::state::Change::Host(body) => {
                let host: Host = serde_json::from_str(&body).expect("host event");
                assert_eq!(host.id, host_id);
                assert_eq!(host.metrics.expect("metrics").cpu_percent, Some(cpu));
            }
            other => panic!("expected a host event, got {other:?}"),
        }
    }
    let stored: Vec<f64> = sqlx::query_scalar(
        "SELECT collected_at FROM metrics WHERE agent_id=? ORDER BY collected_at",
    )
    .bind(&agent)
    .fetch_all(&state.0.db)
    .await
    .expect("stored");
    assert_eq!(stored.len(), 2);
    for (actual, expected) in stored.iter().zip([start, start + 30.0]) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "stored timestamp {actual} differs from expected {expected}"
        );
    }

    let invalid = request(
        &app,
        "PUT",
        "/api/v1/settings/reporting",
        json!({"interval_seconds": 7}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
    let saved = request(
        &app,
        "PUT",
        "/api/v1/settings/reporting",
        json!({"interval_seconds": 5}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(saved.0, StatusCode::OK, "{}", saved.1);
    let bootstrap = request(
        &app,
        "GET",
        "/api/v1/bootstrap",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(bootstrap.1["report_interval_seconds"], 5);
    assert_eq!(state.report_interval(), 5);
}

const RELEASE_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEA2WA6+KWaU2QKPUoNvpE8
uWs6EqZAlw32jXt3QJ4WB3vQPZMmdrq8qLyt2HMnsCGJ4a4J0qCGZeuAp9GbxdQy
Nct276PRAZixbaOcEJfJuPKaLziaWsi/UUjHvEv9MoFC8kjYU5XTFcV3ZJeslmkK
gLFFmNAjA78DTKGOK8jE/3YS4fQOYgpmGOqzFEqcJroJzoCCsMfgWvAIiStS6pFL
j1O8SL4xOSjA6SMbjBBCbt8iifWHOoiHg/PvjywBr2UC4wfWKyP0Rp8IH9VRR52D
XvJ24CepV5rzEndRwLDyi7CF1ryFSB8hwiFWmUbGerk1d6YmWQN8t50hxTnhAYZ4
YYydjBsAO+Uv3g3hSeEsVKGEUJAFc1IeSXkUI+07zMi73yZCI4QC66auJI+pqptc
DRoUsHNM4hkMBCjhBKvyB1uo19I09MQ9e38VaL++yWp5cLNRiYv2T+2YKqw1kAnv
LrmW2OY0Z47dqhFZxgYSGmKSGPOX8uIferQ1dBjahj6ZAgMBAAE=
-----END PUBLIC KEY-----
";
const RELEASE_KEY_ID: &str = "be0e49f69d223eabbd7a66c2d5cb91ce7ff9d504c1032b2f480ef85e238f5ae5";
/// A release directory shaped like assemble-release.sh's output (the server does not check RSA).
fn publish_release(version: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("release directory");
    let put = |name: &str, data: &[u8]| std::fs::write(dir.path().join(name), data).expect("write");
    put("release-public.pem", RELEASE_PEM.as_bytes());
    put("hope-agent-linux-x86_64", b"binary");
    put("hope-agent-linux-x86_64.sig", b"sig");
    put(
        "manifest.json",
        format!(
            r#"{{"format":1,"version":"{version}","created":"2026-10-14T00:00:00Z","key_id":"{RELEASE_KEY_ID}","partial":true,"artifacts":[{{"platform":"linux","arch":"x86_64","file":"hope-agent-linux-x86_64","sha256":"{}","size":6,"min_os":"x"}}]}}"#,
            "a".repeat(64)
        )
        .as_bytes(),
    );
    put("manifest.json.sig", b"sig");
    dir
}
async fn report_as(app: &Router, bearer: &str, agent: &str, version: &str) -> Value {
    report_with_updater(app, bearer, agent, version, true).await
}
async fn report_with_updater(
    app: &Router,
    bearer: &str,
    agent: &str,
    version: &str,
    updater: bool,
) -> Value {
    let report = AgentReport {
        schema_version: 1,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.into(),
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: if updater {
            vec![CollectorStatus {
                name: "agent_update".into(),
                available: true,
                detail: Some("Updater ready".into()),
            }]
        } else {
            vec![]
        },
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agent/report")
                .header("content-type", "application/json")
                .header("authorization", bearer)
                .header("user-agent", format!("hope-agent/{version}"))
                .body(Body::from(json!(report).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json")
}
#[tokio::test]
async fn an_update_request_reaches_the_agent_in_its_next_report_response_and_clears_when_done() {
    let release = publish_release("0.1.60");
    let (_dir, state, app, cookie) =
        setup_with(release.path().to_str().expect("path").into()).await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    let url = format!("/api/v1/hosts/{host_id}/update-agent");
    let admin = Some(("cookie", cookie.as_str()));

    // Nothing is asked of an agent until an administrator asks.
    assert_eq!(
        report_as(&app, &bearer, &agent, "0.1.57").await["update_to"],
        Value::Null
    );
    assert_eq!(
        request(&app, "POST", &url, json!({}), None).await.0,
        StatusCode::UNAUTHORIZED
    );

    let (status, body) = request(&app, "POST", &url, json!({}), admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let host = state.host(&host_id).await.expect("host").expect("exists");
    assert_eq!(host.update_target.as_deref(), Some("0.1.60"));
    assert!(host.update_requested_at.is_some());

    // It stays on offer in every report response until the agent runs the release.
    for _ in 0..2 {
        assert_eq!(
            report_as(&app, &bearer, &agent, "0.1.57").await["update_to"],
            "0.1.60"
        );
    }
    let done = report_as(&app, &bearer, &agent, "0.1.60").await;
    assert_eq!(done["update_to"], Value::Null);
    let host = state.host(&host_id).await.expect("host").expect("exists");
    assert_eq!((host.update_target, host.update_requested_at), (None, None));
    assert_eq!(host.agent_version.as_deref(), Some("0.1.60"));

    // An agent that already runs the release cannot be asked again.
    let (status, _) = request(&app, "POST", &url, json!({}), admin).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn update_all_asks_only_agents_that_run_an_older_release() {
    let release = publish_release("0.1.60");
    let (_dir, state, app, cookie) =
        setup_with(release.path().to_str().expect("path").into()).await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let admin = Some(("cookie", cookie.as_str()));

    // Version unknown, then current: nobody to ask.
    let (_, body) = request(&app, "POST", "/api/v1/agents/update", json!({}), admin).await;
    assert_eq!(body["requested"], 0);
    report_as(&app, &bearer, &agent, "0.1.60").await;
    let (_, body) = request(&app, "POST", "/api/v1/agents/update", json!({}), admin).await;
    assert_eq!(body["requested"], 0);

    report_as(&app, &bearer, &agent, "0.1.57").await;
    let (status, body) = request(&app, "POST", "/api/v1/agents/update", json!({}), admin).await;
    assert_eq!((status, &body["requested"]), (StatusCode::OK, &json!(1)));
    assert_eq!(
        report_as(&app, &bearer, &agent, "0.1.57").await["update_to"],
        "0.1.60"
    );

    // A revoked agent is never asked.
    request(
        &app,
        "POST",
        &format!("/api/v1/agents/{agent}/revoke"),
        json!({}),
        admin,
    )
    .await;
    let (_, body) = request(&app, "POST", "/api/v1/agents/update", json!({}), admin).await;
    assert_eq!(body["requested"], 0);
}
#[tokio::test]
async fn nothing_can_be_requested_without_a_published_release() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    report_as(&app, &format!("Bearer {credential}"), &agent, "0.1.57").await;
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    let admin = Some(("cookie", cookie.as_str()));
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{host_id}/update-agent"),
        json!({}),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("no signed agent release")),
        "{body}"
    );
    assert_eq!(
        request(&app, "POST", "/api/v1/agents/update", json!({}), admin)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_upgrade_command_needs_no_token_and_only_upgrades() {
    let (_dir, _state, app, cookie) = setup().await;
    let (_, bootstrap) = request(
        &app,
        "GET",
        "/api/v1/bootstrap",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    let command = bootstrap["agent_upgrade_command"]
        .as_str()
        .expect("command");
    assert!(
        command.contains("http://localhost:8080/agent/install")
            || command.contains("'http://localhost:8080'/agent/install"),
        "{command}"
    );
    assert!(
        command.contains("HOPE_ONLY_NEWER=1") && command.trim_end().ends_with("upgrade )"),
        "{command}"
    );
    assert!(!command.contains("HOPE_ENROLLMENT_TOKEN"), "{command}");
    assert!(command.contains("--proto '=https'"), "{command}");
}

#[tokio::test]
async fn an_agent_without_an_updater_is_pointed_at_the_upgrade_command() {
    let release = publish_release("0.1.60");
    let (_dir, state, app, cookie) =
        setup_with(release.path().to_str().expect("path").into()).await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let host_id = state.hosts().await.expect("hosts")[0].id.clone();
    let admin = Some(("cookie", cookie.as_str()));
    report_with_updater(&app, &bearer, &agent, "0.1.57", false).await;
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{host_id}/update-agent"),
        json!({}),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("upgrade command")),
        "{body}"
    );
    let (_, all) = request(&app, "POST", "/api/v1/agents/update", json!({}), admin).await;
    assert_eq!(all["requested"], 0);
    assert_eq!(
        report_with_updater(&app, &bearer, &agent, "0.1.57", false).await["update_to"],
        Value::Null
    );
    // Once it reports an updater, the same request goes through.
    report_as(&app, &bearer, &agent, "0.1.57").await;
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/hosts/{host_id}/update-agent"),
        json!({}),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn uptime_is_opt_in_validated_and_notifies_port_transitions() {
    let (_dir, state, app, cookie) = setup().await;
    let auth = Some(("cookie", cookie.as_str()));
    let (server, seen) = fake_ntfy(200).await;
    let mut notification_settings = settings(&server, Value::Null);
    notification_settings["notify_offline"] = json!(false);
    notification_settings["notify_resources"] = json!(false);
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        notification_settings,
        auth,
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut host = Host::new("uptime-box".into(), "Box".into(), "icmp");
    host.addresses = vec!["127.0.0.1".into()];
    state.save_host(&host).await.unwrap();
    for _ in 0..2 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert!(seen.lock().await.is_empty(), "off by default");
    let path = "/api/v1/hosts/uptime-box/uptime";
    let config = json!({"enabled":true,"device":false,"ports":[port, port]});
    assert_eq!(
        request(&app, "PUT", path, config.clone(), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    for ports in [json!([0]), json!([]), json!(vec![80; 33])] {
        assert_eq!(
            request(
                &app,
                "PUT",
                path,
                json!({"enabled":true,"device":false,"ports":ports}),
                auth
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, saved) = request(&app, "PUT", path, config, auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["uptime"]["ports"], json!([port]));
    hope_server::notify::evaluate(&state).await.unwrap();
    assert!(seen.lock().await.is_empty(), "first failure is debounced");
    for _ in 0..2 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert_eq!(
        seen.lock().await.len(),
        1,
        "one alert, independent of legacy categories"
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 2, "recovery");
    drop(listener);
    for _ in 0..2 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert_eq!(seen.lock().await.len(), 3, "new outage");
    request(
        &app,
        "PUT",
        path,
        json!({"enabled":false,"device":false,"ports":[port]}),
        auth,
    )
    .await;
    hope_server::notify::evaluate(&state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 3, "disabling is silent");
}

#[tokio::test]
async fn uptime_device_alerts_respect_connector_and_recovery_switches() {
    let (_dir, state, app, cookie) = setup().await;
    let auth = Some(("cookie", cookie.as_str()));
    let (server, seen) = fake_ntfy(200).await;
    let mut config = settings(&server, Value::Null);
    config["notify_offline"] = json!(false);
    config["notify_recovery"] = json!(false);
    request(&app, "PUT", "/api/v1/notifications", config.clone(), auth).await;
    let mut host = Host::new("device-monitor".into(), "Device".into(), "agent");
    host.agent_id = Some("agent".into());
    host.last_report = Some(now() - 1000.0);
    state.save_host(&host).await.unwrap();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/hosts/device-monitor/uptime",
        json!({"enabled":true,"device":true,"ports":[]}),
        auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..3 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert_eq!(seen.lock().await.len(), 1);
    assert_eq!(seen.lock().await[0].1["title"], "Device is down");
    host = state.host(&host.id).await.unwrap().unwrap();
    host.last_report = Some(now());
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 1, "recovery disabled");
    config["ntfy_enabled"] = json!(false);
    request(&app, "PUT", "/api/v1/notifications", config, auth).await;
    host.last_report = Some(now() - 1000.0);
    state.save_host(&host).await.unwrap();
    for _ in 0..2 {
        hope_server::notify::evaluate(&state).await.unwrap();
    }
    assert_eq!(seen.lock().await.len(), 1, "connector disabled");
}
