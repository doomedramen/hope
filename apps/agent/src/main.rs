mod config;

use clap::Parser;

use crate::config::Config;

#[derive(Parser)]
#[command(name = "agent", version, about = "Homelab Operations Platform agent")]
struct Cli {
    /// Print resolved configuration (server_url) and exit, without
    /// connecting anywhere. The mTLS handshake and WebSocket transport
    /// (ADR-0007, ADR-0008) land in a later milestone.
    #[arg(long)]
    print_config: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    println!("agent {}", env!("CARGO_PKG_VERSION"));
    println!("protocol version {}", protocol::PROTOCOL_VERSION);

    if cli.print_config {
        let config = Config::load()?;
        println!("server_url = {:?}", config.server_url);
    }

    Ok(())
}
