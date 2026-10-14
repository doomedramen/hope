//! Spawns the real server binary, starts a scan and an event stream, sends SIGTERM and checks a
//! clean exit inside the 8 second budget with the scan persisted as interrupted.
use std::{
    io::{Read, Write},
    net::TcpStream,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn http(port: u16, method: &str, path: &str, cookie: &str, body: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\ncookie: {cookie}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut text = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => text.push_str(&String::from_utf8_lossy(&buffer[..n])),
        }
    }
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_string(), rest.to_string())
}

#[test]
fn sigterm_exits_cleanly_closes_streams_and_interrupts_scans() {
    let dir = tempfile::tempdir().unwrap();
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_hope-server"))
        .env("HOPE_BIND", format!("127.0.0.1:{port}"))
        .env("HOPE_SERVER_URL", format!("http://127.0.0.1:{port}"))
        .env("HOPE_DATA_DIR", dir.path())
        .env("HOPE_WEB_DIR", dir.path())
        .env("HOPE_SETUP_TOKEN", "shutdown-test-setup-token-long")
        .env("HOPE_DISCOVERY_MODE", "limited")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    let up = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            up.elapsed() < Duration::from_secs(20),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let (status, head, _) = http(
        port,
        "POST",
        "/api/v1/auth/setup",
        "",
        r#"{"password":"a-long-test-password","setup_token":"shutdown-test-setup-token-long"}"#,
    );
    assert_eq!(status, 200);
    let cookie = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .and_then(|l| l.split_once(' '))
        .map(|(_, v)| v.split(';').next().unwrap().to_string())
        .expect("session cookie");
    // A loopback /20 (4,094 addresses at <= 200 probe starts per second) stays running for ~20 s.
    let (status, _, body) = http(
        port,
        "POST",
        "/api/v1/networks",
        &cookie,
        r#"{"name":"loop","cidr":"127.0.0.0/20"}"#,
    );
    assert_eq!(status, 200, "{body}");
    let network: serde_json::Value = serde_json::from_str(
        body.trim_start_matches(|c| c != '{')
            .trim_end_matches(|c| c != '}'),
    )
    .unwrap();
    let id = network["id"].as_str().unwrap().to_string();
    let (status, _, body) = http(
        port,
        "POST",
        &format!("/api/v1/networks/{id}/scan"),
        &cookie,
        "{}",
    );
    assert_eq!(status, 200, "{body}");

    // Open an event stream and keep it open.
    let mut sse = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        sse,
        "GET /api/v1/events HTTP/1.1\r\nhost: 127.0.0.1\r\ncookie: {cookie}\r\naccept: text/event-stream\r\n\r\n"
    )
    .unwrap();
    sse.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut first = [0u8; 512];
    let n = sse.read(&mut first).unwrap();
    assert!(String::from_utf8_lossy(&first[..n]).contains("text/event-stream"));

    let signalled = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    // The open stream is closed by the server (EOF), not left for the client to time out.
    let mut closed = false;
    let mut buffer = [0u8; 512];
    while signalled.elapsed() < Duration::from_secs(8) {
        match sse.read(&mut buffer) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(closed, "SSE stream was not closed on SIGTERM");
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            signalled.elapsed() < Duration::from_secs(8),
            "server did not exit in time"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
    assert!(signalled.elapsed() < Duration::from_secs(8));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use sqlx::Connection;
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(dir.path().join("hope.db"))
                .read_only(true),
        )
        .await
        .unwrap();
        let rows: Vec<(String,)> = sqlx::query_as("SELECT status FROM scans")
            .fetch_all(&mut db)
            .await
            .unwrap();
        assert_eq!(rows, vec![("interrupted".to_string(),)]);
    });
    // The WAL was checkpointed and removed on a clean close.
    assert!(
        !dir.path().join("hope.db-wal").exists()
            || std::fs::metadata(dir.path().join("hope.db-wal"))
                .unwrap()
                .len()
                == 0
    );
}
