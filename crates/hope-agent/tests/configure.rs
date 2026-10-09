use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fixture() -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let mut credentials = vec![];
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("connection");
            let mut bytes = vec![];
            let (header_end, length) = loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await.expect("request");
                assert!(count > 0 && bytes.len() + count <= 256 * 1024);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).expect("headers");
                    assert!(headers.starts_with("POST /api/v1/agent/enroll "));
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("length"))
                        })
                        .expect("content length");
                    break (end + 4, length);
                }
            };
            assert!(length < 256 * 1024);
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await.expect("body");
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
            }
            let body: Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).expect("body JSON");
            credentials.push(
                body["credential"]
                    .as_str()
                    .expect("pending credential")
                    .to_string(),
            );
            let (status, response) = if attempt == 0 {
                ("410 Gone", "{}".into())
            } else {
                (
                    "200 OK",
                    json!({"credential":body["credential"],"report_interval_seconds":30})
                        .to_string(),
                )
            };
            stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.expect("response");
        }
        credentials
    });
    (format!("http://{address}"), task)
}
async fn configure(directory: &Path, reset: bool) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hope-agent"));
    command
        .arg("configure")
        .env("HOPE_AGENT_DIR", directory)
        .env("HOPE_ENROLLMENT_TOKEN", "fixture-token")
        .env_remove("HOPE_CA_CERT")
        .env_remove("HOPE_SERVER")
        .env_remove("HOPE_RESET_CREDENTIAL");
    if reset {
        command.env("HOPE_RESET_CREDENTIAL", "1");
    }
    tokio::time::timeout(Duration::from_secs(20), command.kill_on_drop(true).output())
        .await
        .expect("bounded configure")
        .expect("agent process")
}
#[tokio::test]
async fn failed_credential_replacement_preserves_old_secret_and_pending_retry_identity() {
    let directory = tempfile::tempdir().expect("temporary state");
    let (server, task) = fixture().await;
    let identity = uuid::Uuid::new_v4().to_string();
    let old_secret = "old-fixture-credential";
    tokio::fs::write(directory.path().join("config.json"), json!({"server":server,"agent_id":identity,"credential":old_secret,"pending_credential":null,"ca_pem":null}).to_string()).await.expect("config");
    let rejected = configure(directory.path(), true).await;
    assert!(!rejected.status.success());
    let first: Value = serde_json::from_slice(
        &tokio::fs::read(directory.path().join("config.json"))
            .await
            .expect("retained config"),
    )
    .expect("config JSON");
    assert!(
        first["credential"].as_str() == Some(old_secret),
        "Failed replacement must preserve the old credential"
    );
    assert!(
        first["agent_id"] == identity,
        "Identity must survive failure"
    );
    assert!(first["pending_credential"].as_str().is_some());
    let retried = configure(directory.path(), true).await;
    assert!(retried.status.success(), "Credential retry must succeed");
    let credentials = task.await.expect("fixture");
    assert!(
        credentials[0] == credentials[1],
        "Retry must reuse the persisted pending credential"
    );
    let second: Value = serde_json::from_slice(
        &tokio::fs::read(directory.path().join("config.json"))
            .await
            .expect("updated config"),
    )
    .expect("config JSON");
    assert!(second["credential"].as_str() == Some(credentials[0].as_str()));
    assert!(second["pending_credential"].is_null());
    assert!(
        !directory.path().join("last-acceptance.json").exists(),
        "Configuration cannot confirm daemon reporting"
    );
    assert!(!String::from_utf8_lossy(&retried.stdout).contains(&credentials[0]));
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains(old_secret));
}
