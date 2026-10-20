use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use hope_protocol::*;
use hope_server::inventory::StoredHost;
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

#[tokio::test]
async fn host_primary_ip_requires_a_usable_address_in_any_configured_cidr() {
    let (_dir, state, _app, _) = setup().await;
    for (id, cidr) in [("office", "192.168.40.0/24"), ("lab", "10.20.0.0/16")] {
        sqlx::query(
            "INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES(?,?,?,?,?)",
        )
        .bind(id)
        .bind(id)
        .bind(cidr)
        .bind(true)
        .bind(5_i64)
        .execute(&state.0.db)
        .await
        .unwrap();
    }

    let mut host = StoredHost::new("host-managed".into(), "managed host".into(), "agent");
    host.addresses = vec![
        "127.0.0.1".into(),
        "169.254.20.1".into(),
        "192.168.41.7".into(),
        "10.20.3.8".into(),
        "192.168.40.9".into(),
    ];
    state.save_host(&host).await.unwrap();
    let projected = state.host("host-managed").await.unwrap().unwrap();
    assert_eq!(projected.primary_ip.as_deref(), Some("10.20.3.8"));
    assert_eq!(projected.addresses, host.addresses);

    let mut unmanaged = StoredHost::new("host-unmanaged".into(), "unmanaged host".into(), "agent");
    unmanaged.addresses = vec![
        "127.0.0.1".into(),
        "169.254.20.1".into(),
        "192.168.41.7".into(),
    ];
    state.save_host(&unmanaged).await.unwrap();
    let projected = state.host("host-unmanaged").await.unwrap().unwrap();
    assert_eq!(projected.primary_ip, None);
    assert_eq!(projected.addresses, unmanaged.addresses);
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

async fn evaluate_notifications(state: &State) -> anyhow::Result<()> {
    hope_server::notify::evaluate(state).await?;
    hope_server::notify::deliver_pending(state).await?;
    Ok(())
}

fn child_batch(
    agent_id: &str,
    sequence: u64,
    generation: u64,
    complete: bool,
    observed_at: f64,
    devices: Vec<ChildDeviceReport>,
) -> ChildReportBatch {
    ChildReportBatch {
        schema_version: SCHEMA_VERSION,
        batch_id: Uuid::new_v4().to_string(),
        agent_id: agent_id.into(),
        source: ChildSourceKind::Docker,
        source_scope: "docker-engine-test".into(),
        source_token: None,
        legacy_source_scope: None,
        legacy_identities: vec![],
        sequence,
        inventory_generation: generation,
        inventory_id: complete.then(|| Uuid::new_v4().to_string()),
        chunk_index: 0,
        chunk_count: 1,
        complete,
        observed_at,
        devices,
    }
}

fn child_device() -> ChildDeviceReport {
    ChildDeviceReport {
        identity: SourceIdentity {
            kind: ChildDeviceKind::DockerContainer,
            scope: "docker-engine-test".into(),
            key: "container-full-id".into(),
            incarnation: "created-at-100".into(),
        },
        name: "web".into(),
        lifecycle: DeviceLifecycle::Running,
        status: Some("Up".into()),
        health: Some("healthy".into()),
        restart_count: Some(0),
        parent_key: None,
        node: None,
        ownership_generation: None,
        legacy_incarnation: None,
        tags: vec![],
        interfaces: vec![Interface {
            name: "eth0".into(),
            mac: Some("02:00:00:00:00:02".into()),
            addresses: vec!["192.168.1.20".into()],
            gateway: None,
        }],
        cpu_cores: None,
        memory_bytes: None,
        image: Some("nginx:latest".into()),
        compose_project: Some("stack".into()),
        compose_service: Some("web".into()),
        published_ports: vec![],
        guest_agent_status: None,
        guest_os: None,
        guest_filesystems: vec![],
        network_configuration: vec![],
        disk_configuration: vec![],
        metrics: ChildMetrics {
            cpu_percent: Some(15.0),
            cpu_capacity_cores: Some(2.0),
            cpu_measurement: Some(CpuMeasurement::DockerEngine),
            memory_used_bytes: Some(1024.0),
            memory_total_bytes: Some(4096.0),
            memory_measurement: Some(MemoryMeasurement::DockerWorkingSet),
            ..Default::default()
        },
    }
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
async fn child_batches_are_idempotent_ordered_and_archive_only_after_complete_absence() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let t0 = now() - 240.0;
    let initial = child_batch(&agent, 1, 1, true, t0, vec![child_device()]);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(initial),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["duplicate"], false);
    let child_id: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let projected = state.host(&child_id).await.unwrap().expect("child device");
    assert_eq!(
        projected.source.as_ref().unwrap().kind,
        ChildDeviceKind::DockerContainer
    );
    assert_eq!(projected.child.as_ref().unwrap().name, "web");
    assert_eq!(
        projected.parent_id,
        Some(state.hosts().await.unwrap()[0].id.clone())
    );
    assert_eq!(state.hosts().await.unwrap().len(), 2);
    let (history_status, history) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{child_id}/source-metrics?hours=168"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(history_status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["metrics"]["cpu_percent"], 15.0);
    let duplicate = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(initial),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(duplicate.0, StatusCode::OK);
    assert_eq!(duplicate.1["duplicate"], true);

    let partial = child_batch(&agent, 2, 2, false, t0 + 60.0, vec![]);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(partial),
            Some(("authorization", &bearer)),
        )
        .await
        .0,
        StatusCode::OK
    );
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(lifecycle, "running", "partial data cannot age a child");

    for (sequence, generation, elapsed) in [(3, 3, 61.0), (4, 4, 240.0), (5, 5, 361.0)] {
        let batch = child_batch(&agent, sequence, generation, true, t0 + elapsed, vec![]);
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(batch),
            Some(("authorization", &bearer)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(lifecycle, "archived");
    assert_eq!(
        state.host(&child_id).await.unwrap().unwrap().lifecycle,
        Some(DeviceLifecycle::Archived)
    );
    let observations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_observations")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let metrics: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(observations, 1);
    assert_eq!(metrics, 1);
    let batches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM child_batches")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(batches, 5, "the replay does not create a second batch row");

    let old = child_batch(&agent, 4, 5, true, now(), vec![]);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(old),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(&agent)
        .execute(&state.0.db)
        .await
        .unwrap();
    let revoked = child_batch(&agent, 6, 6, false, now(), vec![]);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(revoked),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "agent_revoked");
}

