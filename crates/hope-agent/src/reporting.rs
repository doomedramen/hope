use crate::{
    collect,
    config::{self, Config},
    delivery::{self, Outcome, Spool},
    now,
};
use anyhow::{Context, Result};
use hope_protocol::{AgentReport, CollectorStatus, Inventory, Metrics, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path, pin::Pin, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until},
};
use uuid::Uuid;

pub type Sample = (Inventory, Metrics, Vec<CollectorStatus>);
pub type Sampling<'a> = Pin<Box<dyn Future<Output = Sample> + 'a>>;
pub trait Sampler {
    fn sample(&mut self) -> Sampling<'_>;
}
impl Sampler for collect::Collector {
    fn sample(&mut self) -> Sampling<'_> {
        Box::pin(self.sample())
    }
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Acceptance {
    pub(crate) agent_id: String,
    pub(crate) report_id: String,
    pub(crate) run_id: String,
    pub(crate) process_id: u32,
    pub(crate) operation: String,
    pub(crate) collected_at: f64,
    pub(crate) accepted_at: f64,
    #[serde(default)]
    pub(crate) activation_id: Option<String>,
}
#[derive(Default)]
struct InventoryDelivery {
    fingerprint: String,
    accepted_at: f64,
}
impl InventoryDelivery {
    fn changed(&self, inventory: &Inventory, collected_at: f64) -> Result<bool> {
        Ok(self.fingerprint
            != serde_json::to_string(&collect::stable_inventory(inventory.clone()))?
            || collected_at - self.accepted_at >= 3600.0)
    }
    fn accept(&mut self, report: &AgentReport) -> Result<()> {
        if report.collected_at >= self.accepted_at
            && let Some(inventory) = &report.inventory
        {
            self.fingerprint =
                serde_json::to_string(&collect::stable_inventory(inventory.clone()))?;
            self.accepted_at = report.collected_at;
        }
        Ok(())
    }
}
type Request = Pin<Box<dyn Future<Output = (AgentReport, Outcome)> + Send>>;
/// Collection is polled independently of delivery. A bounded channel allows only
/// one completed sample to wait, and this actor remains the sole spool writer.
const MIN_INTERVAL_SECONDS: u32 = 5;
const MAX_INTERVAL_SECONDS: u32 = 300;

/// Spread agents that started together by up to a fifteenth of the interval either side.
fn jittered(period: Duration) -> Duration {
    let spread = period.as_millis() as u64 / 15;
    if period < Duration::from_secs(MIN_INTERVAL_SECONDS.into()) || spread == 0 {
        return period;
    }
    period - Duration::from_millis(spread)
        + Duration::from_millis(rand::random::<u64>() % (2 * spread + 1))
}

async fn sample_loop<C: Sampler>(
    collector: &mut C,
    sender: mpsc::Sender<(Sample, f64)>,
    mut period: watch::Receiver<Duration>,
) {
    let mut next_sample = Instant::now();
    loop {
        tokio::select! {
            _ = sleep_until(next_sample) => {}
            changed = period.changed() => {
                if changed.is_err() { return; }
                next_sample = Instant::now() + jittered(*period.borrow_and_update());
                continue;
            }
        }
        let tick = Instant::now();
        let sample = collector.sample().await;
        let collected_at = now();
        if sender.send((sample, collected_at)).await.is_err() {
            return;
        }
        let interval = jittered(*period.borrow_and_update());
        next_sample = tick + interval;
        // Skip an overrun instead of repeatedly collecting against an expired deadline.
        if next_sample <= Instant::now() {
            next_sample = Instant::now() + interval;
        }
    }
}

