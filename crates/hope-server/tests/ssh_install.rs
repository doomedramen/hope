//! SSH installation jobs against an isolated in-process SSH server on 127.0.0.1.
//! The server executes nothing: its "installer" is a script of output, delays and simulated agent
//! enrollment, so no real installer, service or host is touched.
use axum::{
    Json, Router,
    body::Body,
    extract::State as Extract,
    http::{HeaderMap, HeaderValue, Request, StatusCode},
};
use futures::future::BoxFuture;
use hope_protocol::*;
use hope_server::{
    agents, router, ssh,
    state::{State, hash, now, secret},
};
use http_body_util::BodyExt;
use russh::{
    Channel, ChannelId,
    keys::{Algorithm, PrivateKey, PublicKey, ssh_key::LineEnding},
    server::{self, Auth, Handler, Msg, Session},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-horse-battery-staple-9137";
const PASSPHRASE: &str = "key-passphrase-7731-unique";
const USER: &str = "installer";

type Hook = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;
#[derive(Clone)]
enum Step {
    Out(String),
    Err(String),
    Sleep(Duration),
    Run(Hook),
    Exit(u32),
    CloseWithoutStatus,
    DropConnection,
}
fn err(line: &str) -> Step {
    Step::Err(format!("{line}\n"))
}

#[derive(Default)]
struct Observed {
    auth_attempts: usize,
    connections: usize,
    commands: Vec<String>,
    exec_requests: Vec<String>,
}
struct Shared {
    observed: Mutex<Observed>,
    script: Mutex<Vec<Step>>,
    authorized: PublicKey,
}
struct Conn {
    shared: Arc<Shared>,
    stdin: Vec<u8>,
}
impl Handler for Conn {
    type Error = russh::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.shared.observed.lock().unwrap().auth_attempts += 1;
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.shared.observed.lock().unwrap().auth_attempts += 1;
        Ok(Auth::Accept)
    }
    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        self.shared.observed.lock().unwrap().auth_attempts += 1;
        Ok(
            if user == USER && key.key_data() == self.shared.authorized.key_data() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }
    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shared
            .observed
            .lock()
            .unwrap()
            .exec_requests
            .push(String::from_utf8_lossy(data).into());
        session.channel_success(channel)?;
        Ok(())
    }
    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.stdin.extend_from_slice(data);
        Ok(())
    }
    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(&self.stdin).to_string();
        self.shared
            .observed
            .lock()
            .unwrap()
            .commands
            .push(command.clone());
        let steps = self.shared.script.lock().unwrap().clone();
        let handle = session.handle();
        tokio::spawn(async move {
            for step in steps {
                match step {
                    Step::Out(text) => {
                        let _ = handle.data(channel, text.into_bytes()).await;
                    }
                    Step::Err(text) => {
                        let _ = handle.extended_data(channel, 1, text.into_bytes()).await;
                    }
                    Step::Sleep(d) => tokio::time::sleep(d).await,
                    Step::Run(hook) => hook(command.clone()).await,
                    Step::Exit(code) => {
                        let _ = handle.exit_status_request(channel, code).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.close(channel).await;
                    }
                    Step::CloseWithoutStatus => {
                        let _ = handle.close(channel).await;
                    }
                    Step::DropConnection => {
                        let _ = handle
                            .disconnect(
                                russh::Disconnect::ByApplication,
                                "gone".into(),
                                "en".into(),
                            )
                            .await;
                    }
                }
            }
        });
        Ok(())
    }
}

