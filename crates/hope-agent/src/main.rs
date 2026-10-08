mod collect;
mod config;
use anyhow::{Context, Result};
use config::Config;
use hope_protocol::{AgentReport, EnrollmentRequest, EnrollmentResponse, SCHEMA_VERSION};
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let operation = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    if operation == "--version" {
        println!("hope-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let dir = config::directory();
    if operation == "status" {
        let report = tokio::fs::read_to_string(dir.join("last-report"))
            .await
            .context("No report has been accepted yet")?;
        println!("Last accepted report: {}", report.trim());
        return Ok(());
    }
    let mut collector = collect::Collector::new();
    if operation == "inspect" {
        let (inventory, metrics, collectors) = collector.sample().await;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"inventory":inventory,"metrics":metrics,"collectors":collectors})
            )?
        );
        return Ok(());
    }
    if operation == "benchmark" {
        let samples: u32 = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10)
            .clamp(2, 120);
        let mut measurements = vec![];
        for _ in 0..samples {
            let start = Instant::now();
            let _ = collector.sample().await;
            measurements.push(start.elapsed().as_secs_f64() * 1000.0);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        println!(
            "{}",
            serde_json::json!({"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"samples":samples,"collection_ms":measurements})
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(operation.as_str(), "enroll" | "run" | "once"),
        "Use run, enroll, once, inspect, status, benchmark, or --version"
    );
    let mut configuration = if tokio::fs::try_exists(dir.join("config.json")).await? {
        config::read(&dir).await?
    } else {
        anyhow::ensure!(operation == "enroll", "Agent is not enrolled");
        let server = std::env::var("HOPE_SERVER")
            .context("HOPE_SERVER is required")?
            .trim_end_matches('/')
            .to_string();
        let ca_pem = match std::env::var("HOPE_CA_CERT") {
            Ok(path) => Some(tokio::fs::read_to_string(path).await?),
            Err(_) => None,
        };
        let config = Config {
            server,
            agent_id: Uuid::new_v4().to_string(),
            credential: None,
            pending_credential: None,
            ca_pem,
        };
        config::save(&dir, &config).await?;
        config
    };
    if let Ok(server) = std::env::var("HOPE_SERVER") {
        anyhow::ensure!(
            server.trim_end_matches('/') == configuration.server,
            "Existing agent belongs to another server; uninstall and explicitly remove its identity before changing servers"
        );
    }
    let client = config::client(&configuration)?;
    if configuration.credential.is_none() {
        anyhow::ensure!(
            operation == "enroll",
            "Enrollment was interrupted. Retry with a fresh enrollment token."
        );
        if configuration.pending_credential.is_none() {
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            configuration.pending_credential =
                Some(bytes.iter().map(|b| format!("{b:02x}")).collect());
            config::save(&dir, &configuration).await?;
        }
        let (inventory, _, _) = collector.sample().await;
        let request = EnrollmentRequest {
            schema_version: SCHEMA_VERSION,
            token: std::env::var("HOPE_ENROLLMENT_TOKEN")
                .context("HOPE_ENROLLMENT_TOKEN is required")?,
            agent_id: configuration.agent_id.clone(),
            credential: configuration
                .pending_credential
                .clone()
                .context("Missing pending credential")?,
            inventory,
        };
        let response = client
            .post(format!("{}/api/v1/agent/enroll", configuration.server))
            .json(&request)
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "Enrollment rejected ({}). Check token expiry and agent identity.",
            response.status()
        );
        let enrolled: EnrollmentResponse = response.json().await?;
        configuration.credential = Some(enrolled.credential);
        configuration.pending_credential = None;
        config::save(&dir, &configuration).await?;
    }
    let credential = configuration
        .credential
        .as_ref()
        .context("Missing agent credential")?;
    let mut queue: VecDeque<AgentReport> = match tokio::fs::read(dir.join("spool.json")).await {
        Ok(bytes) if bytes.len() <= 4 * 1024 * 1024 => {
            serde_json::from_slice(&bytes).unwrap_or_default()
        }
        _ => VecDeque::new(),
    };
    queue.retain(|r| r.agent_id == configuration.agent_id && r.collected_at > now() - 3600.0);
    let mut last_inventory = String::new();
    let mut inventory_sent = 0.0;
    let mut failures = 0u32;
    let mut retry_at = Instant::now();
    loop {
        let (inventory, metrics, collectors) = collector.sample().await;
        let fingerprint = serde_json::to_string(&collect::stable_inventory(inventory.clone()))?;
        let changed = fingerprint != last_inventory || now() - inventory_sent > 3600.0;
        if changed {
            last_inventory = fingerprint;
            inventory_sent = now();
        }
        let report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: Uuid::new_v4().to_string(),
            agent_id: configuration.agent_id.clone(),
            collected_at: now(),
            inventory: changed.then_some(inventory),
            metrics,
            collectors,
        };
        queue.push_back(report);
        while queue.len() > 120 || serde_json::to_vec(&queue)?.len() > 4 * 1024 * 1024 {
            queue.pop_front();
        }
        if Instant::now() >= retry_at {
            // Newest first: current health does not wait behind a backlog. The server handles out-of-order history.
            let mut sent = 0;
            while let Some(report) = queue.back() {
                match client
                    .post(format!("{}/api/v1/agent/report", configuration.server))
                    .bearer_auth(credential)
                    .json(report)
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        queue.pop_back();
                        failures = 0;
                        sent += 1;
                        config::write_private(
                            &dir.join("last-report"),
                            now().to_string().as_bytes(),
                        )
                        .await?;
                    }
                    Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        anyhow::bail!(
                            "Agent credential was revoked or rejected; contact the administrator"
                        )
                    }
                    Ok(response) if response.status().is_client_error() => {
                        eprintln!(
                            "Report rejected ({}); dropping invalid sample",
                            response.status()
                        );
                        queue.pop_back();
                        last_inventory.clear();
                    }
                    _ => {
                        failures = failures.saturating_add(1);
                        retry_at = Instant::now()
                            + Duration::from_secs(
                                (30u64.saturating_mul(2u64.saturating_pow(failures.min(5))))
                                    .min(900),
                            );
                        last_inventory.clear();
                        break;
                    }
                }
                if sent >= 10 {
                    break;
                }
            }
        }
        config::write_private(&dir.join("spool.json"), &serde_json::to_vec(&queue)?).await?;
        if operation == "once" || operation == "enroll" {
            anyhow::ensure!(
                failures == 0 && queue.is_empty(),
                "First report was not accepted; check server connectivity and retry"
            );
            println!("Agent enrolled and report accepted");
            return Ok(());
        }
        let delay = Duration::from_secs(28 + rand::random::<u64>() % 5);
        tokio::select! {_=tokio::time::sleep(delay)=>{},_=tokio::signal::ctrl_c()=>break}
    }
    Ok(())
}
