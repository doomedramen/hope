use axum::{Json, Router, http::StatusCode, routing::post};
use hope_protocol::{EnrollmentRequest, EnrollmentResponse, ReportResponse};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn revocation_survives_process_restarts_until_successful_credential_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let revoked = Arc::new(AtomicBool::new(true));
    let reports = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/api/v1/agent/enroll", post(|Json(request): Json<EnrollmentRequest>| async move {
            Json(EnrollmentResponse { credential: request.credential, report_interval_seconds: 30 })
        }))
        .route("/api/v1/agent/report", post({
            let (revoked, reports) = (revoked.clone(), reports.clone());
            move || {
                let (revoked, reports) = (revoked.clone(), reports.clone());
                async move {
                    reports.fetch_add(1, Ordering::SeqCst);
                    if revoked.load(Ordering::SeqCst) {
                        (StatusCode::UNAUTHORIZED, Json(json!({"code":"agent_revoked", "error":"Agent credential revoked"})))
                    } else {
                        (StatusCode::OK, Json(serde_json::to_value(ReportResponse {
                            live_applied: None,
                            report_sequence: None,
                            accepted: true, duplicate: false, report_interval_seconds: None, update_to: None, update_release_id: None,
                        }).unwrap()))
                    }
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config_path = directory.path().join("config.json");
    // Legacy installations have no disabled field and must still start normally.
    tokio::fs::write(
        &config_path,
        serde_json::to_vec(&json!({
            "server": server, "agent_id": uuid::Uuid::new_v4().to_string(),
            "credential": "a".repeat(64), "pending_credential": null, "ca_pem": null,
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let run = |operation: &str, reset: bool| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hope-agent"));
        command
            .arg(operation)
            .env("HOPE_AGENT_DIR", directory.path())
            .env("HOPE_ENROLLMENT_TOKEN", "fixture-reset-token")
            .env_remove("HOPE_SERVER")
            .env_remove("HOPE_CA_CERT")
            .env_remove("HOPE_MONITORS")
            .env_remove("HOPE_RESET_CREDENTIAL")
            .kill_on_drop(true);
        if reset {
            command.env("HOPE_RESET_CREDENTIAL", "1");
        }
        async move {
            tokio::time::timeout(Duration::from_secs(30), command.output())
                .await
                .unwrap()
                .unwrap()
        }
    };
    let first = run("run", false).await;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&config_path).await.unwrap()).unwrap();
    assert_eq!(saved["disabled"], true);
    assert_eq!(
        tokio::fs::read(directory.path().join("spool.json"))
            .await
            .unwrap(),
        b"[]"
    );
    assert_eq!(reports.load(Ordering::SeqCst), 1);

    // Both a service restart and a manual report stay offline, even if the server would accept it.
    revoked.store(false, Ordering::SeqCst);
    assert!(run("run", false).await.status.success());
    assert!(!run("once", false).await.status.success());
    assert!(!run("configure", false).await.status.success());
    assert!(!run("status", false).await.status.success());
    assert_eq!(reports.load(Ordering::SeqCst), 1);

    let reset = run("configure", true).await;
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&config_path).await.unwrap()).unwrap();
    assert_eq!(saved["disabled"], false);
    assert_ne!(saved["credential"], "a".repeat(64));
    assert!(run("once", false).await.status.success());
    assert_eq!(reports.load(Ordering::SeqCst), 2);
    task.abort();
}
