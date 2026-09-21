//! Enroll an agent-generated Ed25519 identity over the web origin.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::{Paths, write_private};
use crate::pinning::PinnedFingerprintVerifier;

#[derive(Debug, Serialize)]
struct EnrollRequest {
    token: String,
    public_key_hex: String,
    hostname: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnrollResponse {
    agent_id: Uuid,
}

pub struct EnrollCode {
    pub token: String,
    pub ca_fingerprint_hex: String,
}

impl EnrollCode {
    pub fn new(token: String, ca_fingerprint_hex: String) -> Self {
        Self {
            token,
            ca_fingerprint_hex: ca_fingerprint_hex.to_lowercase(),
        }
    }

    pub fn parse_combined(code: &str) -> anyhow::Result<Self> {
        let (token, fingerprint) = code
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("--code must be in TOKEN.FINGERPRINT form"))?;
        Ok(Self::new(token.to_string(), fingerprint.to_string()))
    }
}

pub async fn run(
    server_url: &str,
    code: &EnrollCode,
    state_dir: &str,
    system_tls: bool,
) -> anyhow::Result<()> {
    let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
    let client = if system_tls {
        reqwest::Client::new()
    } else {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = PinnedFingerprintVerifier::new(code.ca_fingerprint_hex.clone(), provider);
        let tls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        reqwest::Client::builder()
            .use_preconfigured_tls(tls_config)
            .build()?
    };

    let response = client
        .post(format!(
            "{}/agent/v1/enroll",
            server_url.trim_end_matches('/')
        ))
        .json(&EnrollRequest {
            token: code.token.clone(),
            public_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
            hostname: Some(hostname_or_unknown()),
        })
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("enrollment failed: {}", response.status());
    }
    let enrolled: EnrollResponse = response.json().await?;
    let paths = Paths::new(state_dir);
    write_private(&paths.identity_key, &hex::encode(signing_key.to_bytes()))?;
    write_private(&paths.agent_id, &enrolled.agent_id.to_string())?;
    let trust = if system_tls {
        "system".to_string()
    } else {
        format!("pinned:{}", code.ca_fingerprint_hex)
    };
    write_private(&paths.tls_trust, &trust)?;
    tracing::info!(agent_id = %enrolled.agent_id, "enrollment complete");
    Ok(())
}

fn hostname_or_unknown() -> String {
    #[cfg(unix)]
    {
        if let Ok(name) = std::process::Command::new("hostname").output()
            && name.status.success()
        {
            return String::from_utf8_lossy(&name.stdout).trim().to_string();
        }
    }
    "unknown".to_string()
}
