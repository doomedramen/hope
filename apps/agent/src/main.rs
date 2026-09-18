mod config;
mod enroll;
mod identity;
mod run;

use clap::{Parser, Subcommand};

use crate::config::{Config, DEFAULT_STATE_DIR};

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
    /// certificate (ADR-0007).
    Enroll {
        /// Enrollment HTTPS endpoint, e.g. https://hope.example:8444
        #[arg(long)]
        server: String,
        #[arg(long)]
        token: String,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: String,
    },
    /// Connect to the agent gateway using the enrolled identity and run
    /// the Hello/heartbeat loop.
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
            token,
            state_dir,
        }) => {
            enroll::run(&server, &token, &state_dir).await?;
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
