use axum::{Json, Router, routing::post};
use hope_protocol::{AgentReport, EnrollmentRequest, EnrollmentResponse, ReportResponse};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};

async fn monitor() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(
            include_bytes!("fixtures/tls/server.der").to_vec(),
        )],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("fixtures/tls/server-key.der").to_vec(),
        )),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                let mut request = [0; 4096];
                if stream.read(&mut request).await.is_ok() {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                }
            })
            .await;
        }
    });
    (format!("https://{address}/health"), task)
}

#[tokio::test]
async fn first_reports_after_enrollment_or_ca_update_trust_the_new_monitor_ca() {
    let directory = tempfile::tempdir().unwrap();
    let (monitor_url, monitor_task) = monitor().await;
    let (sender, mut reports) = tokio::sync::mpsc::channel(4);
    let app = Router::new()
        .route(
            "/api/v1/agent/enroll",
            post(|Json(request): Json<EnrollmentRequest>| async move {
                Json(EnrollmentResponse {
                    credential: request.credential,
                    report_interval_seconds: 30,
                })
            }),
        )
        .route(
            "/api/v1/agent/report",
            post(move |Json(report): Json<AgentReport>| {
                let sender = sender.clone();
                async move {
                    sender.send(report).await.unwrap();
                    Json(ReportResponse {
                        live_applied: None,
                        report_sequence: None,
                        accepted: true,
                        duplicate: false,
                        report_interval_seconds: Some(30),
                        update_to: None,
                        update_release_id: None,
                    })
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for existing in [false, true] {
        if existing {
            let path = directory.path().join("config.json");
            let mut saved: serde_json::Value =
                serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
            saved["ca_pem"] = serde_json::Value::Null;
            tokio::fs::write(path, serde_json::to_vec(&saved).unwrap())
                .await
                .unwrap();
        }
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hope-agent"))
                .arg("enroll")
                .env("HOPE_AGENT_DIR", directory.path())
                .env("HOPE_SERVER", format!("http://{address}"))
                .env("HOPE_ENROLLMENT_TOKEN", "local-fixture-token")
                .env(
                    "HOPE_CA_CERT",
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls/ca.pem"),
                )
                .env("HOPE_MONITORS", &monitor_url)
                .env_remove("HOPE_RESET_CREDENTIAL")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "Enrollment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = reports.recv().await.expect("first report");
        let result = report
            .metrics
            .monitors
            .iter()
            .find(|result| result.target == monitor_url)
            .expect("configured monitor");
        assert!(
            result.up,
            "The first report must trust the CA supplied for enrollment: {:?}",
            result.detail
        );
        assert_eq!(result.status_code, Some(200));
    }
    server_task.abort();
    monitor_task.abort();
}