#[tokio::test]
async fn shared_proxmox_identity_follows_authorized_owner_and_rejects_delayed_old_owner() {
    let (_dir, state, app, cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let cluster_token = "a".repeat(64);
    let mut guest = child_device();
    guest.identity.kind = ChildDeviceKind::ProxmoxVm;
    guest.identity.scope = "cluster-uuid".into();
    guest.identity.key = "101".into();
    guest.identity.incarnation = "stable-incarnation".into();
    guest.node = Some("node-a".into());
    guest.ownership_generation = Some(1);
    let mut batch = child_batch(&first, 1, 1, true, now() - 30.0, vec![guest.clone()]);
    batch.source = ChildSourceKind::Proxmox;
    batch.source_scope = "cluster-uuid".into();
    batch.source_token = Some(cluster_token.clone());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(batch),
            Some(("authorization", &format!("Bearer {first_credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let original_id: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();

    // A mismatched token cannot claim a matching VMID from another agent.
    let mut bad = child_batch(&second, 1, 1, true, now(), vec![guest.clone()]);
    bad.source = ChildSourceKind::Proxmox;
    bad.source_scope = "cluster-uuid".into();
    bad.source_token = Some("b".repeat(64));
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(bad),
            Some(("authorization", &format!("Bearer {second_credential}")))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    guest.node = Some("node-b".into());
    guest.ownership_generation = Some(2);
    let mut moved = child_batch(&second, 1, 1, true, now() - 20.0, vec![guest.clone()]);
    moved.source = ChildSourceKind::Proxmox;
    moved.source_scope = "cluster-uuid".into();
    moved.source_token = Some(cluster_token.clone());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(moved),
            Some(("authorization", &format!("Bearer {second_credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let canonical_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(canonical_count, 1);

    guest.node = Some("node-a".into());
    guest.ownership_generation = Some(1);
    let mut delayed = child_batch(&first, 2, 2, true, now() + 10.0, vec![guest]);
    delayed.source = ChildSourceKind::Proxmox;
    delayed.source_scope = "cluster-uuid".into();
    delayed.source_token = Some(cluster_token);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(delayed),
            Some(("authorization", &format!("Bearer {first_credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (owner, generation, body): (String, i64, String) = sqlx::query_as(
        "SELECT parent_agent_id,ownership_generation,body FROM source_devices WHERE id=?",
    )
    .bind(&original_id)
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(owner, second);
    assert_eq!(generation, 2);
    assert_eq!(
        serde_json::from_str::<ChildDeviceReport>(&body)
            .unwrap()
            .node
            .as_deref(),
        Some("node-b")
    );
    let (alias_status, alias): (StatusCode, Value) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{original_id}"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(alias_status, StatusCode::OK);
    assert_eq!(alias["source_device_id"], original_id);
}

#[tokio::test]
async fn equal_vmids_in_different_authorized_clusters_remain_separate() {
    let (_dir, state, app, cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    for (agent, credential, scope, token) in [
        (&first, &first_credential, "cluster-one", "1".repeat(64)),
        (&second, &second_credential, "cluster-two", "2".repeat(64)),
    ] {
        let mut guest = child_device();
        guest.identity.kind = ChildDeviceKind::ProxmoxVm;
        guest.identity.key = "101".into();
        guest.identity.scope = scope.into();
        guest.identity.incarnation = "incarnation".into();
        let mut batch = child_batch(agent, 1, 1, true, now(), vec![guest]);
        batch.source = ChildSourceKind::Proxmox;
        batch.source_scope = scope.into();
        batch.source_token = Some(token);
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/v1/agent/children",
                json!(batch),
                Some(("authorization", &format!("Bearer {credential}")))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let (status, groups) = request(
        &app,
        "GET",
        "/api/v1/devices/duplicate-groups",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(groups["groups"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn joined_cluster_members_auto_reconcile_legacy_exact_rows_and_keep_history_aliases() {
    let (_dir, state, app, cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let mut guest = child_device();
    guest.identity.kind = ChildDeviceKind::ProxmoxVm;
    guest.identity.scope = "shared-cluster".into();
    guest.identity.key = "55".into();
    guest.identity.incarnation = "persistent-incarnation".into();
    let body = serde_json::to_string(&guest).unwrap();
    let id_a = "legacy-row-a";
    let id_b = "legacy-row-b";
    let timestamp = now() - 100.0;
    for (id, agent, generation) in [(id_a, &first, 1_i64), (id_b, &second, 2_i64)] {
        sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,ownership_generation,body) VALUES(?,?,'proxmox','shared-cluster','55','persistent-incarnation','proxmox_vm','running','guest',?,?,?,?)")
            .bind(id).bind(agent).bind(timestamp+generation as f64).bind(generation).bind(generation).bind(&body).execute(&state.0.db).await.unwrap();
        sqlx::query("INSERT INTO source_device_identities(source,identity_owner,source_scope,source_key,incarnation,source_device_id) VALUES('proxmox',?,'shared-cluster','55','persistent-incarnation',?)")
            .bind(agent).bind(id).execute(&state.0.db).await.unwrap();
        sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,30,'{}')")
            .bind(id).bind(format!("report-{id}")).bind(timestamp+generation as f64).execute(&state.0.db).await.unwrap();
        sqlx::query("INSERT INTO source_observations(source_device_id,batch_id,observed_at,inventory_generation,body) VALUES(?,?,?,?,'{}')")
            .bind(id).bind(format!("batch-{id}")).bind(timestamp+generation as f64).bind(generation).execute(&state.0.db).await.unwrap();
    }
    let token = "c".repeat(64);
    for (agent, credential) in [(&first, &first_credential), (&second, &second_credential)] {
        let mut join = child_batch(agent, 1, 1, false, now(), vec![]);
        join.source = ChildSourceKind::Proxmox;
        join.source_scope = "shared-cluster".into();
        join.source_token = Some(token.clone());
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(join),
            Some(("authorization", &format!("Bearer {credential}"))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    guest.ownership_generation = Some(3);
    let mut report = child_batch(&second, 2, 2, true, now(), vec![guest]);
    report.source = ChildSourceKind::Proxmox;
    report.source_scope = "shared-cluster".into();
    report.source_token = Some(token);
    let (status, response) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(report),
        Some(("authorization", &format!("Bearer {second_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let canonical: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(canonical, id_b);
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics WHERE source_device_id=?")
            .bind(&canonical)
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(history, 3);
    let observations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_observations")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        observations, 3,
        "observations keep their original IDs and provenance"
    );
    let (alias_status, alias) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{id_a}"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(alias_status, StatusCode::OK);
    assert_eq!(alias["source_device_id"], canonical);
    for host_id in [canonical.as_str(), id_a] {
        let (status, points) = request(
            &app,
            "GET",
            &format!("/api/v1/hosts/{host_id}/source-metrics?hours=168"),
            Value::Null,
            Some(("cookie", &cookie)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(points.as_array().unwrap().len(), 3);
        assert!(
            points
                .as_array()
                .unwrap()
                .iter()
                .all(|point| point["source_device_id"] == canonical)
        );
    }
}

#[tokio::test]
async fn explicit_local_legacy_identity_proof_maps_old_scope_rows_across_cluster_join() {
    let (_dir, state, app, cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let mut old = child_device();
    old.identity.kind = ChildDeviceKind::ProxmoxVm;
    old.identity.key = "55".into();
    old.identity.incarnation = "old-a".into();
    old.identity.scope = "local-a".into();
    let t = now() - 120.0;
    for (id, agent, scope, inc) in [
        ("old-a-id", &first, "local-a", "old-a"),
        ("old-b-id", &second, "local-b", "old-b"),
    ] {
        old.identity.scope = scope.into();
        old.identity.incarnation = inc.into();
        sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,body) VALUES(?,?,? ,?, '55',?,'proxmox_vm','running','guest',?,1,?)")
            .bind(id).bind(agent).bind("proxmox").bind(scope).bind(inc).bind(t).bind(serde_json::to_string(&old).unwrap()).execute(&state.0.db).await.unwrap();
        sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,30,'{}')")
            .bind(id).bind(format!("legacy-{id}")).bind(t).execute(&state.0.db).await.unwrap();
    }
    let token = "d".repeat(64);
    let mut current = child_device();
    current.identity.kind = ChildDeviceKind::ProxmoxVm;
    current.identity.scope = "shared-cluster".into();
    current.identity.key = "55".into();
    current.identity.incarnation = "new-shared-inc".into();
    current.ownership_generation = Some(1);
    let mut a = child_batch(&first, 1, 1, true, now(), vec![current.clone()]);
    a.source = ChildSourceKind::Proxmox;
    a.source_scope = "shared-cluster".into();
    a.source_token = Some(token.clone());
    a.legacy_source_scope = Some("local-a".into());
    a.devices[0].legacy_incarnation = Some("old-a".into());
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(a),
        Some(("authorization", &format!("Bearer {first_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    current.ownership_generation = Some(2);
    let mut b = child_batch(&second, 1, 1, true, now() + 1.0, vec![current]);
    b.source = ChildSourceKind::Proxmox;
    b.source_scope = "shared-cluster".into();
    b.source_token = Some(token);
    b.legacy_source_scope = Some("local-b".into());
    b.devices[0].legacy_incarnation = Some("old-b".into());
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(b),
        Some(("authorization", &format!("Bearer {second_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let canonical: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(canonical, "old-a-id");
    let aliased: String = sqlx::query_scalar(
        "SELECT survivor_id FROM source_device_aliases WHERE alias_id='old-b-id'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(aliased, canonical);
    let old_history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics WHERE source_device_id='old-b-id'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(
        old_history, 1,
        "legacy incarnation history remains distinct"
    );
    let (status, host) = request(
        &app,
        "GET",
        "/api/v1/hosts/old-b-id",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(host["source_device_id"], canonical);
}

#[tokio::test]
async fn metadata_only_legacy_map_carries_old_remote_guest_to_current_owner() {
    let (_dir, state, app, _cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let mut old = child_device();
    old.identity.kind = ChildDeviceKind::ProxmoxVm;
    old.identity.scope = "node-a-local".into();
    old.identity.key = "99".into();
    old.identity.incarnation = "old-remote-inc".into();
    sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,body) VALUES('remote-old',?,'proxmox','node-a-local','99','old-remote-inc','proxmox_vm','running','remote guest',?,1,?)")
        .bind(&first).bind(now()).bind(serde_json::to_string(&old).unwrap()).execute(&state.0.db).await.unwrap();
    let token = "e".repeat(64);
    let mut mapping = child_batch(&first, 1, 1, false, now(), vec![]);
    mapping.source = ChildSourceKind::Proxmox;
    mapping.source_scope = "joined-cluster".into();
    mapping.source_token = Some(token.clone());
    mapping.legacy_source_scope = Some("node-a-local".into());
    mapping.legacy_identities = vec![LegacySourceIdentity {
        kind: ChildDeviceKind::ProxmoxVm,
        key: "99".into(),
        incarnation: "old-remote-inc".into(),
        canonical_incarnation: "shared-remote-inc".into(),
    }];
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(mapping),
        Some(("authorization", &format!("Bearer {first_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut remote = child_device();
    remote.identity.kind = ChildDeviceKind::ProxmoxVm;
    remote.identity.scope = "joined-cluster".into();
    remote.identity.key = "99".into();
    remote.identity.incarnation = "shared-remote-inc".into();
    remote.ownership_generation = Some(2);
    remote.node = Some("node-b".into());
    let mut current = child_batch(&second, 1, 1, true, now() + 1.0, vec![remote]);
    current.source = ChildSourceKind::Proxmox;
    current.source_scope = "joined-cluster".into();
    current.source_token = Some(token);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(current),
        Some(("authorization", &format!("Bearer {second_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let id:String=sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE identity_owner='@shared' AND source_key='99'").fetch_one(&state.0.db).await.unwrap();
    assert_eq!(id, "remote-old");
    let owner: String =
        sqlx::query_scalar("SELECT parent_agent_id FROM source_devices WHERE id='remote-old'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(owner, second);
}

#[tokio::test]
async fn late_legacy_map_reconciles_against_an_already_reported_canonical_guest() {
    let (_dir, state, app, cookie) = setup().await;
    let (legacy_agent, legacy_credential, _) = enrolled(&state, &app).await;
    let (owner_agent, owner_credential, _) = enrolled(&state, &app).await;
    let token = "c".repeat(64);
    let mut old = child_device();
    old.identity.kind = ChildDeviceKind::ProxmoxVm;
    old.identity.scope = "node-a-local".into();
    old.identity.key = "99".into();
    old.identity.incarnation = "old-remote-inc".into();
    sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,body) VALUES('remote-old',?,'proxmox','node-a-local','99','old-remote-inc','proxmox_vm','running','remote guest',?,1,?)")
        .bind(&legacy_agent).bind(now() - 1.0).bind(serde_json::to_string(&old).unwrap()).execute(&state.0.db).await.unwrap();
    sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES('remote-old','legacy-sample',?,30,'{}')")
        .bind(now() - 1.0).execute(&state.0.db).await.unwrap();

    // The current owner reports first. Its authenticated cluster identity is already stored,
    // but the old remote copy is not yet linked because its reporter has not sent its map.
    let mut current = child_device();
    current.identity.kind = ChildDeviceKind::ProxmoxVm;
    current.identity.scope = "joined-cluster".into();
    current.identity.key = "99".into();
    current.identity.incarnation = "shared-remote-inc".into();
    current.ownership_generation = Some(2);
    current.node = Some("node-b".into());
    let mut report = child_batch(&owner_agent, 1, 1, true, now(), vec![current]);
    report.source = ChildSourceKind::Proxmox;
    report.source_scope = "joined-cluster".into();
    report.source_token = Some(token.clone());
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(report),
        Some(("authorization", &format!("Bearer {owner_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let canonical: String = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE identity_owner='@shared' AND source_key='99'")
        .fetch_one(&state.0.db).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // A metadata-only batch from the old reporter arrives later. There may be no subsequent
    // child report (its collector can now be stopped), so ingest must reconcile the pending
    // identity immediately against the already stored canonical report using MAC + freshness.
    let mut mapping = child_batch(&legacy_agent, 1, 1, false, now() + 1.0, vec![]);
    mapping.source = ChildSourceKind::Proxmox;
    mapping.source_scope = "joined-cluster".into();
    mapping.source_token = Some(token);
    mapping.legacy_source_scope = Some("node-a-local".into());
    mapping.legacy_identities = vec![LegacySourceIdentity {
        kind: ChildDeviceKind::ProxmoxVm,
        key: "99".into(),
        incarnation: "old-remote-inc".into(),
        canonical_incarnation: "shared-remote-inc".into(),
    }];
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(mapping),
        Some(("authorization", &format!("Bearer {legacy_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let survivor: String = sqlx::query_scalar(
        "SELECT survivor_id FROM source_device_aliases WHERE alias_id='remote-old'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(survivor, canonical);
    let archived: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_devices WHERE lifecycle='archived'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(
        archived, 0,
        "merged identities become aliases, not archives"
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pending_legacy_identities WHERE legacy_source_device_id='remote-old'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(pending, 0);
    let (status, old_url) = request(
        &app,
        "GET",
        "/api/v1/hosts/remote-old",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(old_url["source_device_id"], canonical);
    let (status, history) = request(
        &app,
        "GET",
        "/api/v1/hosts/remote-old/source-metrics?hours=168",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|point| point["source_device_id"] == "remote-old")
    );
}

#[tokio::test]
async fn authenticated_legacy_maps_reconcile_same_cluster_copies_before_owner_report() {
    let (_dir, state, app, _) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let token = "d".repeat(64);
    for (agent, id, old_scope, incarnation) in [
        (&first, "old-a", "node-a-local", "old-a-inc"),
        (&second, "old-b", "node-b-local", "old-b-inc"),
    ] {
        let mut old = child_device();
        old.identity.kind = ChildDeviceKind::ProxmoxLxc;
        old.identity.scope = old_scope.into();
        old.identity.key = "241".into();
        old.identity.incarnation = incarnation.into();
        old.name = "guest".into();
        old.interfaces.clear();
        old.network_configuration = vec![GuestNetworkConfiguration {
            name: "net0".into(),
            bridge: Some("vmbr0".into()),
            vlan_tag: None,
            model: Some("virtio".into()),
            mac: Some("02:00:00:00:00:41".into()),
        }];
        sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,body) VALUES(?,?, 'proxmox',?,?,?,'proxmox_lxc','running','guest',?,1,?)")
            .bind(id).bind(agent).bind(old_scope).bind("241").bind(incarnation).bind(now() - 2.0).bind(serde_json::to_string(&old).unwrap())
            .execute(&state.0.db).await.unwrap();
    }

    for (agent, credential, old_scope, incarnation) in [
        (&first, &first_credential, "node-a-local", "old-a-inc"),
        (&second, &second_credential, "node-b-local", "old-b-inc"),
    ] {
        let mut mapping = child_batch(agent, 1, 1, false, now(), vec![]);
        mapping.source = ChildSourceKind::Proxmox;
        mapping.source_scope = "joined-cluster".into();
        mapping.source_token = Some(token.clone());
        mapping.legacy_source_scope = Some(old_scope.into());
        mapping.legacy_identities = vec![LegacySourceIdentity {
            kind: ChildDeviceKind::ProxmoxLxc,
            key: "241".into(),
            incarnation: incarnation.into(),
            canonical_incarnation: "shared-241-inc".into(),
        }];
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(mapping),
            Some(("authorization", &format!("Bearer {credential}"))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "same-cluster copies collapse by shared proof and MAC"
    );
    let survivor: String =
        sqlx::query_scalar("SELECT survivor_id FROM source_device_aliases WHERE alias_id='old-b'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(survivor, "old-a");
    let current_pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pending_legacy_identities WHERE legacy_source_device_id='old-a' AND canonical_incarnation='shared-241-inc'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(
        current_pending, 1,
        "the surviving legacy row can migrate when its owner reports"
    );
    let projected = state.hosts().await.unwrap();
    assert_eq!(
        projected.len(),
        3,
        "two parent agents and one source guest row"
    );
}

#[tokio::test]
async fn legacy_identity_metadata_alone_cannot_merge_reused_guest_with_new_mac() {
    let (_dir, state, app, _cookie) = setup().await;
    let (first, first_credential, _) = enrolled(&state, &app).await;
    let (second, second_credential, _) = enrolled(&state, &app).await;
    let mut old = child_device();
    old.identity.kind = ChildDeviceKind::ProxmoxVm;
    old.identity.scope = "node-a-local".into();
    old.identity.key = "109".into();
    old.identity.incarnation = "old-remote-inc".into();
    sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,body) VALUES('remote-old',?,'proxmox','node-a-local','109','old-remote-inc','proxmox_vm','running','remote guest',?,1,?)")
        .bind(&first).bind(now()).bind(serde_json::to_string(&old).unwrap()).execute(&state.0.db).await.unwrap();
    let mut mapping = child_batch(&first, 1, 1, false, now(), vec![]);
    mapping.source = ChildSourceKind::Proxmox;
    mapping.source_scope = "joined-cluster".into();
    mapping.source_token = Some("f".repeat(64));
    mapping.legacy_source_scope = Some("node-a-local".into());
    mapping.legacy_identities = vec![LegacySourceIdentity {
        kind: ChildDeviceKind::ProxmoxVm,
        key: "109".into(),
        incarnation: "old-remote-inc".into(),
        canonical_incarnation: "new-shared-inc".into(),
    }];
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(mapping),
        Some(("authorization", &format!("Bearer {first_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut recreated = child_device();
    recreated.identity.kind = ChildDeviceKind::ProxmoxVm;
    recreated.identity.scope = "joined-cluster".into();
    recreated.identity.key = "109".into();
    recreated.identity.incarnation = "new-shared-inc".into();
    recreated.interfaces[0].mac = Some("02:00:00:00:00:99".into());
    recreated.ownership_generation = Some(1);
    let mut report = child_batch(&second, 1, 1, true, now() + 1.0, vec![recreated]);
    report.source = ChildSourceKind::Proxmox;
    report.source_scope = "joined-cluster".into();
    report.source_token = Some("f".repeat(64));
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(report),
        Some(("authorization", &format!("Bearer {second_credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "a new MAC must leave the prior incarnation separate"
    );
    let aliases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM source_device_aliases WHERE alias_id='remote-old'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(aliases, 0, "metadata alone cannot create a merge alias");
}

#[tokio::test]
async fn proxmox_lxc_child_batches_are_projected_as_hosts() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let scope = Uuid::new_v4().to_string();
    let mut lxc = child_device();
    lxc.identity.kind = hope_protocol::ChildDeviceKind::ProxmoxLxc;
    lxc.identity.scope = scope.clone();
    lxc.identity.key = "202".into();
    lxc.identity.incarnation = "lxc-incarnation".into();
    lxc.name = "dns-container".into();
    lxc.status = Some("running".into());
    lxc.health = None;
    lxc.restart_count = None;
    lxc.node = Some("pve-a".into());
    let mut batch = child_batch(&agent, 1, 1, true, now(), vec![lxc]);
    batch.source = ChildSourceKind::Proxmox;
    batch.source_scope = scope;

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(batch),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hosts = state.hosts().await.unwrap();
    let lxc_host = hosts
        .iter()
        .find(|host| host.name == "dns-container")
        .expect("LXC inventory item should appear in Hosts");
    assert_eq!(
        lxc_host.source.as_ref().unwrap().kind,
        hope_protocol::ChildDeviceKind::ProxmoxLxc
    );
}

#[tokio::test]
async fn multipart_inventory_is_authoritative_only_after_every_part_is_stored() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let first = child_batch(&agent, 1, 1, true, now() - 30.0, vec![child_device()]);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(first),
            Some(("authorization", &bearer)),
        )
        .await
        .0,
        StatusCode::OK
    );

    let group = Uuid::new_v4().to_string();
    let mut part_0 = child_batch(&agent, 2, 2, false, now(), vec![child_device()]);
    part_0.inventory_id = Some(group.clone());
    part_0.chunk_index = 0;
    part_0.chunk_count = 2;
    let mut part_1 = child_batch(&agent, 3, 2, true, now(), vec![]);
    part_1.inventory_id = Some(group);
    part_1.chunk_index = 1;
    part_1.chunk_count = 2;
    for batch in [part_0, part_1] {
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(batch),
            Some(("authorization", &bearer)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let complete_generation: i64 = sqlx::query_scalar(
        "SELECT complete_generation FROM child_batch_heads WHERE parent_agent_id=? AND source='docker' AND source_scope='docker-engine-test'",
    )
    .bind(&agent)
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(complete_generation, 2);
    let (lifecycle, missing_count): (String, i64) = sqlx::query_as(
        "SELECT lifecycle,missing_complete_count FROM source_devices WHERE source_key='container-full-id'",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(
        lifecycle, "running",
        "all chunk observations form the full presence set"
    );
    assert_eq!(missing_count, 0);

    // A final marker cannot make a generation complete if an earlier part was dropped.
    let missing = Uuid::new_v4().to_string();
    let mut orphan_final = child_batch(&agent, 5, 3, true, now(), vec![]);
    orphan_final.inventory_id = Some(missing);
    orphan_final.chunk_index = 1;
    orphan_final.chunk_count = 2;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(orphan_final),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let complete_generation: i64 = sqlx::query_scalar(
        "SELECT complete_generation FROM child_batch_heads WHERE parent_agent_id=? AND source='docker' AND source_scope='docker-engine-test'",
    )
    .bind(&agent)
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(complete_generation, 2);
}

#[tokio::test]
async fn published_ports_create_distinct_service_relationships_per_container() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let mut first = child_device();
    first.published_ports = vec![ContainerPort {
        host_ip: Some("0.0.0.0".into()),
        container_port: 80,
        host_port: Some(8080),
        protocol: "tcp".into(),
    }];
    let mut second = first.clone();
    second.identity.key = "other-container-id".into();
    second.identity.incarnation = "created-at-200".into();
    second.name = "other-web".into();
    let batch = child_batch(&agent, 1, 1, true, now(), vec![first, second]);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(batch),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let edges: Vec<(String, String)> = sqlx::query_as(
        "SELECT from_device_id,detail FROM device_relationships WHERE kind='exposes_service' ORDER BY from_device_id",
    )
    .fetch_all(&state.0.db)
    .await
    .unwrap();
    assert_eq!(edges.len(), 2);
    assert_ne!(
        edges[0].0, edges[1].0,
        "shared host ports stay container-scoped"
    );
    assert_eq!(edges[0].1, "tcp://0.0.0.0:8080 → container:80");
    let projected = state.host(&edges[0].0).await.unwrap().unwrap();
    assert!(projected.relationships.iter().any(|relationship| {
        relationship.kind == RelationshipKind::ExposesService
            && relationship.detail.as_deref() == Some("tcp://0.0.0.0:8080 → container:80")
    }));
}

#[tokio::test]
async fn source_matches_can_be_rejected_confirmed_and_unlinked_without_losing_source_data() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)")
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut scanned = StoredHost::new("scanned-device".into(), "scanned device".into(), "tcp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:99".into()];
    scanned.network_ids = vec!["lan".into()];
    state.save_host(&scanned).await.unwrap();

    let first = child_batch(&agent, 1, 1, true, now(), vec![child_device()]);
    let bearer = format!("Bearer {credential}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(first),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let source_id: String =
        sqlx::query_scalar("SELECT id FROM source_devices WHERE source_key='container-full-id'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    let associations = state.host(&source_id).await.unwrap().unwrap().associations;
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0].status, AssociationStatus::Suggested);

    let path = format!("/api/v1/hosts/{source_id}/association");
    let (status, body) = request(
        &app,
        "POST",
        &path,
        json!({"action":"reject","host_id":"scanned-device"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let next = child_batch(&agent, 2, 2, true, now(), vec![child_device()]);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(next),
            Some(("authorization", &bearer)),
        )
        .await
        .0,
        StatusCode::OK
    );
    let associations = state.host(&source_id).await.unwrap().unwrap().associations;
    assert_eq!(associations[0].status, AssociationStatus::Rejected);

    let (status, body) = request(
        &app,
        "POST",
        &path,
        json!({"action":"confirm","host_id":"scanned-device"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let combined = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|host| host.id == "scanned-device")
        .unwrap();
    assert_eq!(
        combined.source_device_id.as_deref(),
        Some(source_id.as_str())
    );
    assert!(combined.child.is_some());
    assert_eq!(state.hosts().await.unwrap().len(), 2);

    let (status, body) = request(
        &app,
        "POST",
        &path,
        json!({"action":"unlink","host_id":"scanned-device"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state.hosts().await.unwrap().len(), 3);
    assert!(
        state
            .host("scanned-device")
            .await
            .unwrap()
            .unwrap()
            .child
            .is_none()
    );
    assert!(
        state
            .host(&source_id)
            .await
            .unwrap()
            .unwrap()
            .child
            .is_some()
    );
}

#[tokio::test]
async fn unique_interface_matches_auto_link_without_changing_discovered_host_id() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)")
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut scanned = StoredHost::new("scanned-device".into(), "scanned device".into(), "tcp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.network_ids = vec!["lan".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(child_batch(&agent, 1, 1, true, now(), vec![child_device()])),
        Some(("authorization", &format!("Bearer {credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state.hosts().await.unwrap().len(), 2);
    let combined = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|host| host.id == "scanned-device")
        .unwrap();
    assert_eq!(
        combined.id, "scanned-device",
        "the discovered URL remains stable"
    );
    assert!(combined.child.is_some());
    assert_eq!(
        combined.source_device_id,
        sqlx::query_scalar::<_, String>("SELECT id FROM source_devices")
            .fetch_one(&state.0.db)
            .await
            .ok()
    );
    assert_eq!(
        combined.associations[0].status,
        AssociationStatus::Confirmed
    );
    let source_id = combined.source_device_id.clone().unwrap();
    let (history_status, history) = request(
        &app,
        "GET",
        "/api/v1/hosts/scanned-device/source-metrics?hours=168",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(history_status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["source_device_id"], source_id);
}

#[tokio::test]
async fn proxmox_configured_nic_mac_links_guest_to_unique_scanned_host() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES(?,?,?,?,?)")
        .bind("managed-network")
        .bind("Managed network")
        .bind("192.168.1.0/24")
        .bind(true)
        .bind(5_i64)
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut scanned = StoredHost::new("scanned-lxc".into(), "sabnzbd".into(), "arp");
    scanned.addresses = vec!["192.168.1.41".into()];
    scanned.macs = vec!["02:00:00:00:00:41".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();

    let mut guest = child_device();
    guest.identity.kind = ChildDeviceKind::ProxmoxLxc;
    guest.identity.key = "241".into();
    guest.identity.incarnation = "lxc-241-incarnation".into();
    guest.name = "sabnzbd".into();
    guest.interfaces.clear();
    guest.network_configuration = vec![GuestNetworkConfiguration {
        name: "net0".into(),
        bridge: Some("vmbr0".into()),
        vlan_tag: None,
        model: Some("virtio".into()),
        mac: Some("02:00:00:00:00:41".into()),
    }];
    let mut batch = child_batch(&agent, 1, 1, true, now(), vec![guest]);
    batch.source = ChildSourceKind::Proxmox;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(batch),
        Some(("authorization", &format!("Bearer {credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let source_id: String =
        sqlx::query_scalar("SELECT id FROM source_devices WHERE source_key='241'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    let hosts = state.hosts().await.unwrap();
    assert_eq!(
        hosts.len(),
        2,
        "the source guest should project into its scan host"
    );
    let combined = hosts.iter().find(|host| host.id == "scanned-lxc").unwrap();
    assert_eq!(
        combined.source_device_id.as_deref(),
        Some(source_id.as_str())
    );
    assert_eq!(
        combined.child.as_ref().unwrap().identity.kind,
        ChildDeviceKind::ProxmoxLxc
    );
    assert!(combined.macs.iter().any(|mac| mac == "02:00:00:00:00:41"));
    assert_eq!(
        combined.associations[0].status,
        AssociationStatus::Confirmed
    );
    assert_eq!(combined.primary_ip.as_deref(), Some("192.168.1.41"));

    let (status, old_url) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{source_id}"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(old_url["id"], "scanned-lxc");
    assert_eq!(old_url["source_device_id"], source_id);
    assert_eq!(old_url["primary_ip"], "192.168.1.41");
    let (status, history) = request(
        &app,
        "GET",
        "/api/v1/hosts/scanned-lxc/source-metrics?hours=168",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["source_device_id"], source_id);
}

#[tokio::test]
async fn stale_source_reconciles_on_later_scan_without_another_child_report() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('managed-network','Managed network','192.168.1.0/24',1,5)")
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut scanned = StoredHost::new("scanned-lxc".into(), "sabnzbd".into(), "arp");
    scanned.addresses = vec!["192.168.1.41".into()];
    scanned.macs = vec!["02:00:00:00:00:41".into()];
    scanned.last_seen = now() - 8.0 * 86400.0;
    state.save_host(&scanned).await.unwrap();

    let mut guest = child_device();
    guest.identity.kind = ChildDeviceKind::ProxmoxLxc;
    guest.identity.key = "241".into();
    guest.identity.incarnation = "lxc-241-incarnation".into();
    guest.name = "sabnzbd".into();
    guest.interfaces.clear();
    guest.network_configuration = vec![GuestNetworkConfiguration {
        name: "net0".into(),
        bridge: Some("vmbr0".into()),
        vlan_tag: None,
        model: Some("virtio".into()),
        mac: Some("02:00:00:00:00:41".into()),
    }];
    let mut batch = child_batch(&agent, 1, 1, true, now(), vec![guest]);
    batch.source = ChildSourceKind::Proxmox;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(batch),
        Some(("authorization", &format!("Bearer {credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let source_id: String =
        sqlx::query_scalar("SELECT id FROM source_devices WHERE source_key='241'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    let unlinked: Option<String> =
        sqlx::query_scalar("SELECT host_id FROM source_devices WHERE id=?")
            .bind(&source_id)
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(
        unlinked, None,
        "old scan evidence should not auto-link at ingest"
    );

    // A later network scan sees the same guest; the Proxmox collector is now offline, so no
    // new child report can trigger the write-time association path.
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    let linked: Option<String> =
        sqlx::query_scalar("SELECT host_id FROM source_devices WHERE id=?")
            .bind(&source_id)
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(
        linked.as_deref(),
        Some("scanned-lxc"),
        "scan persistence must commit the association before any inventory read"
    );
    let hosts = state.hosts().await.unwrap();
    assert_eq!(hosts.len(), 2, "one parent and one canonical guest row");
    let combined = hosts.iter().find(|host| host.id == "scanned-lxc").unwrap();
    assert_eq!(
        combined.source_device_id.as_deref(),
        Some(source_id.as_str())
    );
    assert_eq!(combined.child.as_ref().unwrap().identity.key, "241");
    assert_eq!(combined.associations.len(), 1);
    assert_eq!(
        combined.associations[0].status,
        AssociationStatus::Confirmed
    );

    let (status, old_url) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{source_id}"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(old_url["id"], "scanned-lxc");
    assert_eq!(old_url["source_device_id"], source_id);
    let (status, history) = request(
        &app,
        "GET",
        &format!("/api/v1/hosts/{source_id}/source-metrics?hours=168"),
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["source_device_id"], source_id);
    let (status, scan_history) = request(
        &app,
        "GET",
        "/api/v1/hosts/scanned-lxc/source-metrics?hours=168",
        Value::Null,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scan_history.as_array().unwrap().len(), 1);
    assert_eq!(scan_history[0]["source_device_id"], source_id);
}

#[tokio::test]
async fn stale_source_projection_keeps_ambiguous_mac_candidates_separate() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let mac = "02:00:00:00:00:41";
    for id in ["scanned-a", "scanned-b"] {
        let mut scanned = StoredHost::new(id.into(), "sabnzbd".into(), "arp");
        scanned.addresses = vec!["192.168.1.41".into()];
        scanned.macs = vec![mac.into()];
        scanned.last_seen = now();
        state.save_host(&scanned).await.unwrap();
    }
    let mut guest = child_device();
    guest.identity.kind = ChildDeviceKind::ProxmoxLxc;
    guest.identity.key = "241".into();
    guest.identity.incarnation = "lxc-241-incarnation".into();
    guest.name = "sabnzbd".into();
    guest.interfaces.clear();
    guest.network_configuration = vec![GuestNetworkConfiguration {
        name: "net0".into(),
        bridge: Some("vmbr0".into()),
        vlan_tag: None,
        model: Some("virtio".into()),
        mac: Some(mac.into()),
    }];
    let mut batch = child_batch(&agent, 1, 1, true, now(), vec![guest]);
    batch.source = ChildSourceKind::Proxmox;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(batch),
        Some(("authorization", &format!("Bearer {credential}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hosts = state.hosts().await.unwrap();
    assert_eq!(
        hosts.len(),
        4,
        "parent, two ambiguous scan rows, and source row"
    );
    assert!(
        hosts
            .iter()
            .filter(|host| host.source_device_id.is_some())
            .all(|host| {
                host.agent_id.is_none() && !["scanned-a", "scanned-b"].contains(&host.id.as_str())
            })
    );
}

#[tokio::test]
async fn linked_scan_presence_prevents_source_archival_until_the_scan_goes_stale() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)")
        .execute(&state.0.db)
        .await
        .unwrap();
    let t0 = now() - 620.0;
    let mut scanned = StoredHost::new("scanned-device".into(), "web".into(), "tcp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = t0;
    state.save_host(&scanned).await.unwrap();
    let bearer = format!("Bearer {credential}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/agent/children",
        json!(child_batch(&agent, 1, 1, true, t0, vec![child_device()])),
        Some(("authorization", &bearer)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child_id: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM source_devices WHERE id=? AND host_id='scanned-device')",
    )
    .bind(&child_id)
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert!(linked);

    for (sequence, elapsed) in [(2, 61.0), (3, 240.0), (4, 361.0)] {
        let batch = child_batch(&agent, sequence, sequence, true, t0 + elapsed, vec![]);
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(batch),
            Some(("authorization", &bearer)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM source_devices WHERE id=?")
        .bind(&child_id)
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(lifecycle, "stale", "fresh scan presence prevents archival");

    for (sequence, elapsed) in [(5, 600.0), (6, 780.0), (7, 900.0)] {
        let batch = child_batch(&agent, sequence, sequence, true, t0 + elapsed, vec![]);
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(batch),
            Some(("authorization", &bearer)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM source_devices WHERE id=?")
        .bind(&child_id)
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        lifecycle, "archived",
        "stale scan presence no longer blocks archival"
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
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES(?,?,?,?,?)")
        .bind("managed-network")
        .bind("Managed network")
        .bind("192.168.1.0/24")
        .bind(true)
        .bind(5_i64)
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut host = state.stored_host(&host_id).await.unwrap().unwrap();
    host.addresses = vec![
        "127.0.0.1".into(),
        "169.254.1.2".into(),
        "203.0.113.7".into(),
        "192.168.1.10".into(),
    ];
    state.save_host(&host).await.unwrap();
    assert_eq!(
        state
            .host(&host_id)
            .await
            .unwrap()
            .unwrap()
            .primary_ip
            .as_deref(),
        Some("192.168.1.10")
    );
    request(
        &app,
        "POST",
        &format!("/api/v1/agents/{agent}/revoke"),
        json!({}),
        Some(("cookie", &cookie)),
    )
    .await;
    let revoked = state.host(&host_id).await.unwrap().unwrap();
    assert!(revoked.agent_revoked);
    assert_eq!(revoked.primary_ip.as_deref(), Some("192.168.1.10"));
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
    assert_eq!(hosts[0].primary_ip.as_deref(), Some("192.168.1.10"));
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

fn discovered(id: &str, address: &str, mac: &str) -> StoredHost {
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
    let mut other = state
        .stored_host(&host_id)
        .await
        .expect("query")
        .expect("host");
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
    let (server, seen, _) = fake_ntfy_switchable(status).await;
    (server, seen)
}

async fn fake_ntfy_switchable(
    status: u16,
) -> (String, Seen, std::sync::Arc<std::sync::atomic::AtomicU16>) {
    let status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(status));
    let response_status = status.clone();
    let seen: Seen = Default::default();
    let record = seen.clone();
    let app = Router::new().route(
        "/",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
            let record = record.clone();
            let status = response_status.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                record
                    .lock()
                    .await
                    .push((auth, serde_json::from_str(&body).unwrap_or(Value::Null)));
                StatusCode::from_u16(status.load(std::sync::atomic::Ordering::SeqCst))
                    .expect("status")
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
    (format!("http://{address}"), seen, status)
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
    let mut host = StoredHost::new("pve".into(), "pve1".into(), "agent");
    host.agent_id = Some("agent-1".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.expect("host");
    evaluate_notifications(&state).await.expect("evaluate");
    evaluate_notifications(&state)
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
    evaluate_notifications(&state).await.expect("evaluate");
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
    let mut host = StoredHost::new("storage".into(), "storage".into(), "agent");
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
    evaluate_notifications(&state)
        .await
        .expect("open disk alert");
    assert_eq!(seen.lock().await.len(), 1);
    host.metrics.as_mut().unwrap().disks.clear();
    host.collectors[0].available = false;
    state.save_host(&host).await.expect("unknown storage");
    evaluate_notifications(&state)
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
    evaluate_notifications(&state).await.expect("recover");
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
    let mut host = StoredHost::new("box".into(), "192.168.1.9".into(), "icmp");
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
    let id = format!("{version}-0123456789ab");
    let directory = dir.path().join(&id);
    std::fs::create_dir(&directory).unwrap();
    std::os::unix::fs::symlink(&id, dir.path().join("current")).unwrap();
    let put = |name: &str, data: &[u8]| std::fs::write(directory.join(name), data).expect("write");
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
    let mut host = StoredHost::new("uptime-box".into(), "Box".into(), "icmp");
    host.addresses = vec!["127.0.0.1".into()];
    state.save_host(&host).await.unwrap();
    for _ in 0..2 {
        evaluate_notifications(&state).await.unwrap();
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
    evaluate_notifications(&state).await.unwrap();
    assert!(seen.lock().await.is_empty(), "first failure is debounced");
    for _ in 0..2 {
        evaluate_notifications(&state).await.unwrap();
    }
    assert_eq!(
        seen.lock().await.len(),
        1,
        "one alert, independent of legacy categories"
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    evaluate_notifications(&state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 2, "recovery");
    drop(listener);
    for _ in 0..2 {
        evaluate_notifications(&state).await.unwrap();
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
    evaluate_notifications(&state).await.unwrap();
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
    let mut host = StoredHost::new("device-monitor".into(), "Device".into(), "agent");
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
        evaluate_notifications(&state).await.unwrap();
    }
    assert_eq!(seen.lock().await.len(), 1);
    assert_eq!(seen.lock().await[0].1["title"], "Device is down");
    host = state.stored_host(&host.id).await.unwrap().unwrap();
    host.last_report = Some(now());
    state.save_host(&host).await.unwrap();
    evaluate_notifications(&state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 1, "recovery disabled");
    config["ntfy_enabled"] = json!(false);
    request(&app, "PUT", "/api/v1/notifications", config, auth).await;
    host.last_report = Some(now() - 1000.0);
    state.save_host(&host).await.unwrap();
    for _ in 0..2 {
        evaluate_notifications(&state).await.unwrap();
    }
    assert_eq!(seen.lock().await.len(), 1, "connector disabled");
}

#[tokio::test]
async fn identity_unlink_survives_reads_reports_and_explicit_reset() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)").execute(&state.0.db).await.unwrap();
    let mut scanned = StoredHost::new("identity-scan".into(), "scan".into(), "arp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    let bearer = format!("Bearer {credential}");
    let post_child = |sequence| {
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(
                &agent,
                sequence,
                sequence,
                true,
                now(),
                vec![child_device()]
            )),
            Some(("authorization", &bearer)),
        )
    };
    assert_eq!(post_child(1).await.0, StatusCode::OK);
    let source_id = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.id == "identity-scan")
        .unwrap()
        .source_device_id
        .unwrap();
    let path = format!("/api/v1/hosts/{source_id}/association");
    let (status, body) = request(
        &app,
        "POST",
        &path,
        json!({"action":"unlink","host_id":"identity-scan"}),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for _ in 0..2 {
        let hosts = state.hosts().await.unwrap();
        assert!(
            hosts
                .iter()
                .find(|h| h.id == "identity-scan")
                .unwrap()
                .child
                .is_none(),
            "unlink must not be undone by projection"
        );
    }
    assert_eq!(post_child(2).await.0, StatusCode::OK);
    assert!(
        state
            .host("identity-scan")
            .await
            .unwrap()
            .unwrap()
            .child
            .is_none()
    );
    // The UI exposes Reset for rejected/unlinked associations, allowing auto-matching again.
    assert_eq!(
        request(
            &app,
            "POST",
            &path,
            json!({"action":"unlink","host_id":"identity-scan"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(post_child(3).await.0, StatusCode::OK);
    assert!(
        state
            .hosts()
            .await
            .unwrap()
            .iter()
            .find(|h| h.id == "identity-scan")
            .unwrap()
            .child
            .is_some()
    );
}

#[tokio::test]
async fn identity_network_refresh_does_not_persist_source_projections() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)").execute(&state.0.db).await.unwrap();
    let mut scanned = StoredHost::new("identity-scan".into(), "scan".into(), "arp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(&agent, 1, 1, true, now(), vec![child_device()])),
            Some(("authorization", &format!("Bearer {credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let source_id = state
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.id == "identity-scan")
        .unwrap()
        .source_device_id
        .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/networks",
            json!({"name":"Other","cidr":"10.20.0.0/24"}),
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
            &format!("/api/v1/hosts/{source_id}/association"),
            json!({"action":"reject","host_id":"identity-scan"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::OK
    );
    let hosts = state.hosts().await.unwrap();
    assert_eq!(
        hosts
            .iter()
            .filter(|h| h.source_device_id.as_deref() == Some(&source_id))
            .count(),
        1,
        "detached source must appear exactly once"
    );
    let stored: String = sqlx::query_scalar("SELECT body FROM hosts WHERE id='identity-scan'")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    let stored: Host = serde_json::from_str(&stored).unwrap();
    assert!(stored.child.is_none());
    assert!(stored.source_device_id.is_none());
}

#[tokio::test]
async fn identity_live_event_preserves_the_snapshot_source_view() {
    let (_dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled,interval_minutes) VALUES('lan','LAN','192.168.1.0/24',1,5)").execute(&state.0.db).await.unwrap();
    let host_id = state.hosts().await.unwrap()[0].id.clone();
    for round in 0..2 {
        let report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: agent.clone(),
            collected_at: now(),
            inventory: None,
            metrics: Metrics::default(),
            collectors: vec![],
        };
        let mut events = state.0.events.subscribe();
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
        let hope_server::state::Change::Host(body) = events.recv().await.unwrap() else {
            panic!("expected host event")
        };
        if round == 1 {
            let live: Host = serde_json::from_str(&body).unwrap();
            let snapshot = state
                .hosts()
                .await
                .unwrap()
                .into_iter()
                .find(|host| host.id == host_id)
                .unwrap();
            assert!(snapshot.child.is_some());
            assert_eq!(
                serde_json::to_value(live).unwrap(),
                serde_json::to_value(snapshot).unwrap()
            );
        } else {
            let mut child = child_device();
            child.interfaces = inventory().interfaces;
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/api/v1/agent/children",
                    json!(child_batch(&agent, 1, 1, true, now(), vec![child])),
                    Some(("authorization", &bearer))
                )
                .await
                .0,
                StatusCode::OK
            );
        }
    }
}

#[tokio::test]
async fn identity_source_settings_survive_reports_without_materializing_a_host() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(&agent, 1, 1, true, now(), vec![child_device()])),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    let source_id: String = sqlx::query_scalar("SELECT id FROM source_devices")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    for (suffix, body) in [
        (
            "identity",
            json!({"name":"My workload","icon":"selfhst:nginx"}),
        ),
        ("type", json!({"kind":"server"})),
        (
            "uptime",
            json!({"enabled":true,"device":false,"ports":[80]}),
        ),
    ] {
        let (status, body) = request(
            &app,
            "PUT",
            &format!("/api/v1/hosts/{source_id}/{suffix}"),
            body,
            Some(("cookie", &cookie)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let mut child = child_device();
    child.name = "A new source name".into();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(&agent, 2, 2, true, now(), vec![child])),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    let host = state.host(&source_id).await.unwrap().unwrap();
    assert_eq!(host.name_override.as_deref(), Some("My workload"));
    assert_eq!(host.icon_override.as_deref(), Some("selfhst:nginx"));
    assert!(host.device.unwrap().manual);
    assert_eq!(host.uptime.ports, vec![80]);
    assert!(host.uptime.enabled);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hosts WHERE id=?")
        .bind(&source_id)
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "settings must not create a physical host for a source-only device"
    );
}

#[tokio::test]
async fn identity_migration_repairs_saved_views_and_keeps_settings_and_old_links() {
    let (dir, state, app, _) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let bearer = format!("Bearer {credential}");
    let mut scanned = StoredHost::new("migration-scan".into(), "scan".into(), "arp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(&agent, 1, 1, true, now(), vec![child_device()])),
            Some(("authorization", &bearer))
        )
        .await
        .0,
        StatusCode::OK
    );
    let projected = state.host("migration-scan").await.unwrap().unwrap();
    let source_id = projected.source_device_id.clone().unwrap();
    // Recreate the schema and persisted projection corruption produced by v14.
    sqlx::raw_sql("DROP INDEX confirmed_source_device; DROP INDEX confirmed_scan_host; ALTER TABLE device_associations DROP COLUMN manual; DROP TABLE source_device_settings; PRAGMA user_version=14;").execute(&state.0.db).await.unwrap();
    sqlx::query("UPDATE hosts SET body=? WHERE id='migration-scan'")
        .bind(serde_json::to_string(&projected).unwrap())
        .execute(&state.0.db)
        .await
        .unwrap();
    let mut ghost = Host::new(source_id.clone(), "workload".into(), "docker");
    ghost.source = projected.source.clone();
    ghost.child = projected.child.clone();
    ghost.source_device_id = Some(source_id.clone());
    ghost.name_override = Some("Keep my name".into());
    ghost.icon_override = Some("selfhst:nginx".into());
    ghost.uptime = UptimeSettings {
        enabled: true,
        device: false,
        ports: vec![80],
    };
    sqlx::query("INSERT INTO hosts(id,name,body) VALUES(?,?,?)")
        .bind(&ghost.id)
        .bind(&ghost.name)
        .bind(serde_json::to_string(&ghost).unwrap())
        .execute(&state.0.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO source_device_aliases(alias_id,survivor_id) VALUES('old-source-url',?)",
    )
    .bind(&source_id)
    .execute(&state.0.db)
    .await
    .unwrap();
    let capabilities = state.0.capabilities.clone();
    state.0.db.close().await;
    let state = State::open_with_agent_ca(
        dir.path().join("test.db").to_str().unwrap(),
        "http://localhost:8080".into(),
        "releases".into(),
        capabilities,
        None,
    )
    .await
    .unwrap();
    let host = state.host("old-source-url").await.unwrap().unwrap();
    assert_eq!(host.id, "migration-scan");
    assert_eq!(host.name_override.as_deref(), Some("Keep my name"));
    assert!(host.uptime.enabled);
    assert_eq!(state.hosts().await.unwrap().len(), 2);
    let stored = state.stored_host("migration-scan").await.unwrap().unwrap();
    assert!(stored.child.is_none());
    assert!(stored.source_device_id.is_none());
    assert!(state.stored_host(&source_id).await.unwrap().is_none());
    assert_eq!(
        state
            .host(&source_id)
            .await
            .unwrap()
            .unwrap()
            .source_device_id,
        Some(source_id.clone())
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics WHERE source_device_id=?")
            .bind(&source_id)
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    // Opening a repaired database repeatedly must be harmless.
    hope_server::migrations::run(&state.0.db).await.unwrap();
}

#[tokio::test]
async fn identity_database_prevents_two_confirmed_sources_for_one_host() {
    let (_dir, state, app, cookie) = setup().await;
    let (agent, credential, _) = enrolled(&state, &app).await;
    let mut scanned = StoredHost::new("single-owner".into(), "scan".into(), "arp");
    scanned.addresses = vec!["192.168.1.20".into()];
    scanned.macs = vec!["02:00:00:00:00:02".into()];
    scanned.last_seen = now();
    state.save_host(&scanned).await.unwrap();
    let mut other = child_device();
    other.identity.key = "other-child".into();
    other.interfaces.clear();
    other.name = "other".into();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/agent/children",
            json!(child_batch(
                &agent,
                1,
                1,
                true,
                now(),
                vec![child_device(), other]
            )),
            Some(("authorization", &format!("Bearer {credential}")))
        )
        .await
        .0,
        StatusCode::OK
    );
    let source_id: String =
        sqlx::query_scalar("SELECT id FROM source_devices WHERE source_key='other-child'")
            .fetch_one(&state.0.db)
            .await
            .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/v1/hosts/{source_id}/association"),
            json!({"action":"confirm","host_id":"single-owner"}),
            Some(("cookie", &cookie))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(sqlx::query("INSERT INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at) VALUES(?,'single-owner','confirmed','test','[]','hash',1)").bind(&source_id).execute(&state.0.db).await.is_err());
}

#[tokio::test]
async fn notification_outbox_retries_after_restart_and_keeps_recovery_in_order() {
    let (dir, state, app, cookie) = setup().await;
    let (server, seen, response_status) = fake_ntfy_switchable(503).await;
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, Value::Null),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut host = StoredHost::new("retry".into(), "retry host".into(), "agent");
    host.agent_id = Some("retry-agent".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    assert!(
        seen.lock().await.is_empty(),
        "evaluation does no network I/O"
    );
    assert_eq!(
        hope_server::notify::deliver_pending(&state).await.unwrap(),
        0
    );
    let retry: (i64, f64) = sqlx::query_as("SELECT attempts,next_attempt FROM notification_outbox")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(retry.0, 1);
    assert!(retry.1 > now());
    host.last_report = Some(now());
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    assert_eq!(
        hope_server::notify::deliver_pending(&state).await.unwrap(),
        0
    );
    assert_eq!(
        seen.lock().await.len(),
        1,
        "recovery waits behind the failed opening message"
    );
    let capabilities = state.0.capabilities.clone();
    state.0.db.close().await;
    let reopened = State::open(
        dir.path().join("test.db").to_str().unwrap(),
        "http://localhost:8080".into(),
        "releases".into(),
        capabilities,
    )
    .await
    .unwrap();
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&reopened.0.db)
        .await
        .unwrap();
    assert_eq!(pending, 2, "both transitions survive a restart");
    response_status.store(200, std::sync::atomic::Ordering::SeqCst);
    sqlx::query("UPDATE notification_outbox SET next_attempt=0")
        .execute(&reopened.0.db)
        .await
        .unwrap();
    assert_eq!(
        hope_server::notify::deliver_pending(&reopened)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        hope_server::notify::deliver_pending(&reopened)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        hope_server::notify::deliver_pending(&reopened)
            .await
            .unwrap(),
        0
    );
    let messages = seen.lock().await;
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].1["title"], messages[1].1["title"]);
    assert!(
        messages[2].1["title"]
            .as_str()
            .unwrap()
            .starts_with("Resolved:")
    );
    assert!(
        hope_server::notify::settings(&reopened)
            .await
            .unwrap()
            .last_error
            .is_none()
    );
}

#[tokio::test]
async fn notification_outbox_capacity_postpones_transitions_without_losing_them() {
    let (_dir, state, app, cookie) = setup().await;
    let (server, _) = fake_ntfy(200).await;
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, Value::Null),
        Some(("cookie", &cookie)),
    )
    .await;
    sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4096) INSERT INTO notification_outbox(alert_key,kind,destination,body,next_attempt,created_at) SELECT 'full-'||x,'resource','fixture','{}',0,0 FROM n").execute(&state.0.db).await.unwrap();
    let mut host = StoredHost::new("capacity".into(), "capacity host".into(), "agent");
    host.agent_id = Some("capacity-agent".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(
        open, 0,
        "a full queue must not consume an unsent transition"
    );
    sqlx::query("DELETE FROM notification_outbox WHERE id=1")
        .execute(&state.0.db)
        .await
        .unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM notification_outbox),(SELECT COUNT(*) FROM alerts)",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(counts, (4096, 1));
    host.last_report = Some(now());
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(open, 1, "recovery also waits for queue capacity");
    sqlx::query("DELETE FROM notification_outbox WHERE id=2")
        .execute(&state.0.db)
        .await
        .unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM notification_outbox),(SELECT COUNT(*) FROM alerts)",
    )
    .fetch_one(&state.0.db)
    .await
    .unwrap();
    assert_eq!(counts, (4096, 0));
}

#[tokio::test]
async fn notification_settings_cancel_pending_delivery_for_disabled_or_changed_destinations() {
    let (_dir, state, app, cookie) = setup().await;
    let (server, seen) = fake_ntfy(503).await;
    let mut config = settings(&server, Value::Null);
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        config.clone(),
        Some(("cookie", &cookie)),
    )
    .await;
    let mut host = StoredHost::new("cancel".into(), "cancel host".into(), "agent");
    host.agent_id = Some("cancel-agent".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    config["ntfy_topic"] = json!("different-topic");
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        config.clone(),
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(
        hope_server::notify::deliver_pending(&state).await.unwrap(),
        0
    );
    assert!(
        seen.lock().await.is_empty(),
        "old messages cannot leak to a new topic"
    );
    host.last_report = Some(now());
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    config["ntfy_enabled"] = json!(false);
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        config,
        Some(("cookie", &cookie)),
    )
    .await;
    assert_eq!(
        hope_server::notify::deliver_pending(&state).await.unwrap(),
        0
    );
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&state.0.db)
        .await
        .unwrap();
    assert_eq!(pending, 0);
    assert!(seen.lock().await.is_empty());
}

#[tokio::test]
async fn slow_notification_delivery_does_not_block_evaluation_or_shutdown() {
    let (dir, state, app, cookie) = setup().await;
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let request_entered = entered.clone();
    let ntfy = Router::new().route(
        "/",
        axum::routing::post(move || {
            let request_entered = request_entered.clone();
            async move {
                request_entered.notify_one();
                std::future::pending::<StatusCode>().await
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let ntfy_task = tokio::spawn(async move { axum::serve(listener, ntfy).await.unwrap() });
    request(
        &app,
        "PUT",
        "/api/v1/notifications",
        settings(&server, Value::Null),
        Some(("cookie", &cookie)),
    )
    .await;
    let mut host = StoredHost::new("slow".into(), "slow host".into(), "agent");
    host.agent_id = Some("slow-agent".into());
    host.last_report = Some(now() - 900.0);
    state.save_host(&host).await.unwrap();
    hope_server::notify::evaluate(&state).await.unwrap();
    hope_server::notify::spawn(state.clone());
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .expect("delivery starts");
    host.last_report = Some(now());
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        state.save_host(&host).await.unwrap();
        hope_server::notify::evaluate(&state).await.unwrap();
    })
    .await
    .expect("condition evaluation stays responsive during HTTP I/O");
    let capabilities = state.0.capabilities.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hope_server::lifecycle::settle(&state, std::time::Duration::from_secs(2)),
    )
    .await
    .expect("shutdown cancels delivery");
    assert_eq!(
        state
            .0
            .active_tasks
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let reopened = State::open(
        dir.path().join("test.db").to_str().unwrap(),
        "http://localhost:8080".into(),
        "releases".into(),
        capabilities,
    )
    .await
    .unwrap();
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&reopened.0.db)
        .await
        .unwrap();
    assert_eq!(
        pending, 2,
        "cancellation keeps unacknowledged messages durable"
    );
    ntfy_task.abort();
}

#[tokio::test]
async fn inventory_writes_wait_for_an_independent_sqlite_writer_before_reading() {
    let (_dir, state, _app, _) = setup().await;
    let mut host = StoredHost::new("concurrent".into(), "before".into(), "agent");
    state.save_host(&host).await.unwrap();
    // SSH progress and other single-statement writers do not take the inventory mutex.
    let mut writer = state.0.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO meta(key,value) VALUES('independent-writer','held')")
        .execute(&mut *writer)
        .await
        .unwrap();
    host.name = "after".into();
    let save = state.save_host(&host);
    tokio::pin!(save);
    tokio::select! {
        result = &mut save => panic!("inventory write must wait for the writer reservation: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
    }
    writer.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), &mut save)
        .await
        .expect("writer is released")
        .expect("save succeeds without a stale-snapshot upgrade");
    assert_eq!(state.host(&host.id).await.unwrap().unwrap().name, "after");
}
