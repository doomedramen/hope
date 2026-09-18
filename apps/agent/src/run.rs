//! Agent runtime: connect to the gateway over mTLS WebSocket, send Hello,
//! then heartbeat on a fixed interval (ADR-0007/0008). No reconnect/backoff
//! logic yet — that's a later slice.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rustls::RootCertStore;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::identity::Paths;
use protocol::{Envelope, Heartbeat, Hello, Message};

pub async fn run(gateway_url: &str, state_dir: &str) -> anyhow::Result<()> {
    let paths = Paths::new(state_dir);
    if !paths.exist() {
        anyhow::bail!(
            "agent is not enrolled (run `agent enroll` first): missing files under {state_dir}"
        );
    }

    let cert_pem = std::fs::read_to_string(&paths.cert)?;
    let key_pem = std::fs::read_to_string(&paths.key)?;
    let ca_pem = std::fs::read_to_string(&paths.ca)?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", paths.key.display()))?;

    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert?)?;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)?;

    let (ws_stream, _response) = tokio_tungstenite::connect_async_tls_with_config(
        gateway_url,
        None,
        false,
        Some(Connector::Rustls(Arc::new(tls_config))),
    )
    .await?;

    tracing::info!(gateway_url, "connected to agent gateway");

    let (mut write, mut read) = ws_stream.split();
    let agent_id = Uuid::new_v4();

    let hello = Envelope::new(Message::Hello(Hello {
        agent_id,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: hostname_or_unknown(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }));
    write
        .send(WsMessage::Text(serde_json::to_string(&hello)?))
        .await?;

    let start = Instant::now();
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    heartbeat_interval.tick().await; // first tick fires immediately

    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                let hb = Envelope::new(Message::Heartbeat(Heartbeat {
                    agent_id,
                    uptime_secs: start.elapsed().as_secs(),
                }));
                write.send(WsMessage::Text(serde_json::to_string(&hb)?)).await?;
            }
            msg = read.next() => {
                let Some(msg) = msg else {
                    tracing::warn!("gateway closed the connection");
                    break;
                };
                let msg = msg?;
                if let WsMessage::Text(text) = msg {
                    match serde_json::from_str::<Envelope>(&text) {
                        Ok(envelope) => tracing::info!(?envelope.message, "received from gateway"),
                        Err(err) => tracing::warn!(error = %err, "malformed envelope from gateway"),
                    }
                } else if msg.is_close() {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn hostname_or_unknown() -> String {
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("hostname").output()
            && out.status.success()
        {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }
    "unknown".to_string()
}
