//! Operational hardening: headers, cookies, same-origin mutation checks, sessions, abuse limits,
//! SSE lag/disconnect behavior, error bounding and consistent online backup.
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, Response, StatusCode},
};
use hope_protocol::*;
use hope_server::{
    agents, backup,
    limits::TrustedProxies,
    router,
    state::{State, hash, now, secret},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "a-long-test-password";

fn capabilities() -> Capabilities {
    Capabilities {
        platform: "test".into(),
        discovery_mode: "limited".into(),
        arp: false,
        icmp: false,
        limitations: vec![],
    }
}
struct Env {
    _dir: TempDir,
    db: std::path::PathBuf,
    state: State,
    app: Router,
}
async fn env(url: &str) -> Env {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("h.db");
    let state = State::open(
        db.to_str().unwrap(),
        url.into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .expect("state");
    state
        .set_meta("setup_token_hash", &hash("test-setup-token-long-enough"))
        .await
        .unwrap();
    let app = router(state.clone(), "web/out");
    Env {
        _dir: dir,
        db,
        state,
        app,
    }
}
fn json_request(method: &str, path: &str, body: Value, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
async fn send(app: &Router, request: Request<Body>) -> Response<Body> {
    app.clone().oneshot(request).await.expect("response")
}
async fn body_text(response: Response<Body>) -> String {
    String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes()).into()
}
fn from_ip(mut request: Request<Body>, ip: &str) -> Request<Body> {
    let addr: SocketAddr = format!("{ip}:40000").parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));
    request
}
async fn setup_session(env: &Env) -> (String, String) {
    let response = send(
        &env.app,
        json_request(
            "POST",
            "/api/v1/auth/setup",
            json!({"password":PASSWORD,"setup_token":"test-setup-token-long-enough"}),
            &[],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string();
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    (cookie, set_cookie)
}
fn login(password: &str) -> Value {
    json!({"password": password})
}

#[tokio::test]
async fn security_headers_everywhere_and_no_store_on_api_responses() {
    let plain = env("http://localhost:8080").await;
    let (cookie, _) = setup_session(&plain).await;
    let health = send(
        &plain.app,
        Request::get("/healthz").body(Body::empty()).unwrap(),
    )
    .await;
    let h = health.headers();
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["x-frame-options"], "DENY");
    let csp = h["content-security-policy"].to_str().unwrap();
    for part in [
        "default-src 'self'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "base-uri 'none'",
    ] {
        assert!(csp.contains(part), "{csp}");
    }
    assert!(
        h.get("strict-transport-security").is_none(),
        "no HSTS for http origin"
    );
    assert!(
        h.get("cache-control").is_none(),
        "static/health responses stay cacheable-neutral"
    );
    // Static fallback (even a 404) carries the security headers too.
    let missing = send(
        &plain.app,
        Request::get("/missing").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(missing.headers()["x-frame-options"], "DENY");

    // Authentication, enrollment, install-command and SSH job responses: no-store, including errors.
    let sensitive: Vec<(&str, &str, Value, bool)> = vec![
        ("GET", "/api/v1/auth/status", Value::Null, false),
        (
            "POST",
            "/api/v1/auth/login",
            login("wrong-password-123"),
            false,
        ),
        ("POST", "/api/v1/auth/login", login(PASSWORD), false),
        ("POST", "/api/v1/enrollment-tokens", json!({}), true),
        ("POST", "/api/v1/enrollment-tokens", json!({}), false),
        ("POST", "/api/v1/agent/enroll", json!({}), false),
        ("GET", "/api/v1/ssh/jobs", Value::Null, true),
        ("GET", "/api/v1/ssh/jobs", Value::Null, false),
        ("GET", "/api/v1/events", Value::Null, true),
        ("POST", "/api/v1/ssh/install", json!({}), true),
    ];
    for (method, path, body, authed) in sensitive {
        let headers: Vec<(&str, &str)> = if authed {
            vec![("cookie", &cookie)]
        } else {
            vec![]
        };
        let response = send(&plain.app, json_request(method, path, body, &headers)).await;
        assert_eq!(
            response.headers()["cache-control"],
            "no-store",
            "{method} {path} {}",
            response.status()
        );
        assert_eq!(response.headers()["pragma"], "no-cache", "{method} {path}");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }

    let secure = env("https://hope.example").await;
    let response = send(
        &secure.app,
        Request::get("/healthz").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(
        response.headers()["strict-transport-security"]
            .to_str()
            .unwrap()
            .starts_with("max-age=")
    );
}

#[tokio::test]
async fn readiness_checks_database_and_shutdown_while_liveness_stays_process_only() {
    let env = env("http://localhost:8080").await;
    let ready = send(
        &env.app,
        Request::get("/readyz").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(ready.status(), StatusCode::OK);
    let ready: Value = serde_json::from_str(&body_text(ready).await).unwrap();
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["checks"]["database"], "ok");
    assert_eq!(ready["checks"]["shutdown"], "running");

    env.state.0.db.close().await;
    let unavailable = send(
        &env.app,
        Request::get("/readyz").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    let unavailable: Value = serde_json::from_str(&body_text(unavailable).await).unwrap();
    assert_eq!(unavailable["checks"]["database"], "unavailable");

    env.state.0.shutdown.cancel();
    let stopping = send(
        &env.app,
        Request::get("/readyz").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(stopping.status(), StatusCode::SERVICE_UNAVAILABLE);
    let stopping: Value = serde_json::from_str(&body_text(stopping).await).unwrap();
    assert_eq!(stopping["checks"]["shutdown"], "stopping");

    let live = send(
        &env.app,
        Request::get("/healthz").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(live.status(), StatusCode::OK);
}

#[tokio::test]
async fn session_cookie_flags_follow_the_canonical_origin() {
    let secure = env("https://hope.example").await;
    let (_, set_cookie) = setup_session(&secure).await;
    for flag in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/"] {
        assert!(set_cookie.contains(flag), "{set_cookie}");
    }
    let logout = send(
        &secure.app,
        json_request(
            "POST",
            "/api/v1/logout",
            json!({}),
            &[("cookie", set_cookie.split(';').next().unwrap())],
        ),
    )
    .await;
    let cleared = logout.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cleared.contains("Max-Age=0") && cleared.contains("Secure") && cleared.contains("HttpOnly")
    );

    let plain = env("http://127.0.0.1:8080").await;
    let (_, set_cookie) = setup_session(&plain).await;
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    assert!(
        !set_cookie.contains("Secure"),
        "Secure would make the cookie unusable over http"
    );
}

#[tokio::test]
async fn mutations_must_be_same_origin() {
    let env = env("https://hope.example").await;
    let (cookie, _) = setup_session(&env).await;
    let network = json!({"name":"LAN","cidr":"192.168.50.0/24"});
    let denied = [
        ("POST", "Origin", "https://evil.example"),
        ("POST", "Origin", "null"),
        ("POST", "Origin", "http://hope.example"),
        ("POST", "Sec-Fetch-Site", "cross-site"),
        ("POST", "Sec-Fetch-Site", "same-site"),
        ("PUT", "Origin", "https://evil.example"),
        ("PATCH", "Sec-Fetch-Site", "cross-site"),
        ("DELETE", "Origin", "https://evil.example"),
    ];
    for (method, name, value) in denied {
        let path = if method == "POST" {
            "/api/v1/networks"
        } else {
            "/api/v1/map"
        };
        let response = send(
            &env.app,
            json_request(
                method,
                path,
                network.clone(),
                &[("cookie", &cookie), (name, value)],
            ),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{method} {name}: {value}"
        );
    }
    let allowed = [
        vec![
            ("Origin", "https://hope.example"),
            ("Sec-Fetch-Site", "same-origin"),
        ],
        vec![],
    ];
    for (i, extra) in allowed.into_iter().enumerate() {
        let mut headers = vec![("cookie", cookie.as_str())];
        headers.extend(extra);
        let response = send(
            &env.app,
            json_request(
                "POST",
                "/api/v1/networks",
                json!({"name":"LAN","cidr":format!("192.168.{}.0/24", 60 + i)}),
                &headers,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    // Reads are not blocked by the cross-site check (they carry no side effects).
    let read = send(
        &env.app,
        json_request(
            "GET",
            "/api/v1/bootstrap",
            Value::Null,
            &[("cookie", &cookie), ("Sec-Fetch-Site", "cross-site")],
        ),
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
}

#[tokio::test]
async fn sessions_expire_and_logout_invalidates_them() {
    let env = env("http://localhost:8080").await;
    let (cookie, _) = setup_session(&env).await;
    let status = |cookie: String| {
        let app = env.app.clone();
        async move {
            app.oneshot(json_request(
                "GET",
                "/api/v1/bootstrap",
                Value::Null,
                &[("cookie", &cookie)],
            ))
            .await
            .unwrap()
            .status()
        }
    };
    assert_eq!(status(cookie.clone()).await, StatusCode::OK);
    // Expiry is enforced by the server, not just by the cookie's Max-Age.
    sqlx::query("UPDATE sessions SET expires_at=?")
        .bind(now() - 1.0)
        .execute(&env.state.0.db)
        .await
        .unwrap();
    assert_eq!(status(cookie.clone()).await, StatusCode::UNAUTHORIZED);
    let auth = send(
        &env.app,
        json_request(
            "GET",
            "/api/v1/auth/status",
            Value::Null,
            &[("cookie", &cookie)],
        ),
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&body_text(auth).await).unwrap()["authenticated"],
        false
    );

    let response = send(
        &env.app,
        json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
    )
    .await;
    let fresh = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(status(fresh.clone()).await, StatusCode::OK);
    let out = send(
        &env.app,
        json_request("POST", "/api/v1/logout", json!({}), &[("cookie", &fresh)]),
    )
    .await;
    assert_eq!(out.status(), StatusCode::OK);
    assert_eq!(
        status(fresh).await,
        StatusCode::UNAUTHORIZED,
        "logout deletes the server-side session"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE expires_at>?")
        .bind(now())
        .fetch_one(&env.state.0.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn login_limit_is_per_client_so_an_attacker_cannot_lock_out_the_administrator() {
    let env = env("http://localhost:8080").await;
    setup_session(&env).await;
    let attacker = "203.0.113.9";
    for _ in 0..10 {
        let r = send(
            &env.app,
            from_ip(
                json_request(
                    "POST",
                    "/api/v1/auth/login",
                    login("wrong-password-123"),
                    &[],
                ),
                attacker,
            ),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    let limited = send(
        &env.app,
        from_ip(
            json_request(
                "POST",
                "/api/v1/auth/login",
                login("wrong-password-123"),
                &[],
            ),
            attacker,
        ),
    )
    .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let text = body_text(limited).await;
    assert!(text.contains("rate_limited") && !text.contains("wrong-password-123"));
    // Even the right password is refused for the abusive address ...
    let still = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
            attacker,
        ),
    )
    .await;
    assert_eq!(still.status(), StatusCode::TOO_MANY_REQUESTS);
    // ... but the administrator from another address is unaffected.
    let admin = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
            "198.51.100.7",
        ),
    )
    .await;
    assert_eq!(admin.status(), StatusCode::OK);
    // A spoofed X-Forwarded-For does not choose the key unless the peer is a trusted proxy.
    let spoof = send(
        &env.app,
        from_ip(
            json_request(
                "POST",
                "/api/v1/auth/login",
                login(PASSWORD),
                &[("x-forwarded-for", "192.0.2.77")],
            ),
            attacker,
        ),
    )
    .await;
    assert_eq!(spoof.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn trusted_proxy_forwarded_address_separates_clients() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::open(
        dir.path().join("p.db").to_str().unwrap(),
        "https://hope.example".into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .unwrap()
    .with_trusted_proxies(TrustedProxies::parse("127.0.0.1").unwrap())
    .unwrap();
    state
        .set_meta("setup_token_hash", &hash("test-setup-token-long-enough"))
        .await
        .unwrap();
    let app = router(state.clone(), "web/out");
    let response = send(
        &app,
        json_request(
            "POST",
            "/api/v1/auth/setup",
            json!({"password":PASSWORD,"setup_token":"test-setup-token-long-enough"}),
            &[],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let via = |client: &str, password: &str| {
        from_ip(
            json_request(
                "POST",
                "/api/v1/auth/login",
                login(password),
                &[("x-forwarded-for", client)],
            ),
            "127.0.0.1",
        )
    };
    for _ in 0..10 {
        send(&app, via("203.0.113.9", "wrong-password-123")).await;
    }
    assert_eq!(
        send(&app, via("203.0.113.9", "wrong-password-123"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(&app, via("198.51.100.7", PASSWORD)).await.status(),
        StatusCode::OK
    );
}

fn enroll_body(token: &str) -> Value {
    json!(EnrollmentRequest {
        schema_version: 1,
        token: token.into(),
        agent_id: Uuid::new_v4().to_string(),
        credential: secret(),
        inventory: Inventory {
            hostname: "h".into(),
            os: "Linux".into(),
            architecture: "x86_64".into(),
            cpu_model: "cpu".into(),
            cpu_cores: 1,
            memory_total_bytes: 1e9,
            interfaces: vec![],
            disks: vec![],
            gpus: vec![],
            collectors: vec![],
        },
    })
}

#[tokio::test]
async fn enrollment_and_report_abuse_is_bounded_per_client() {
    let env = env("http://localhost:8080").await;
    let attacker = "203.0.113.50";
    for _ in 0..20 {
        let r = send(
            &env.app,
            from_ip(
                json_request("POST", "/api/v1/agent/enroll", enroll_body("nope"), &[]),
                attacker,
            ),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    let r = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/agent/enroll", enroll_body("nope"), &[]),
            attacker,
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    // A legitimate agent behind another address still enrolls.
    let token = agents::issue_token(&env.state).await.unwrap();
    let ok = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/agent/enroll", enroll_body(&token), &[]),
            "198.51.100.20",
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);

    let report = |agent: &str, credential: &str| {
        json_request(
            "POST",
            "/api/v1/agent/report",
            json!(AgentReport {
                schema_version: SCHEMA_VERSION,
                report_id: Uuid::new_v4().to_string(),
                agent_id: agent.into(),
                sequence: None,
                collected_at: now(),
                inventory: None,
                metrics: Metrics::default(),
                collectors: vec![],
            }),
            &[("authorization", &format!("Bearer {credential}"))],
        )
    };
    for _ in 0..30 {
        let r = send(
            &env.app,
            from_ip(report(&Uuid::new_v4().to_string(), &secret()), attacker),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    let r = send(
        &env.app,
        from_ip(report(&Uuid::new_v4().to_string(), &secret()), attacker),
    )
    .await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn per_agent_report_ceiling_applies_after_authentication() {
    let env = env("http://localhost:8080").await;
    let token = agents::issue_token(&env.state).await.unwrap();
    let agent = Uuid::new_v4().to_string();
    let credential = secret();
    let mut request = enroll_body(&token);
    request["agent_id"] = json!(agent);
    request["credential"] = json!(credential);
    assert_eq!(
        send(
            &env.app,
            json_request("POST", "/api/v1/agent/enroll", request, &[])
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut last = StatusCode::OK;
    let mut accepted = 0;
    for _ in 0..125 {
        let body = json!(AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: agent.clone(),
            sequence: None,
            collected_at: now(),
            inventory: None,
            metrics: Metrics::default(),
            collectors: vec![],
        });
        last = send(
            &env.app,
            json_request(
                "POST",
                "/api/v1/agent/report",
                body,
                &[("authorization", &format!("Bearer {credential}"))],
            ),
        )
        .await
        .status();
        if last == StatusCode::OK {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 120);
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn malformed_bodies_get_fixed_messages_that_never_echo_input() {
    let env = env("http://localhost:8080").await;
    let marker = "SECRET-marker-value-123456";
    // serde's own error text quotes the offending value; the response must not.
    let attempts = [
        ("/api/v1/auth/login", format!("\"{marker}\"")),
        (
            "/api/v1/agent/enroll",
            format!("{{\"schema_version\":\"{marker}\"}}"),
        ),
        (
            "/api/v1/agent/report",
            format!("{{\"agent_id\":\"{marker}\""),
        ),
        (
            "/api/v1/auth/login",
            format!("{{\"password\":\"{}\"}}", "x".repeat(300 * 1024)),
        ),
    ];
    for (path, body) in attempts {
        let response = send(
            &env.app,
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
        assert!(response.status().is_client_error(), "{path}");
        assert_eq!(response.headers()["content-type"], "application/json");
        let text = body_text(response).await;
        assert!(!text.contains(marker) && text.len() < 200, "{text}");
        assert!(serde_json::from_str::<Value>(&text).unwrap()["error"].is_string());
    }
    let wrong_type = send(
        &env.app,
        Request::post("/api/v1/auth/login")
            .header("content-type", "text/plain")
            .body(Body::from(marker))
            .unwrap(),
    )
    .await;
    assert!(!body_text(wrong_type).await.contains(marker));
}

#[tokio::test]
async fn sse_lag_sends_resync_and_disconnect_releases_the_subscriber() {
    let env = env("http://localhost:8080").await;
    let (cookie, _) = setup_session(&env).await;
    let response = send(
        &env.app,
        json_request("GET", "/api/v1/events", Value::Null, &[("cookie", &cookie)]),
    )
    .await;
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(env.state.0.events.receiver_count(), 1);
    // The broadcast buffer holds 256 messages; overrun it before the client reads anything.
    for _ in 0..400 {
        env.state.changed();
    }
    let mut body = response.into_body();
    let mut seen = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.contains("event: resync") {
            let frame = body.frame().await.expect("stream open").expect("frame");
            if let Ok(data) = frame.into_data() {
                seen.push_str(&String::from_utf8_lossy(&data));
            }
        }
    })
    .await;
    assert!(read.is_ok(), "lagged client must be told to resync: {seen}");
    // Reverse proxy / browser disconnect: dropping the body frees the broadcast receiver.
    drop(body);
    for _ in 0..50 {
        if env.state.0.events.receiver_count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("subscriber leaked after disconnect");
}

#[tokio::test]
async fn shutdown_ends_open_event_streams() {
    let env = env("http://localhost:8080").await;
    let (cookie, _) = setup_session(&env).await;
    let response = send(
        &env.app,
        json_request("GET", "/api/v1/events", Value::Null, &[("cookie", &cookie)]),
    )
    .await;
    let mut body = response.into_body();
    env.state.0.shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(frame) = body.frame().await {
            let _ = frame;
        }
    })
    .await;
    assert!(ended.is_ok(), "stream must end on shutdown");
}

#[tokio::test]
async fn backup_during_writes_is_consistent_and_restorable() {
    let env = env("http://localhost:8080").await;
    let dest_dir = tempfile::tempdir().unwrap();
    // Writers commit pairs of rows atomically; a consistent snapshot never splits a pair.
    let db = env.state.0.db.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let writer = tokio::spawn(async move {
        let mut n = 0u64;
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            n += 1;
            let mut tx = db.begin().await.unwrap();
            for key in ["pair_a", "pair_b"] {
                sqlx::query("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
                    .bind(key).bind(n.to_string()).execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            tokio::task::yield_now().await;
        }
        n
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut backups = vec![];
    for i in 0..5 {
        let path = dest_dir.path().join(format!("b{i}.db"));
        backup::run(&env.db, &path).await.expect("backup");
        backups.push(path);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let written = writer.await.unwrap();
    assert!(written > 5);
    for path in &backups {
        // Standalone file: no WAL sidecars are needed to read it, and it passes integrity checks.
        assert!(!path.with_extension("db-wal").exists());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o077, 0, "backup must be private");
        let restored = State::open(
            path.to_str().unwrap(),
            "http://localhost:8080".into(),
            "releases".into(),
            capabilities(),
        )
        .await
        .expect("restored database opens and migrates");
        let a = restored.meta("pair_a").await.unwrap().expect("pair_a");
        let b = restored.meta("pair_b").await.unwrap().expect("pair_b");
        assert_eq!(a, b, "snapshot split an atomic transaction");
        assert!(a.parse::<u64>().unwrap() <= written);
        restored.0.db.close().await;
    }
    // Refuses to overwrite and to back up a missing database.
    assert!(backup::run(&env.db, &backups[0]).await.is_err());
    assert!(
        backup::run(
            &dest_dir.path().join("none.db"),
            &dest_dir.path().join("x.db")
        )
        .await
        .is_err()
    );
    assert!(!dest_dir.path().join("x.db").exists());
}

#[tokio::test]
async fn backup_subcommand_writes_a_private_openable_copy_of_the_live_database() {
    let env = env("http://localhost:8080").await;
    env.state.set_meta("marker", "kept").await.unwrap();
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("copy.db");
    // The CLI expects `$HOPE_DATA_DIR/hope.db`; the fixture names its database `h.db`.
    let data_dir = tempfile::tempdir().unwrap();
    backup::run(&env.db, &data_dir.path().join("hope.db"))
        .await
        .unwrap();
    let data_dir = data_dir.keep();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_hope-server"))
        .arg("backup")
        .arg(&target)
        .env("HOPE_DATA_DIR", &data_dir)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let copy = State::open(
        target.to_str().unwrap(),
        "http://localhost:8080".into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .unwrap();
    assert_eq!(copy.meta("marker").await.unwrap().as_deref(), Some("kept"));
    // A second run to the same name must not overwrite.
    let again = std::process::Command::new(env!("CARGO_BIN_EXE_hope-server"))
        .arg("backup")
        .arg(&target)
        .env("HOPE_DATA_DIR", &data_dir)
        .output()
        .unwrap();
    assert!(!again.status.success());
}

#[tokio::test]
async fn distributed_failures_cannot_lock_out_the_administrator_and_success_is_free() {
    let env = env("http://localhost:8080").await;
    setup_session(&env).await;
    // 300 distinct sources each fail once (over-long passwords fail without hashing work).
    let long = "x".repeat(300);
    for i in 0..300 {
        let ip = format!("203.0.{}.{}", i / 250, i % 250 + 1);
        let r = send(
            &env.app,
            from_ip(
                json_request("POST", "/api/v1/auth/login", login(&long), &[]),
                &ip,
            ),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    // Successful logins never consume the failure budget: far more than ten in a row work.
    for _ in 0..12 {
        let ok = send(
            &env.app,
            from_ip(
                json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
                "198.51.100.7",
            ),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
    }
    // One failure then success clears the client's failures.
    for _ in 0..9 {
        send(
            &env.app,
            from_ip(
                json_request("POST", "/api/v1/auth/login", login(&long), &[]),
                "198.51.100.8",
            ),
        )
        .await;
    }
    let ok = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
            "198.51.100.8",
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let again = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(&long), &[]),
            "198.51.100.8",
        ),
    )
    .await;
    assert_eq!(
        again.status(),
        StatusCode::UNAUTHORIZED,
        "failure budget was reset by the success"
    );
}

#[tokio::test]
async fn hashing_saturation_is_a_transient_503_not_a_lockout() {
    let env = env("http://localhost:8080").await;
    setup_session(&env).await;
    let held = env.state.0.hashing.acquire_many(2).await.unwrap();
    let busy = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
            "198.51.100.7",
        ),
    )
    .await;
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.headers()["retry-after"], "2");
    drop(held);
    // The refusal did not count as a failure, so the same client logs in immediately afterwards.
    let ok = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/auth/login", login(PASSWORD), &[]),
            "198.51.100.7",
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn valid_agents_are_not_blocked_by_distributed_bad_credentials() {
    let env = env("http://localhost:8080").await;
    let token = agents::issue_token(&env.state).await.unwrap();
    let agent = Uuid::new_v4().to_string();
    let credential = secret();
    let mut request = enroll_body(&token);
    request["agent_id"] = json!(agent);
    request["credential"] = json!(credential);
    assert_eq!(
        send(
            &env.app,
            json_request("POST", "/api/v1/agent/enroll", request, &[])
        )
        .await
        .status(),
        StatusCode::OK
    );
    for i in 0..150 {
        let ip = format!("203.0.{}.{}", i / 250, i % 250 + 1);
        let bad = json_request("POST", "/api/v1/agent/enroll", enroll_body("nope"), &[]);
        send(&env.app, from_ip(bad, &ip)).await;
        let bad = json_request(
            "POST",
            "/api/v1/agent/report",
            json!({"agent_id": agent}),
            &[("authorization", "Bearer nope")],
        );
        send(&env.app, from_ip(bad, &ip)).await;
    }
    let body = json!(AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.clone(),
        sequence: None,
        collected_at: now(),
        inventory: None,
        metrics: Metrics::default(),
        collectors: vec![],
    });
    let ok = send(
        &env.app,
        from_ip(
            json_request(
                "POST",
                "/api/v1/agent/report",
                body,
                &[("authorization", &format!("Bearer {credential}"))],
            ),
            "198.51.100.30",
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let token = agents::issue_token(&env.state).await.unwrap();
    let ok = send(
        &env.app,
        from_ip(
            json_request("POST", "/api/v1/agent/enroll", enroll_body(&token), &[]),
            "198.51.100.31",
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn origin_comparison_is_normalized() {
    let env = env("https://hope.example").await;
    let (cookie, _) = setup_session(&env).await;
    let body = |n: u8| json!({"name":"LAN","cidr":format!("192.168.{n}.0/24")});
    for (i, origin) in [
        "https://HOPE.example",
        "https://hope.example:443",
        "HTTPS://Hope.Example/",
    ]
    .into_iter()
    .enumerate()
    {
        let r = send(
            &env.app,
            json_request(
                "POST",
                "/api/v1/networks",
                body(70 + i as u8),
                &[("cookie", &cookie), ("origin", origin)],
            ),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "{origin}");
    }
    for origin in [
        "https://hope.example:8443",
        "http://hope.example",
        "https://hope.example.evil",
        "https://evil.example",
    ] {
        let r = send(
            &env.app,
            json_request(
                "POST",
                "/api/v1/networks",
                body(90),
                &[("cookie", &cookie), ("origin", origin)],
            ),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{origin}");
    }
}

#[tokio::test]
async fn startup_reconciles_scans_left_running_by_a_crash() {
    let env = env("http://localhost:8080").await;
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled) VALUES('n','n','192.0.2.0/24',0)")
        .execute(&env.state.0.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO scans(id,network_id,cidr,status,total,started_at) VALUES('s','n','192.0.2.0/24','running',254,0)").execute(&env.state.0.db).await.unwrap();
    env.state.0.db.close().await;
    let reopened = State::open(
        env.db.to_str().unwrap(),
        "http://localhost:8080".into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM scans WHERE id='s'")
        .fetch_one(&reopened.0.db)
        .await
        .unwrap();
    assert_eq!(status, "interrupted");
}

#[tokio::test]
async fn work_requested_after_shutdown_begins_is_refused_and_leaves_no_running_rows() {
    let env = env("http://localhost:8080").await;
    let (cookie, _) = setup_session(&env).await;
    let net = send(
        &env.app,
        json_request(
            "POST",
            "/api/v1/networks",
            json!({"name":"n","cidr":"127.0.0.0/24","enabled":false}),
            &[("cookie", &cookie)],
        ),
    )
    .await;
    let id = serde_json::from_str::<Value>(&body_text(net).await).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.state.0.shutdown.cancel();
    let r = send(
        &env.app,
        json_request(
            "POST",
            &format!("/api/v1/networks/{id}/scan"),
            json!({}),
            &[("cookie", &cookie)],
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM scans WHERE status IN ('queued','running')")
            .fetch_one(&env.state.0.db)
            .await
            .unwrap();
    assert_eq!(running, 0);
}
