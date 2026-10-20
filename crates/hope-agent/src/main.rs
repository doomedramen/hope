mod child_delivery;
mod collect;
mod config;
mod cpu;
mod delivery;
mod extras;
mod memory;
mod network;
mod proxmox;
mod reporting;
mod sequence;
mod status;
mod telemetry;
mod updates;
mod uptime;

/// The agent's version: the build version CI compiles in (`HOPE_BUILD_VERSION`, for example
/// 0.1.57), or the crate version for local builds. `--version` prints it, the installer compares it
/// with the signed manifest, and every request names it in `User-Agent`. An empty value (a workflow
/// that passes no version) means the crate version, as in `scripts/build-agent.sh`.
pub const VERSION: &str = match option_env!("HOPE_BUILD_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};
use anyhow::{Context, Result};
use config::Config;
use hope_protocol::{EnrollmentRequest, EnrollmentResponse, SCHEMA_VERSION};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match execute().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            let operation = std::env::args().nth(1).unwrap_or_else(|| "run".into());
            if operation == "run" && error.downcast_ref::<delivery::Revoked>().is_some() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}
async fn execute() -> Result<()> {
    let operation = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    if operation == "--version" {
        println!("hope-agent {VERSION}");
        return Ok(());
    }
    if operation == "collect-proxmox" {
        return proxmox::collect().await;
    }
    let dir = config::directory();
    if operation == "status" {
        return status::show(&dir, &std::env::args().skip(2).collect::<Vec<_>>()).await;
    }
    if operation == "check-config" {
        let configuration = config::read(&dir).await?;
        config::validate_identity(&configuration)?;
        if let Ok(server) = std::env::var("HOPE_SERVER") {
            anyhow::ensure!(
                server.trim_end_matches('/') == configuration.server,
                "Existing agent belongs to another server"
            );
        }
        if configuration.credential.is_none() {
            anyhow::ensure!(
                std::env::var("HOPE_ENROLLMENT_TOKEN").is_ok_and(|token| !token.is_empty()),
                "Incomplete enrollment requires a token"
            );
        }
        println!("Agent configuration is valid");
        return Ok(());
    }
    if operation == "restore-config" {
        let previous = std::env::args()
            .nth(2)
            .context("Use restore-config PREVIOUS_CONFIG_FILE")?;
        config::restore_settings(&dir, std::path::Path::new(&previous)).await?;
        println!("Previous settings restored; enrollment identity retained");
        return Ok(());
    }
    if matches!(operation.as_str(), "inspect" | "benchmark") {
        let mut collector = collect::Collector::new(extras::Settings::load(&dir).await);
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
        let samples: u32 = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10)
            .clamp(2, 120);
        let mut measurements = vec![];
        let mut collector_measurements = vec![];
        for _ in 0..samples {
            let start = Instant::now();
            let _ = collector.sample().await;
            measurements.push(start.elapsed().as_secs_f64() * 1000.0);
            collector_measurements.push(collector.costs.clone());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        println!(
            "{}",
            serde_json::json!({"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"samples":samples,"collection_ms":measurements,"collector_costs":collector_measurements})
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(operation.as_str(), "enroll" | "configure" | "run" | "once"),
        "Use run, enroll, configure, once, inspect, status, benchmark, collect-proxmox, or --version"
    );
    let _state_lock = config::lock(&dir).await?;
    let mut configuration = if tokio::fs::try_exists(dir.join("config.json")).await? {
        config::read(&dir).await?
    } else {
        anyhow::ensure!(
            matches!(operation.as_str(), "enroll" | "configure"),
            "Agent is not enrolled"
        );
        let server = std::env::var("HOPE_SERVER")
            .context("HOPE_SERVER is required")?
            .trim_end_matches('/')
            .to_string();
        let ca_pem = match std::env::var("HOPE_CA_CERT") {
            Ok(path) => Some(String::from_utf8(
                config::read_bounded(std::path::Path::new(&path), 65536).await?,
            )?),
            Err(_) => None,
        };
        let config = Config {
            server,
            agent_id: Uuid::new_v4().to_string(),
            credential: None,
            pending_credential: None,
            ca_pem,
            disabled: false,
        };
        let _ = config::client(&config)?;
        config::save(&dir, &config).await?;
        config
    };
    if let Ok(server) = std::env::var("HOPE_SERVER") {
        anyhow::ensure!(
            server.trim_end_matches('/') == configuration.server,
            "Existing agent belongs to another server; uninstall and explicitly remove its identity before changing servers"
        );
    }
    let reset_credential = std::env::var("HOPE_RESET_CREDENTIAL").as_deref() == Ok("1");
    if reset_credential {
        anyhow::ensure!(
            matches!(operation.as_str(), "enroll" | "configure"),
            "Credential replacement requires enrollment"
        );
        anyhow::ensure!(
            std::env::var("HOPE_ENROLLMENT_TOKEN").is_ok_and(|token| !token.is_empty()),
            "Credential replacement requires an enrollment token"
        );
    }
    if matches!(operation.as_str(), "enroll" | "configure")
        && let Ok(path) = std::env::var("HOPE_CA_CERT")
    {
        let pem =
            String::from_utf8(config::read_bounded(std::path::Path::new(&path), 65536).await?)?;
        let previous = configuration.ca_pem.replace(pem);
        if let Err(error) = config::client(&configuration) {
            configuration.ca_pem = previous;
            return Err(error);
        }
        config::save(&dir, &configuration).await?;
    }
    let needs_enrollment = configuration.credential.is_none() || reset_credential;
    if operation == "configure" && !needs_enrollment {
        return configured(&configuration);
    }
    if configuration.disabled && !needs_enrollment {
        return Err(delivery::Revoked.into());
    }
    // Monitor clients must see the CA/configuration just persisted above. Also
    // defer collector setup until after ownership and configuration validation.
    let mut collector = collect::Collector::new(extras::Settings::load(&dir).await);
    if needs_enrollment {
        let client = config::client(&configuration)?;
        anyhow::ensure!(
            matches!(operation.as_str(), "enroll" | "configure"),
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
        let enrolled: EnrollmentResponse = delivery::bounded_json(response).await?;
        anyhow::ensure!(
            Some(&enrolled.credential) == configuration.pending_credential.as_ref(),
            "Enrollment returned an unexpected credential"
        );
        configuration.credential = Some(enrolled.credential);
        configuration.pending_credential = None;
        configuration.disabled = false;
        config::save(&dir, &configuration).await?;
    }
    if operation == "configure" {
        return configured(&configuration);
    }
    let bytes = config::read_bounded(&dir.join("spool.json"), delivery::MAX_BYTES)
        .await
        .unwrap_or_default();
    let spool = delivery::Spool::decode(&bytes, &configuration.agent_id, now());
    reporting::run(
        &mut collector,
        &configuration,
        spool,
        &operation,
        &dir,
        Duration::from_secs(30),
        config::shutdown(),
    )
    .await
}

fn configured(configuration: &Config) -> Result<()> {
    if configuration.disabled {
        return Err(delivery::Revoked.into());
    }
    let _ = config::client(configuration)?;
    println!("Agent configuration ready; service reporting has not been confirmed");
    Ok(())
}
