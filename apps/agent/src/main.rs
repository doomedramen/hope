mod collectors;
mod config;
mod enroll;
mod identity;
mod pinning;
mod release_verify;
mod run;

use clap::{Parser, Subcommand};
use std::io::Read;

use crate::config::{Config, DEFAULT_STATE_DIR};
use crate::enroll::EnrollCode;

#[derive(Parser)]
#[command(name = "agent", version, about = "Homelab Operations Platform agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print resolved configuration (server_url) and exit, without
    /// connecting anywhere.
    #[arg(long)]
    print_config: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Register an agent-generated identity key using a single-use token.
    /// The CA fingerprint is required for direct LAN TLS (either
    /// `--code TOKEN.FINGERPRINT`, or `--token` + `--ca-fingerprint`
    /// separately) and is verified before the token is ever sent.
    Enroll {
        /// Hope HTTPS origin, e.g. https://hope.example
        #[arg(long)]
        server: String,
        /// Combined `TOKEN.FINGERPRINT` code, as printed by
        /// `server enroll-token create`.
        #[arg(long, conflicts_with_all = ["token", "ca_fingerprint", "code_stdin"])]
        code: Option<String>,
        /// Read the combined enrollment code from stdin so it does not
        /// appear in the remote process list or shell history.
        #[arg(long, conflicts_with_all = ["code", "token", "ca_fingerprint"])]
        code_stdin: bool,
        #[arg(long, required_unless_present_any = ["code", "code_stdin"])]
        token: Option<String>,
        /// SHA-256 fingerprint (hex) of the server's CA certificate.
        #[arg(long, required_unless_present_any = ["code", "code_stdin"])]
        ca_fingerprint: Option<String>,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: String,
        /// Trust a certificate issued by the system roots (for an HTTPS proxy).
        #[arg(long)]
        system_tls: bool,
    },
    /// Connect to the agent gateway using the enrolled identity and run
    /// the Hello/heartbeat loop, reconnecting with backoff on failure.
    Run {
        /// Agent WebSocket endpoint, e.g. wss://hope.example/agent/v1/connect
        #[arg(long)]
        gateway: String,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: String,
    },
    /// Verify a downloaded release manifest.json entry + binary against
    /// this build's embedded trusted public key (spec §7.7), without
    /// installing anything. Exit code is non-zero on any failure.
    VerifyRelease {
        /// Path to a manifest.json produced by `cargo xtask sign`.
        #[arg(long)]
        manifest: String,
        /// Which artifact in the manifest to check (matched by
        /// platform/arch); defaults to this build's own platform/arch.
        #[arg(long)]
        binary: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Enroll {
            server,
            code,
            code_stdin,
            token,
            ca_fingerprint,
            state_dir,
            system_tls,
        }) => {
            let code = match code {
                Some(combined) => EnrollCode::parse_combined(&combined)?,
                None if code_stdin => {
                    let mut input = Vec::new();
                    std::io::stdin().take(8 * 1024).read_to_end(&mut input)?;
                    let combined = String::from_utf8(input)
                        .map_err(|_| anyhow::anyhow!("stdin enrollment code is not UTF-8"))?;
                    EnrollCode::parse_combined(combined.trim())?
                }
                None => {
                    let token = token.ok_or_else(|| anyhow::anyhow!("--token is required"))?;
                    let ca_fingerprint = ca_fingerprint
                        .ok_or_else(|| anyhow::anyhow!("--ca-fingerprint is required"))?;
                    EnrollCode::new(token, ca_fingerprint)
                }
            };
            enroll::run(&server, &code, &state_dir, system_tls).await?;
        }
        Some(Command::Run { gateway, state_dir }) => {
            run::run(&gateway, &state_dir).await?;
        }
        Some(Command::VerifyRelease {
            manifest: manifest_path,
            binary: binary_path,
        }) => {
            let manifest_bytes = std::fs::read(&manifest_path)?;
            let manifest_signature_path =
                std::path::Path::new(&manifest_path).with_file_name("manifest.json.sig");
            let manifest_signature = std::fs::read_to_string(&manifest_signature_path)?;
            let binary_bytes = std::fs::read(&binary_path)?;
            let (platform, arch) = release_verify::current_platform_arch();

            let artifact = release_verify::verify_release_manifest(
                &manifest_bytes,
                manifest_signature.trim(),
                &binary_bytes,
            )
            .map_err(|error| anyhow::anyhow!("no {platform}/{arch} verified release: {error}"))?;
            println!(
                "OK: {} {}/{} verified",
                artifact.record.version, platform, arch
            );
        }
        None => {
            println!("agent {}", env!("CARGO_PKG_VERSION"));
            println!("protocol version {}", protocol::PROTOCOL_VERSION);

            if cli.print_config {
                let config = Config::load()?;
                println!("server_url = {:?}", config.server_url);
            }
        }
    }

    Ok(())
}
