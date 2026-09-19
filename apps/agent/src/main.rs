mod config;
mod enroll;
mod identity;
mod pinning;
mod run;

use clap::{Parser, Subcommand};

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
    /// Exchange a single-use enrollment token for a signed client
    /// certificate (ADR-0007). The CA fingerprint is required (either
    /// `--code TOKEN.FINGERPRINT`, or `--token` + `--ca-fingerprint`
    /// separately) and is verified before the token is ever sent.
    Enroll {
        /// Enrollment HTTPS endpoint, e.g. https://hope.example:8444
        #[arg(long)]
        server: String,
        /// Combined `TOKEN.FINGERPRINT` code, as printed by
        /// `server enroll-token create`.
        #[arg(long, conflicts_with_all = ["token", "ca_fingerprint"])]
        code: Option<String>,
        #[arg(long, required_unless_present = "code")]
        token: Option<String>,
        /// SHA-256 fingerprint (hex) of the server's CA certificate.
        #[arg(long, required_unless_present = "code")]
        ca_fingerprint: Option<String>,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: String,
    },
    /// Connect to the agent gateway using the enrolled identity and run
    /// the Hello/heartbeat loop, reconnecting with backoff on failure.
    Run {
        /// Agent gateway WebSocket endpoint, e.g. wss://hope.example:8443
        #[arg(long)]
        gateway: String,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
            token,
            ca_fingerprint,
            state_dir,
        }) => {
            let code = match code {
                Some(combined) => EnrollCode::parse_combined(&combined)?,
                None => {
                    let token = token.ok_or_else(|| anyhow::anyhow!("--token is required"))?;
                    let ca_fingerprint = ca_fingerprint
                        .ok_or_else(|| anyhow::anyhow!("--ca-fingerprint is required"))?;
                    EnrollCode::new(token, ca_fingerprint)
                }
            };
            enroll::run(&server, &code, &state_dir).await?;
        }
        Some(Command::Run { gateway, state_dir }) => {
            run::run(&gateway, &state_dir).await?;
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