struct Fixture {
    _dir: TempDir,
    db: std::path::PathBuf,
    state: State,
    app: Router,
    cookie: String,
    port: u16,
    shared: Arc<Shared>,
    host_fingerprint: String,
    client_key: PrivateKey,
    host_id: String,
}
fn capabilities() -> Capabilities {
    Capabilities {
        platform: "test".into(),
        discovery_mode: "limited".into(),
        arp: false,
        icmp: false,
        limitations: vec![],
    }
}
async fn fixture_with(agent: Option<(&str, &str)>, ca: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().expect("temporary database");
    let db = dir.path().join("ssh.db");
    let state = State::open_with_agent_ca(
        db.to_str().expect("path"),
        "https://hope.example".into(),
        "releases".into(),
        capabilities(),
        ca.map(str::to_string),
    )
    .await
    .expect("state");
    state
        .set_meta("setup_token_hash", &hash("test-setup-token-long-enough"))
        .await
        .expect("setup token");
    let app = router(state.clone(), "web/out");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/setup")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"password":"a-long-test-password","setup_token":"test-setup-token-long-enough"})
                        .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("session")
        .to_string();
    let host_key = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).expect("host key");
    let host_fingerprint = host_key
        .public_key()
        .fingerprint(Default::default())
        .to_string();
    let client_key =
        PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).expect("client key");
    let shared = Arc::new(Shared {
        observed: Mutex::default(),
        script: Mutex::default(),
        authorized: client_key.public_key().clone(),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let port = listener.local_addr().expect("address").port();
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::from_millis(1),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        inactivity_timeout: None,
        ..Default::default()
    });
    let accept_shared = shared.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            accept_shared.observed.lock().unwrap().connections += 1;
            let conn = Conn {
                shared: accept_shared.clone(),
                stdin: vec![],
            };
            let config = config.clone();
            tokio::spawn(async move {
                if let Ok(session) = server::run_stream(config, stream, conn).await {
                    let _ = session.await;
                }
            });
        }
    });
    let host_id = Uuid::new_v4().to_string();
    let mut host = Host {
        id: host_id.clone(),
        agent_id: None,
        name: "target".into(),
        addresses: vec!["127.0.0.1".into()],
        macs: vec!["02:00:00:00:00:77".into()],
        network_ids: vec![],
        last_seen: 0.0,
        last_report: None,
        last_collected_at: None,
        agent_revoked: false,
        enrichment_status: EnrichmentStatus::Pending,
        discovery_method: "tcp".into(),
        inventory: None,
        metrics: None,
        collectors: vec![],
        services: vec![],
    };
    if let Some((agent, credential)) = agent {
        sqlx::query("INSERT INTO agents(id,credential_hash) VALUES(?,?)")
            .bind(agent)
            .bind(hash(credential))
            .execute(&state.0.db)
            .await
            .expect("agent");
        host.agent_id = Some(agent.into());
        host.last_report = Some(now() - 3600.0);
    }
    state.save_host(&host).await.expect("host");
    ssh::set_limits(
        &state,
        ssh::Limits {
            connect: Duration::from_secs(5),
            inactivity: Duration::from_secs(5),
            total: Duration::from_secs(20),
            report_wait: Duration::from_secs(2),
            max_line: 256,
            max_bytes: 64 * 1024,
            panic_for_test: false,
        },
    );
    Fixture {
        _dir: dir,
        db,
        state,
        app,
        cookie,
        port,
        shared,
        host_fingerprint,
        client_key,
        host_id,
    }
}
async fn fixture() -> Fixture {
    fixture_with(None, None).await
}
impl Fixture {
    /// Enrollment replaces the host's addresses with the agent's non-loopback ones.
    async fn restore_address(&self) {
        let mut host = self.state.host(&self.host_id).await.unwrap().unwrap();
        host.addresses = vec!["127.0.0.1".into()];
        self.state.save_host(&host).await.unwrap();
    }
    fn script(&self, steps: Vec<Step>) {
        *self.shared.script.lock().unwrap() = steps;
    }
    async fn call(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("cookie", &self.cookie)
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
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
    fn request(&self, fingerprint: &str) -> Value {
        json!({"host_id": self.host_id, "address": "127.0.0.1", "port": self.port,
               "username": USER, "fingerprint": fingerprint})
    }
    fn with(&self, fingerprint: &str, extra: Value) -> Value {
        let mut body = self.request(fingerprint);
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body
    }
    fn password_request(&self) -> Value {
        self.with(&self.host_fingerprint, json!({"password": PASSWORD}))
    }
    fn key_pem(&self) -> String {
        self.client_key
            .to_openssh(LineEnding::LF)
            .unwrap()
            .to_string()
    }
    async fn run(&self, body: Value) -> Value {
        let (status, job) = self.call("POST", "/api/v1/ssh/install", body).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        assert_eq!(job["status"], "running");
        self.wait(job["id"].as_str().unwrap()).await
    }
    async fn wait(&self, id: &str) -> Value {
        for _ in 0..400 {
            let (status, job) = self
                .call("GET", &format!("/api/v1/ssh/jobs/{id}"), json!({}))
                .await;
            assert_eq!(status, StatusCode::OK);
            if job["status"] != "running" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job did not finish");
    }
    fn auth_attempts(&self) -> usize {
        self.shared.observed.lock().unwrap().auth_attempts
    }
    fn commands(&self) -> Vec<String> {
        self.shared.observed.lock().unwrap().commands.clone()
    }
}

fn token_of(command: &str) -> String {
    let marker = "HOPE_ENROLLMENT_TOKEN='";
    let start = command.find(marker).expect("token in command") + marker.len();
    command[start..start + 64].to_string()
}
fn inventory() -> Inventory {
    inventory_with("02:00:00:00:00:77", &["127.0.0.1", "192.168.1.77"])
}
fn inventory_with(mac: &str, addresses: &[&str]) -> Inventory {
    Inventory {
        hostname: "target".into(),
        os: "Linux test".into(),
        architecture: "aarch64".into(),
        cpu_model: "Test CPU".into(),
        cpu_cores: 4,
        memory_total_bytes: 8e9,
        interfaces: vec![Interface {
            name: "eth0".into(),
            mac: Some(mac.into()),
            addresses: addresses.iter().map(|a| a.to_string()).collect(),
            gateway: None,
        }],
        disks: vec![],
        gpus: vec![],
        collectors: vec![],
    }
}
async fn send_report(state: &State, agent: &str, credential: &str) {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {credential}")).unwrap(),
    );
    let report = AgentReport {
        schema_version: SCHEMA_VERSION,
        report_id: Uuid::new_v4().to_string(),
        agent_id: agent.into(),
        collected_at: now(),
        inventory: None,
        metrics: Metrics {
            cpu_percent: Some(1.0),
            memory_used_bytes: Some(1e6),
            memory_total_bytes: Some(8e9),
            ..Default::default()
        },
        collectors: vec![],
    };
    let _ = agents::report(
        Extract(state.clone()),
        axum::extract::Extension(hope_server::limits::ClientKey("test".into())),
        headers,
        Json(report),
    )
    .await
    .expect("report accepted");
}
/// Simulates the installed agent: enrolls with the job's token, then reports.
fn enroll_and_report(state: &State, agent: &str, credential: &str) -> Step {
    enroll_with(state, agent, credential, inventory)
}
fn enroll_with(
    state: &State,
    agent: &str,
    credential: &str,
    make_inventory: fn() -> Inventory,
) -> Step {
    let (state, agent, credential) = (state.clone(), agent.to_string(), credential.to_string());
    Step::Run(Arc::new(move |command| {
        let (state, agent, credential) = (state.clone(), agent.clone(), credential.clone());
        Box::pin(async move {
            // Real services start seconds after the `activate` line; let the job observe it first.
            tokio::time::sleep(Duration::from_millis(400)).await;
            let _ = agents::enroll(
                Extract(state.clone()),
                axum::extract::Extension(hope_server::limits::ClientKey("test".into())),
                Json(EnrollmentRequest {
                    schema_version: SCHEMA_VERSION,
                    token: token_of(&command),
                    agent_id: agent.clone(),
                    credential: credential.clone(),
                    inventory: make_inventory(),
                }),
            )
            .await
            .expect("enrolled");
            send_report(&state, &agent, &credential).await;
        })
    }))
}
/// Simulates a reinstalled agent that keeps its credential and sends a fresh report.
fn report_only(state: &State, agent: &str, credential: &str) -> Step {
    let (state, agent, credential) = (state.clone(), agent.to_string(), credential.to_string());
    Step::Run(Arc::new(move |_| {
        let (state, agent, credential) = (state.clone(), agent.clone(), credential.clone());
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            send_report(&state, &agent, &credential).await
        })
    }))
}
fn phases(names: &[&str]) -> Vec<Step> {
    names
        .iter()
        .map(|n| err(&format!("hope-install: phase={n}")))
        .collect()
}
fn happy(fixture: &Fixture, agent: &str, credential: &str) -> Vec<Step> {
    let mut steps = phases(&[
        "preflight",
        "stage",
        "verify",
        "stop",
        "place-binary",
        "configure",
        "write-unit",
        "activate",
    ]);
    steps.push(enroll_and_report(&fixture.state, agent, credential));
    steps.extend(phases(&["wait-report", "complete"]));
    steps.push(Step::Exit(0));
    steps
}
/// Raw bytes of the database and its WAL, to prove secrets were never written.
fn database_bytes(fixture: &Fixture) -> Vec<u8> {
    let mut bytes = vec![];
    for suffix in ["", "-wal", "-shm"] {
        let mut path = fixture.db.clone().into_os_string();
        path.push(suffix);
        if let Ok(data) = std::fs::read(path) {
            bytes.extend(data);
        }
    }
    bytes
}
fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    // russh logs through the `log` crate; bridge it so its output lands in the same buffer.
    static BRIDGE: std::sync::Once = std::sync::Once::new();
    BRIDGE.call_once(|| {
        let _ = tracing_log::LogTracer::init();
        log::set_max_level(log::LevelFilter::Trace);
    });
    let buffer = LogBuffer::default();
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buffer.clone())
            .finish(),
    );
    (buffer, guard)
}