pub async fn run<C: Sampler, S: Future<Output = ()>>(
    collector: &mut C,
    configuration: &Config,
    mut spool: Spool,
    operation: &str,
    dir: &Path,
    period: Duration,
    shutdown: S,
) -> Result<()> {
    if configuration.disabled {
        return Err(delivery::Revoked.into());
    }
    let client = config::client(configuration)?;
    let credential = configuration
        .credential
        .clone()
        .context("Missing agent credential")?;
    let run_id = Uuid::new_v4().to_string();
    let activation_id = std::env::var("HOPE_ACTIVATION_ID").ok();
    if let Some(id) = &activation_id {
        Uuid::parse_str(id).context("Invalid service activation identifier")?;
    }
    let (sample_tx, mut samples) = mpsc::channel(1);
    let (period_tx, period_rx) = watch::channel(period);
    let sampling = sample_loop(collector, sample_tx, period_rx);
    tokio::pin!(sampling);
    let mut current_reports = std::collections::HashSet::new();
    let mut retry_at = Instant::now();
    let mut inflight: Option<Request> = None;
    let mut inventory_delivery = InventoryDelivery::default();
    let mut first_report: Option<String> = None;
    let mut failures = 0u32;
    let mut update_requests = crate::updates::Requests;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _=&mut shutdown => { config::write_private(&dir.join("spool.json"),&spool.encode()?).await?; return Ok(()); }
            _=&mut sampling => { anyhow::bail!("Collector stopped unexpectedly"); }
            Some(((inventory,metrics,mut collectors),collected_at))=samples.recv() => {
                collectors.push(crate::updates::status(dir));
                let changed=inventory_delivery.changed(&inventory,collected_at)?;
                let mut report=AgentReport { schema_version:SCHEMA_VERSION,report_id:Uuid::new_v4().to_string(),agent_id:configuration.agent_id.clone(),collected_at,inventory:changed.then_some(inventory),metrics,collectors };
                crate::telemetry::bound_report(&mut report)?;
                if first_report.is_none() { first_report=Some(report.report_id.clone()); }
                current_reports.insert(report.report_id.clone());
                spool.push(report,now())?;
                current_reports.retain(|id| spool.iter().any(|report| &report.report_id == id));
                config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
            }
            result=async { inflight.as_mut().expect("guarded request").await }, if inflight.is_some() => {
                inflight=None;
                let (report,outcome)=result;
                match outcome {
                    Outcome::Accepted(response) => {
                        debug_assert!(response.accepted);
                        spool.remove(&report.report_id); failures=0; retry_at=Instant::now();
                        if let Some(version)=&response.update_to { update_requests.handle_now(dir,version).await; }
                        if let Some(seconds)=response.report_interval_seconds { period_tx.send_if_modified(|period| { let next=Duration::from_secs(seconds.clamp(MIN_INTERVAL_SECONDS,MAX_INTERVAL_SECONDS).into()); if *period==next { false } else { *period=next; true } }); }
                        inventory_delivery.accept(&report)?;
                        if current_reports.remove(&report.report_id) {
                            let accepted=now();
                            config::write_private(&dir.join("last-report"),accepted.to_string().as_bytes()).await?;
                            config::write_private(&dir.join("last-acceptance.json"),&serde_json::to_vec(&Acceptance { agent_id:configuration.agent_id.clone(),report_id:report.report_id.clone(),run_id:run_id.clone(),process_id:std::process::id(),operation:operation.into(),activation_id:activation_id.clone(),collected_at:report.collected_at,accepted_at:accepted })?).await?;
                        }
                        config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
                        if operation!="run" && first_report.as_ref()==Some(&report.report_id) { println!("Agent report accepted"); return Ok(()); }
                    }
                    Outcome::Revoked => {
                        let mut disabled = configuration.clone();
                        disabled.disabled = true;
                        config::save(dir, &disabled).await?;
                        // Queued telemetry is no longer deliverable. Do not retain it on disable.
                        config::write_private(&dir.join("spool.json"), b"[]").await?;
                        return Err(delivery::Revoked.into());
                    }
                    Outcome::Invalid(status) => {
                        current_reports.remove(&report.report_id);
                        spool.remove(&report.report_id);
                        config::write_private(&dir.join("spool.json"),&spool.encode()?).await?;
                        if operation!="run" { anyhow::bail!("First report rejected ({status}); no success was confirmed"); }
                        eprintln!("Report rejected ({status}); dropping invalid sample");
                    }
                    Outcome::Retry(after) => {
                        failures=failures.saturating_add(1); retry_at=Instant::now()+after.unwrap_or_else(||delivery::backoff(failures,rand::random()));
                        if operation!="run" { anyhow::bail!("First report was not accepted; check connectivity and retry"); }
                    }
                }
            }
            _=sleep_until(retry_at), if inflight.is_none() && !spool.is_empty() => {
                spool.trim(now());
                if let Some(report)=spool.latest().cloned() {
                    inflight=Some(Box::pin(delivery::send(client.clone(),configuration.server.clone(),credential.clone(),report)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jitter_stays_within_a_fifteenth_of_the_interval() {
        for _ in 0..200 {
            let thirty = jittered(Duration::from_secs(30));
            assert!(thirty >= Duration::from_secs(28) && thirty <= Duration::from_secs(32));
            let ten = jittered(Duration::from_secs(10));
            assert!(ten >= Duration::from_millis(9334) && ten <= Duration::from_millis(10666));
        }
        assert_eq!(
            jittered(Duration::from_millis(50)),
            Duration::from_millis(50)
        );
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct FakeSampler {
        count: usize,
        stop_after_samples: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Sampler for FakeSampler {
        fn sample(&mut self) -> Sampling<'_> {
            self.count += 1;
            if self.count >= 6
                && let Some(sender) = self.stop_after_samples.take()
            {
                let _ = sender.send(());
            }
            Box::pin(async {
                (
                    Inventory {
                        hostname: "fixture".into(),
                        os: "test".into(),
                        architecture: "test".into(),
                        cpu_model: "test".into(),
                        cpu_cores: 1,
                        memory_total_bytes: 1.0,
                        interfaces: vec![],
                        disks: vec![],
                        gpus: vec![],
                        collectors: vec![],
                    },
                    Metrics::default(),
                    vec![],
                )
            })
        }
    }
    #[tokio::test]
    async fn inventory_suppression_starts_only_after_acceptance_and_ignores_older_backlog() {
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        let (inventory, metrics, collectors) = sampler.sample().await;
        let mut delivery = InventoryDelivery::default();
        assert!(delivery.changed(&inventory, 100.0).expect("changed"));
        assert!(
            delivery
                .changed(&inventory, 200.0)
                .expect("still unaccepted")
        );
        let mut report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: Uuid::new_v4().to_string(),
            collected_at: 200.0,
            inventory: Some(inventory.clone()),
            metrics,
            collectors,
        };
        delivery.accept(&report).expect("accept");
        assert!(!delivery.changed(&inventory, 201.0).expect("unchanged"));
        assert!(delivery.changed(&inventory, 3800.0).expect("refresh"));
        let mut changed = inventory.clone();
        changed.hostname = "changed".into();
        assert!(delivery.changed(&changed, 201.0).expect("new inventory"));
        report.collected_at = 100.0;
        report.inventory = Some(changed);
        delivery.accept(&report).expect("old accepted backlog");
        assert!(!delivery.changed(&inventory, 201.0).expect("latest remains"));
    }
    async fn server(
        status: &str,
        body: &str,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let status = status.to_string();
        let body = body.to_string();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let status = status.clone();
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0; 8192];
                    let _ = socket.read(&mut request).await;
                    tokio::time::sleep(delay).await;
                    let _=socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
                });
            }
        });
        (format!("http://{address}"), task)
    }
    fn configuration(server: String) -> Config {
        Config {
            server,
            agent_id: Uuid::new_v4().to_string(),
            credential: Some("fixture-credential".into()),
            pending_credential: None,
            ca_pem: None,
            disabled: false,
        }
    }
    struct SlowSampler {
        inner: FakeSampler,
        delay: Duration,
    }

    #[tokio::test]
    async fn only_explicit_revocation_is_terminal() {
        for (status, body, revoked) in [
            ("403 Forbidden", "access policy denied", false),
            (
                "401 Unauthorized",
                r#"{"code":"authentication_required","error":"Authentication required"}"#,
                false,
            ),
            (
                "401 Unauthorized",
                r#"{"code":"agent_revoked","error":"Agent credential revoked"}"#,
                true,
            ),
        ] {
            let (endpoint, task) = server(status, body, Duration::ZERO).await;
            let config = configuration(endpoint.clone());
            let client = config::client(&config).expect("client");
            let report = AgentReport {
                schema_version: SCHEMA_VERSION,
                report_id: Uuid::new_v4().to_string(),
                agent_id: config.agent_id,
                collected_at: now(),
                inventory: None,
                metrics: Metrics::default(),
                collectors: vec![],
            };
            let (_, outcome) =
                delivery::send(client, endpoint, config.credential.unwrap(), report).await;
            if revoked {
                assert!(matches!(outcome, Outcome::Revoked));
            } else {
                assert!(matches!(outcome, Outcome::Retry(_)));
            }
            task.abort();
        }
    }
    impl Sampler for SlowSampler {
        fn sample(&mut self) -> Sampling<'_> {
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                self.inner.sample().await
            })
        }
    }

    #[tokio::test]
    async fn collection_slower_than_the_interval_cannot_starve_delivery_or_shutdown() {
        let (endpoint, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::from_millis(50),
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let mut sampler = SlowSampler {
            inner: FakeSampler {
                count: 0,
                stop_after_samples: None,
            },
            delay: Duration::from_millis(100),
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            run(
                &mut sampler,
                &configuration(endpoint.clone()),
                Spool::default(),
                "once",
                directory.path(),
                Duration::from_millis(5),
                std::future::pending(),
            ),
        )
        .await
        .expect("slow collection must not starve HTTP")
        .expect("accepted");
        assert!(directory.path().join("last-acceptance.json").exists());

        sampler.delay = Duration::from_secs(60);
        tokio::time::timeout(
            Duration::from_secs(1),
            run(
                &mut sampler,
                &configuration(endpoint),
                Spool::default(),
                "run",
                directory.path(),
                Duration::from_millis(5),
                tokio::time::sleep(Duration::from_millis(20)),
            ),
        )
        .await
        .expect("shutdown must not wait for collection")
        .expect("shutdown");
        task.abort();
    }

    #[tokio::test]
    async fn sampling_continues_during_slow_requests_and_spool_is_saved() {
        let (server, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::from_secs(10),
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: Some(sender),
        };
        let configuration = configuration(server);
        tokio::time::timeout(
            Duration::from_secs(2),
            run(
                &mut sampler,
                &configuration,
                Spool::default(),
                "run",
                directory.path(),
                Duration::from_millis(15),
                async {
                    let _ = receiver.await;
                },
            ),
        )
        .await
        .expect("Sampling must continue before the slow request finishes")
        .expect("run");
        task.abort();
        assert!(
            sampler.count >= 6,
            "Slow network must not postpone sampling"
        );
        let bytes = config::read_bounded(&directory.path().join("spool.json"), delivery::MAX_BYTES)
            .await
            .expect("spool");
        let queue: std::collections::VecDeque<AgentReport> =
            serde_json::from_slice(&bytes).expect("queue");
        assert!(queue.len() >= 3);
        assert!(
            !tokio::fs::try_exists(directory.path().join("last-acceptance.json"))
                .await
                .expect("marker")
        );
    }
    #[tokio::test]
    async fn rejected_or_false_acceptance_never_confirms_success() {
        for (status, body) in [
            ("400 Bad Request", "{}"),
            ("429 Too Many Requests", "{}"),
            ("200 OK", r#"{"accepted":false,"duplicate":false}"#),
            ("401 Unauthorized", "{}"),
            ("200 OK", &"x".repeat(16385)),
        ] {
            let (server, task) = server(status, body, Duration::ZERO).await;
            let directory = tempfile::tempdir().expect("temporary");
            let mut sampler = FakeSampler {
                count: 0,
                stop_after_samples: None,
            };
            assert!(
                run(
                    &mut sampler,
                    &configuration(server),
                    Spool::default(),
                    "once",
                    directory.path(),
                    Duration::from_secs(30),
                    std::future::pending()
                )
                .await
                .is_err()
            );
            task.abort();
            assert!(
                !tokio::fs::try_exists(directory.path().join("last-acceptance.json"))
                    .await
                    .expect("marker")
            );
        }
    }
    #[tokio::test]
    async fn offline_startup_and_tls_failure_do_not_confirm_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let offline = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let (plain, task) = server("200 OK", "{}", Duration::ZERO).await;
        for endpoint in [offline, plain.replacen("http:", "https:", 1)] {
            let directory = tempfile::tempdir().expect("temporary");
            let mut sampler = FakeSampler {
                count: 0,
                stop_after_samples: None,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                run(
                    &mut sampler,
                    &configuration(endpoint),
                    Spool::default(),
                    "once",
                    directory.path(),
                    Duration::from_secs(30),
                    std::future::pending(),
                ),
            )
            .await
            .expect("bounded transport");
            assert!(result.is_err());
            assert!(!directory.path().join("last-acceptance.json").exists());
            assert!(directory.path().join("spool.json").exists());
        }
        task.abort();
    }
    #[tokio::test]
    async fn only_accepted_current_report_creates_success_marker() {
        let (server, task) = server(
            "200 OK",
            r#"{"accepted":true,"duplicate":false}"#,
            Duration::ZERO,
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary");
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        run(
            &mut sampler,
            &configuration(server),
            Spool::default(),
            "once",
            directory.path(),
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .expect("accepted");
        task.abort();
        let marker: serde_json::Value = serde_json::from_slice(
            &config::read_bounded(&directory.path().join("last-acceptance.json"), 4096)
                .await
                .expect("marker"),
        )
        .expect("json");
        assert_eq!(marker["operation"], "once");
        assert_eq!(marker["process_id"], std::process::id());
    }
    async fn report_once(directory: &std::path::Path, body: &str) {
        let (server, task) = server("200 OK", body, Duration::ZERO).await;
        let mut sampler = FakeSampler {
            count: 0,
            stop_after_samples: None,
        };
        run(
            &mut sampler,
            &configuration(server),
            Spool::default(),
            "once",
            directory,
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .expect("accepted");
        task.abort();
    }
    #[tokio::test]
    async fn an_update_the_server_asks_for_is_left_for_the_updater() {
        let directory = tempfile::tempdir().expect("temporary");
        std::fs::create_dir(directory.path().join("update-queue")).expect("queue");
        report_once(
            directory.path(),
            r#"{"accepted":true,"duplicate":false,"update_to":"0.1.60"}"#,
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(directory.path().join("update-queue/request"))
                .expect("request"),
            "0.1.60\n"
        );
    }
    #[tokio::test]
    async fn no_request_is_left_when_none_is_asked_or_no_updater_exists() {
        let asked = tempfile::tempdir().expect("temporary");
        std::fs::create_dir(asked.path().join("update-queue")).expect("queue");
        report_once(asked.path(), r#"{"accepted":true,"duplicate":false}"#).await;
        assert!(!asked.path().join("update-queue/request").exists());
        let bare = tempfile::tempdir().expect("temporary");
        report_once(
            bare.path(),
            r#"{"accepted":true,"duplicate":false,"update_to":"0.1.60"}"#,
        )
        .await;
        assert!(!bare.path().join("update-queue").exists());
    }
}
