//! Agent gateway: mTLS WebSocket listener (ADR-0007 client-cert auth,
//! ADR-0008 WebSocket transport). Every connection must present a client
//! certificate signed by the internal CA; the certificate's SHA-256
//! fingerprint must map to a known, non-revoked agent.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::agents;
use crate::config::Config;
use crate::pki;
use protocol::{Envelope, Message};

fn build_server_config(config: &Config) -> anyhow::Result<ServerConfig> {
    let cert_pem = std::fs::read_to_string(&config.server_cert_path)?;
    let key_pem = std::fs::read_to_string(&config.server_key_path)?;
    let ca_pem = std::fs::read_to_string(&config.ca_cert_path)?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", config.server_key_path))?;

    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert?)?;
    }

    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;

    let server_config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)?;

    Ok(server_config)
}

pub async fn serve(config: Config, pool: PgPool) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let server_config = build_server_config(&config)?;
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind(&config.gateway_bind_addr).await?;
    tracing::info!(addr = %config.gateway_bind_addr, "agent gateway listening");

    loop {
        let (tcp, peer_addr) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(error = %err, "gateway accept error");
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let pool = pool.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_connection(acceptor, tcp, pool).await {
                tracing::warn!(peer = %peer_addr, error = %err, "gateway connection ended with error");
            }
        });
    }
}

async fn handle_connection(
    acceptor: TlsAcceptor,
    tcp: tokio::net::TcpStream,
    pool: PgPool,
) -> anyhow::Result<()> {
    let tls_stream = acceptor.accept(tcp).await?;

    let peer_certs = tls_stream
        .get_ref()
        .1
        .peer_certificates()
        .map(|certs| certs.to_vec())
        .unwrap_or_default();

    let Some(leaf) = peer_certs.first() else {
        anyhow::bail!("no client certificate presented");
    };

    let fingerprint = pki::fingerprint_der(leaf);

    let agent = agents::find_by_fingerprint(&pool, &fingerprint).await?;
    let Some(agent) = agent else {
        anyhow::bail!("unknown agent certificate: {fingerprint}");
    };
    if agent.revoked_at.is_some() {
        anyhow::bail!("revoked agent certificate: {fingerprint}");
    }

    let ws_stream = tokio_tungstenite::accept_async(tls_stream).await?;
    let (mut write, mut read) = ws_stream.split();

    while let Some(msg) = read.next().await {
        let msg = msg?;
        let WsMessage::Text(text) = msg else {
            if msg.is_close() {
                break;
            }
            continue;
        };

        let envelope: Envelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(error = %err, "malformed envelope from agent");
                continue;
            }
        };

        match envelope.message {
            Message::Hello(hello) => {
                tracing::info!(agent_id = %agent.id, hello_agent_id = %hello.agent_id, "agent hello");
                let ack = Envelope::new(Message::HelloAck(protocol::HelloAck {
                    accepted: true,
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                    heartbeat_interval_secs: 30,
                    reason: None,
                }));
                write
                    .send(WsMessage::Text(serde_json::to_string(&ack)?))
                    .await?;
                agents::touch_last_seen(&pool, agent.id).await?;
            }
            Message::Heartbeat(_) => {
                agents::touch_last_seen(&pool, agent.id).await?;
                let ack = Envelope::new(Message::HeartbeatAck(protocol::HeartbeatAck {
                    server_time_unix_secs: time::OffsetDateTime::now_utc().unix_timestamp(),
                }));
                write
                    .send(WsMessage::Text(serde_json::to_string(&ack)?))
                    .await?;
            }
            other => {
                tracing::debug!(?other, "unhandled message from agent");
            }
        }
    }

    Ok(())
}
