//! Agent-side enrollment: generate a keypair + CSR locally, exchange the
//! operator-supplied single-use token for a signed client cert over the
//! enroll HTTPS endpoint (ADR-0007).
//!
//! The enroll TLS connection is verified by pinning the CA's SHA-256
//! fingerprint (printed once by `server enroll-token create` alongside the
//! token) rather than trusting on first use: `PinnedFingerprintVerifier`
//! rejects the handshake before the token is ever sent if the presented
//! chain doesn't include a certificate matching the pinned fingerprint.

use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use serde::{Deserialize, Serialize};

use crate::identity::{Paths, write_private};
use crate::pinning::PinnedFingerprintVerifier;

#[derive(Debug, Serialize)]
struct EnrollRequest {
    token: String,
    csr_pem: String,
    hostname: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnrollResponse {
    #[allow(dead_code)]
    agent_id: String,
    cert_pem: String,
    ca_cert_pem: String,
}

/// A single-use enrollment code, either as separate token + CA fingerprint,
/// or the combined `token.fingerprint` form printed by
/// `server enroll-token create`.
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
        Ok(Self {
            token: token.to_string(),
            ca_fingerprint_hex: fingerprint.to_lowercase(),
        })
    }
}

pub async fn run(server_url: &str, code: &EnrollCode, state_dir: &str) -> anyhow::Result<()> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    let hostname = hostname_or_unknown();
    dn.push(DnType::CommonName, hostname.as_str());
    params.distinguished_name = dn;
    let csr = params.serialize_request(&key)?;
    let csr_pem = csr.pem()?;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinnedFingerprintVerifier::new(code.ca_fingerprint_hex.clone(), provider);

    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();

    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .build()?;

    let url = format!("{}/enroll", server_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(&EnrollRequest {
            token: code.token.clone(),
            csr_pem,
            hostname: Some(hostname),
        })
        .send()
        .await
        .map_err(|err| {
            anyhow::anyhow!(
                "connecting to enroll endpoint failed (this is expected if --ca-fingerprint is wrong): {err}"
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("enrollment failed: {status}: {body}");
    }

    let body: EnrollResponse = response.json().await?;

    let paths = Paths::new(state_dir);
    write_private(&paths.key, &key.serialize_pem())?;
    write_private(&paths.cert, &body.cert_pem)?;
    write_private(&paths.ca, &body.ca_cert_pem)?;

    tracing::info!(state_dir, "enrollment complete");
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