#[tokio::test]
async fn fingerprint_discovery_is_separate_from_trust_and_sends_no_credentials() {
    let f = fixture().await;
    let (status, body) = f
        .call(
            "POST",
            "/api/v1/ssh/fingerprint",
            json!({"host_id": f.host_id, "address": "127.0.0.1", "port": f.port}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fingerprint"], f.host_fingerprint);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(f.auth_attempts(), 0);
    assert!(f.commands().is_empty());
    // A job for an address outside the selected host is rejected before connecting.
    let (status, _) = f
        .call(
            "POST",
            "/api/v1/ssh/install",
            json!({"host_id": f.host_id, "address": "10.9.9.9", "port": f.port, "username": USER,
                   "fingerprint": f.host_fingerprint, "password": PASSWORD}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verified_key_installs_and_succeeds_only_after_an_accepted_report() {
    let (logs, _guard) = capture_logs();
    let f = fixture().await;
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    f.script(happy(&f, &agent, &credential));
    let job = f
        .run(f.with(&f.host_fingerprint, json!({"private_key": f.key_pem()})))
        .await;
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["agent_id"], agent.as_str());
    assert_eq!(job["mode"], "install");
    assert_eq!(job["host"], "127.0.0.1");
    assert_eq!(job["fingerprint"], f.host_fingerprint.as_str());
    assert!(job["error_code"].is_null());
    let stages: Vec<_> = job["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    for expected in [
        "connecting",
        "authenticating",
        "preflight",
        "activate",
        "complete",
        "confirming_report",
    ] {
        assert!(stages.contains(&expected), "{expected} in {stages:?}");
    }
    let observed = f.shared.observed.lock().unwrap();
    assert_eq!(observed.exec_requests, vec!["sh -s"]);
    // Token travels on stdin and only on stdin; the command is the existing install_command shape.
    assert!(observed.commands[0].contains("HOPE_SERVER='https://hope.example'"));
    assert!(observed.commands[0].contains("/agent/install"));
    drop(observed);
    let token = token_of(&f.commands()[0]);
    let bytes = database_bytes(&f);
    for secret_text in [token.as_str(), credential.as_str(), &f.key_pem()] {
        assert!(!contains(&bytes, secret_text));
    }
    // Only the token's hash is stored, so the absence checks above are not vacuous.
    assert!(contains(&bytes, &hash(&token)));
    let log = logs.0.lock().unwrap().clone();
    assert!(contains(&log, "russh"), "russh log output must be captured");
    assert!(!contains(&log, &token) && !contains(&log, &credential));
    assert!(!String::from_utf8_lossy(&logs.0.lock().unwrap()).contains(&token));
}

#[tokio::test]
async fn changed_host_key_fails_before_any_authentication_material_is_sent() {
    let f = fixture().await;
    f.script(vec![Step::Exit(0)]);
    let wrong = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let job = f.run(f.with(wrong, json!({"password": PASSWORD}))).await;
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error_code"], "HOST_KEY_MISMATCH");
    assert!(job["guidance"].as_str().unwrap().contains("fingerprint"));
    assert_eq!(
        f.auth_attempts(),
        0,
        "no authentication request may reach the server"
    );
    assert!(f.commands().is_empty());
    assert_eq!(f.shared.observed.lock().unwrap().connections, 1);
}

#[tokio::test]
async fn wrong_password_wrong_key_and_wrong_passphrase_fail_without_running_anything() {
    let (logs, _guard) = capture_logs();
    let f = fixture().await;
    f.script(vec![Step::Exit(0)]);
    let bad_password = "definitely-not-the-password-5521";
    let job = f
        .run(f.with(&f.host_fingerprint, json!({"password": bad_password})))
        .await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("failed"), Some("AUTH_FAILED"))
    );

    let other = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
    let other_pem = other.to_openssh(LineEnding::LF).unwrap().to_string();
    let job = f
        .run(f.with(&f.host_fingerprint, json!({"private_key": other_pem})))
        .await;
    assert_eq!(job["error_code"], "AUTH_FAILED");
    assert!(f.commands().is_empty());

    let connections = f.shared.observed.lock().unwrap().connections;
    let encrypted = f
        .client_key
        .encrypt(&mut rand010::rng(), PASSPHRASE)
        .unwrap()
        .to_openssh(LineEnding::LF)
        .unwrap()
        .to_string();
    let wrong_passphrase = "this-is-the-wrong-passphrase-8842";
    let job = f
        .run(f.with(
            &f.host_fingerprint,
            json!({"private_key": encrypted, "passphrase": wrong_passphrase}),
        ))
        .await;
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error_code"], "KEY_INVALID");
    assert_eq!(
        f.shared.observed.lock().unwrap().connections,
        connections,
        "an undecryptable key must not even open a connection"
    );
    // The correct passphrase works for the same key.
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    f.script(happy(&f, &agent, &credential));
    let job = f
        .run(f.with(
            &f.host_fingerprint,
            json!({"private_key": encrypted, "passphrase": PASSPHRASE}),
        ))
        .await;
    assert_eq!(job["status"], "completed", "{job}");
    let bytes = database_bytes(&f);
    let log = logs.0.lock().unwrap().clone();
    for secret_text in [
        bad_password,
        wrong_passphrase,
        PASSPHRASE,
        PASSWORD,
        encrypted.as_str(),
    ] {
        assert!(!contains(&bytes, secret_text), "database leaked a secret");
        assert!(!contains(&log, secret_text), "logs leaked a secret");
    }
    let (_, jobs) = f.call("GET", "/api/v1/ssh/jobs", json!({})).await;
    assert!(!jobs.to_string().contains(PASSWORD));
}

#[tokio::test]
async fn sudo_denied_user_fallback_and_custom_ca_are_handled() {
    let f = fixture_with(None, Some("/etc/hope/custom-ca.pem")).await;
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    let mut steps = vec![
        err(
            "hope-install: notice=Non-interactive sudo is unavailable, so a user service will be installed instead of a system service.",
        ),
        err("hope-install: notice=<script>alert(1)</script> sudo password is hunter2"),
    ];
    steps.extend(happy(&f, &agent, &credential));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "completed", "{job}");
    let notice = job["notice"].as_str().unwrap();
    assert!(notice.contains("user service") && !notice.contains("hunter2"));
    let command = &f.commands()[0];
    assert!(command.contains("HOPE_CA_CERT='/etc/hope/custom-ca.pem'"));
    assert!(command.contains("--cacert '/etc/hope/custom-ca.pem'"));
    assert!(
        !command.contains("sudo"),
        "no password is ever piped to sudo"
    );
    assert!(!contains(&database_bytes(&f), "hunter2"));
}

#[tokio::test]
async fn ssh_jobs_require_persistent_service_and_linger_refusal_is_a_vetted_definite_failure() {
    let f = fixture().await;
    f.script(vec![
        err("hope-install: phase=preflight"),
        err(
            "hope-install: error=LINGER_REQUIRED lingering is off for hopeplain; token=leak password=hunter2",
        ),
        Step::Exit(1),
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(job["error_code"], "LINGER_REQUIRED");
    assert!(
        job["message"]
            .as_str()
            .unwrap()
            .contains("Nothing was installed")
    );
    assert!(job["guidance"].as_str().unwrap().contains("enable-linger"));
    assert!(!job.to_string().contains("hunter2") && !job.to_string().contains("leak"));
    let command = &f.commands()[0];
    assert!(
        command.contains(" HOPE_REQUIRE_PERSISTENT_SERVICE=1 sh "),
        "{command}"
    );
    assert!(
        job["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s != "complete")
    );
}

#[tokio::test]
async fn progress_errors_map_to_vetted_messages_and_untrusted_lines_are_dropped() {
    let (logs, _guard) = capture_logs();
    let f = fixture().await;
    let fake_token = "tok_live_0123456789abcdef";
    let fake_password = "hunter2-supersecret";
    f.script(vec![
        err(&format!("password={fake_password} token={fake_token}")),
        err(&format!("hope-install: error=NOT_A_REAL_CODE {fake_token}")),
        err(&format!("hope-install: phase=complete {fake_password}")),
        err("hope-install: phase=preflight"),
        Step::Out(format!("{}\n", "x".repeat(10_000) + fake_password)),
        Step::Out("hope-install: error=BAD-SIGNATURE token=\u{1b}[31mleak\n".into()),
        Step::Out("hope-install: error=RELEASE_UNAVAILABLE No signed package; token=leak\n".into()),
        err("hope-install: error=ROLLED_BACK Previous installation restored"),
        Step::Exit(1),
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(job["error_code"], "RELEASE_UNAVAILABLE");
    let message = job["message"].as_str().unwrap();
    assert!(message.contains("no signed release manifest listing an agent"));
    assert!(
        job["guidance"]
            .as_str()
            .unwrap()
            .contains("publish a signed release")
    );
    let stages: Vec<_> = job["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert!(stages.contains(&"preflight") && !stages.contains(&"complete"));
    let stored = job.to_string();
    let bytes = database_bytes(&f);
    let log = logs.0.lock().unwrap().clone();
    for leaked in [
        fake_token,
        fake_password,
        "leak",
        "Previous installation restored",
    ] {
        assert!(!stored.contains(leaked));
        assert!(!contains(&bytes, leaked));
        assert!(!contains(&log, leaked));
    }
}

#[tokio::test]
async fn quiet_operation_longer_than_the_old_limit_succeeds_but_prolonged_silence_fails() {
    let f = fixture().await;
    ssh::set_limits(
        &f.state,
        ssh::Limits {
            inactivity: Duration::from_secs(2),
            total: Duration::from_secs(20),
            report_wait: Duration::from_secs(2),
            ..Default::default()
        },
    );
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    // Output every second for five seconds: each byte resets inactivity, so total > inactivity.
    let mut steps = vec![];
    for _ in 0..5 {
        steps.push(Step::Sleep(Duration::from_secs(1)));
        steps.push(err("hope-install: phase=stage"));
    }
    steps.extend(happy(&f, &agent, &credential));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "completed", "{job}");

    f.restore_address().await;
    f.script(vec![
        err("hope-install: phase=stage"),
        Step::Sleep(Duration::from_secs(3)),
        Step::Exit(0),
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "uncertain", "{job}");
    assert_eq!(job["error_code"], "INACTIVITY_TIMEOUT");
    assert!(
        job["message"]
            .as_str()
            .unwrap()
            .contains("Remote state is unknown")
    );
    assert!(
        !job["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("rolled back")
    );
}

#[tokio::test]
async fn total_timeout_and_output_flood_leave_remote_state_uncertain() {
    let f = fixture().await;
    ssh::set_limits(
        &f.state,
        ssh::Limits {
            inactivity: Duration::from_secs(10),
            total: Duration::from_secs(2),
            report_wait: Duration::from_secs(1),
            ..Default::default()
        },
    );
    let mut steps = vec![];
    for _ in 0..6 {
        steps.push(Step::Sleep(Duration::from_millis(500)));
        steps.push(err("hope-install: phase=stage"));
    }
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("TOTAL_TIMEOUT"))
    );

    ssh::set_limits(
        &f.state,
        ssh::Limits {
            max_bytes: 4096,
            ..Default::default()
        },
    );
    f.script((0..40).map(|_| Step::Out("y".repeat(512) + "\n")).collect());
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("OUTPUT_LIMIT"))
    );
}

#[tokio::test]
async fn dropped_connection_mid_run_is_uncertain_not_rolled_back() {
    let f = fixture().await;
    f.script(vec![
        err("hope-install: phase=preflight"),
        err("hope-install: phase=place-binary"),
        Step::Sleep(Duration::from_millis(100)),
        Step::DropConnection,
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "uncertain", "{job}");
    assert_eq!(job["error_code"], "CONNECTION_LOST");
    assert_eq!(job["stage"], "place_binary");
    assert!(
        !job["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("restored")
    );
    // A clean channel close without an exit status is equally uncertain.
    f.script(vec![
        err("hope-install: phase=stop"),
        Step::CloseWithoutStatus,
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("CONNECTION_LOST"))
    );
}

#[tokio::test]
async fn restart_marks_running_jobs_interrupted_and_history_survives() {
    let f = fixture().await;
    f.script(vec![
        err("hope-install: phase=stage"),
        Step::Sleep(Duration::from_secs(30)),
    ]);
    let (status, job) = f
        .call("POST", "/api/v1/ssh/install", f.password_request())
        .await;
    assert_eq!(status, StatusCode::OK);
    let id = job["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, running) = f
        .call("GET", &format!("/api/v1/ssh/jobs/{id}"), json!({}))
        .await;
    assert_eq!(running["status"], "running");
    // "Restart": open the same database in a new State, as the server process would.
    let reopened = State::open(
        f.db.to_str().unwrap(),
        "https://hope.example".into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .expect("reopen");
    let stored = ssh::load_job(&reopened, &id)
        .await
        .unwrap()
        .expect("job survives restart");
    assert_eq!(stored.status, InstallStatus::Interrupted);
    assert_eq!(stored.error_code.as_deref(), Some("SERVER_RESTARTED"));
    assert!(stored.finished_at.is_some());
    assert!(stored.guidance.is_some());
    assert_eq!(stored.stage, Some(InstallStage::Stage));
}

#[tokio::test]
async fn reinstall_keeps_credential_and_succeeds_without_consuming_the_new_token() {
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    let f = fixture_with(Some((&agent, &credential)), None).await;
    let mut steps = phases(&[
        "preflight",
        "stage",
        "verify",
        "stop",
        "place-binary",
        "configure",
        "write-unit",
        "activate",
    ]);
    steps.push(report_only(&f.state, &agent, &credential));
    steps.extend(phases(&["wait-report", "complete"]));
    steps.push(Step::Exit(0));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["mode"], "reinstall");
    assert_eq!(job["agent_id"], agent.as_str());
    let used: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM enrollment_tokens WHERE used_at IS NOT NULL")
            .fetch_one(&f.state.0.db)
            .await
            .unwrap();
    assert_eq!(used, 0, "the job's enrollment token was not consumed");
}

#[tokio::test]
async fn reinstall_does_not_trust_a_report_sent_before_activation() {
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    let f = fixture_with(Some((&agent, &credential)), None).await;
    // The old service reports while the installer is still preparing; nothing follows activation.
    let mut steps = phases(&["preflight", "stage"]);
    steps.push(report_only(&f.state, &agent, &credential));
    steps.extend(phases(&["activate", "wait-report", "complete"]));
    steps.push(Step::Exit(0));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(job["error_code"], "FIRST_REPORT_TIMEOUT");
}

#[tokio::test]
async fn unavailable_artifacts_and_first_report_timeout_are_reported_distinctly() {
    let f = fixture().await;
    f.script(vec![
        err("hope-install: phase=preflight"),
        err("hope-install: error=RELEASE_UNAVAILABLE No signed agent for linux/x86_64"),
        Step::Exit(1),
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("failed"), Some("RELEASE_UNAVAILABLE"))
    );

    // Installer claims success but no accepted report ever arrives.
    let mut steps = phases(&[
        "preflight",
        "stage",
        "verify",
        "stop",
        "place-binary",
        "configure",
        "write-unit",
        "activate",
        "wait-report",
        "complete",
    ]);
    steps.push(Step::Exit(0));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert_eq!(job["error_code"], "FIRST_REPORT_TIMEOUT");
    assert!(
        job["guidance"]
            .as_str()
            .unwrap()
            .contains("reach the server")
    );

    // Exit 0 without the `complete` phase is not success either.
    f.script(vec![err("hope-install: phase=stage"), Step::Exit(0)]);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("INSTALLER_INCOMPLETE"))
    );
}

fn other_inventory() -> Inventory {
    inventory_with("02:00:00:00:00:99", &["192.168.9.9"])
}

#[tokio::test]
async fn concurrency_is_bounded_and_history_is_listed_newest_first() {
    let f = fixture().await;
    {
        let mut running = f.state.0.installs.lock().await;
        for n in 0..4 {
            running.insert(format!("other-{n}"));
        }
    }
    let (status, body) = f
        .call("POST", "/api/v1/ssh/install", f.password_request())
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("four"));
    f.state.0.installs.lock().await.clear();
    f.script(vec![Step::Exit(1)]);
    let first = f.run(f.password_request()).await;
    let second = f.run(f.password_request()).await;
    let (_, jobs) = f.call("GET", "/api/v1/ssh/jobs", json!({})).await;
    let listed: Vec<_> = jobs
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed,
        vec![
            second["id"].as_str().unwrap(),
            first["id"].as_str().unwrap()
        ]
    );
    let (status, _) = f.call("GET", "/api/v1/ssh/jobs/missing", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_one_job_may_run_per_host_or_ssh_address() {
    let f = fixture().await;
    f.script(vec![Step::Sleep(Duration::from_secs(30))]);
    let (status, _) = f
        .call("POST", "/api/v1/ssh/install", f.password_request())
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = f
        .call("POST", "/api/v1/ssh/install", f.password_request())
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("already running"));
    // A different host record sharing the same address and port is the same SSH endpoint.
    let mut twin = f.state.host(&f.host_id).await.unwrap().unwrap();
    twin.id = Uuid::new_v4().to_string();
    twin.macs = vec![];
    f.state.save_host(&twin).await.unwrap();
    let mut body = f.password_request();
    body["host_id"] = json!(twin.id);
    let (status, _) = f.call("POST", "/api/v1/ssh/install", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_panicking_job_is_marked_uncertain_and_frees_its_slot() {
    let f = fixture().await;
    ssh::set_limits(
        &f.state,
        ssh::Limits {
            panic_for_test: true,
            ..Default::default()
        },
    );
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("INTERNAL_ERROR")),
        "{job}"
    );
    assert!(job["finished_at"].is_number());
    for _ in 0..40 {
        if f.state.0.installs.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(f.state.0.installs.lock().await.is_empty());
    // The host can be used again.
    ssh::set_limits(&f.state, ssh::Limits::default());
    f.script(vec![Step::Exit(1)]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["error_code"], "INSTALLER_FAILED");
}

#[tokio::test]
async fn completion_without_a_stop_or_replacement_phase_is_not_confirmed_by_a_stale_report() {
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    let f = fixture_with(Some((&agent, &credential)), None).await;
    let mut steps = phases(&["preflight", "stage"]);
    steps.push(report_only(&f.state, &agent, &credential));
    steps.extend(phases(&["complete"]));
    steps.push(Step::Exit(0));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("INSTALLER_INCOMPLETE")),
        "{job}"
    );
}

#[tokio::test]
async fn enrollment_that_cannot_be_matched_to_the_selected_host_is_uncertain() {
    let f = fixture().await;
    let (agent, credential) = (Uuid::new_v4().to_string(), secret());
    let mut steps = phases(&["preflight", "stop", "place-binary", "activate"]);
    steps.push(enroll_with(&f.state, &agent, &credential, other_inventory));
    steps.extend(phases(&["complete"]));
    steps.push(Step::Exit(0));
    f.script(steps);
    let job = f.run(f.password_request()).await;
    assert_eq!(
        (job["status"].as_str(), job["error_code"].as_str()),
        (Some("uncertain"), Some("HOST_ASSOCIATION_UNCONFIRMED")),
        "{job}"
    );
    assert_eq!(job["agent_id"], agent.as_str());
}

#[tokio::test]
async fn long_and_non_utf8_message_tails_still_yield_the_error_code() {
    let f = fixture().await;
    let mut tail = b"hope-install: error=DOWNLOAD_FAILED ".to_vec();
    tail.extend([0xff, 0xfe]);
    tail.extend(vec![b'q'; 4000]);
    tail.push(b'\n');
    f.script(vec![
        Step::Out(String::from_utf8_lossy(&tail).into_owned()),
        Step::Exit(1),
    ]);
    let job = f.run(f.password_request()).await;
    assert_eq!(job["error_code"], "DOWNLOAD_FAILED", "{job}");
}

#[tokio::test]
async fn shutdown_settles_running_job_as_interrupted_within_the_deadline() {
    let f = fixture().await;
    f.script(vec![
        err("hope-install: phase=stage"),
        Step::Sleep(Duration::from_secs(60)),
    ]);
    let (status, job) = f
        .call("POST", "/api/v1/ssh/install", f.password_request())
        .await;
    assert_eq!(status, StatusCode::OK);
    let id = job["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, running) = f
        .call("GET", &format!("/api/v1/ssh/jobs/{id}"), json!({}))
        .await;
    assert_eq!(running["status"], "running");

    let started = std::time::Instant::now();
    hope_server::lifecycle::settle(&f.state, Duration::from_secs(6)).await;
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "{:?}",
        started.elapsed()
    );

    // The pool is closed; read the persisted state through a fresh connection.
    let reopened = State::open(
        f.db.to_str().unwrap(),
        "https://hope.example".into(),
        "releases".into(),
        capabilities(),
    )
    .await
    .expect("reopen");
    let stored = ssh::load_job(&reopened, &id).await.unwrap().expect("job");
    assert_eq!(stored.status, InstallStatus::Interrupted);
    assert_eq!(stored.error_code.as_deref(), Some("SERVER_RESTARTED"));
    assert_eq!(stored.stage, Some(InstallStage::Stage));
    assert!(stored.finished_at.is_some());
    // New work is refused once shutdown has begun.
    assert!(f.state.0.shutdown.is_cancelled());
}
