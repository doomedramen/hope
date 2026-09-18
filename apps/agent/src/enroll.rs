//! Agent-side enrollment: generate a keypair + CSR locally, exchange the
//! operator-supplied single-use token for a signed client cert over the
//! enroll HTTPS endpoint (ADR-0007).
//!
//! TOFU note (TODO for a later slice): at this point the agent has no CA
//! cert yet, so it cannot verify the enroll server's TLS certificate. We
//! trust it on first use for this one request, on the strength of the
//! operator-supplied token; every subsequent connection (the gateway
//! WebSocket) verifies the server strictly against the CA cert returned
//! here.

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use serde::{Deserialize, Serialize};

use crate::identity::{Paths, write_private};

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

pub async fn run(server_url: &str, token: &str, state_dir: &str) -> anyhow::Result<()> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    let hostname = hostname_or_unknown();
    dn.push(DnType::CommonName, hostname.as_str());
    params.distinguished_name = dn;
    let csr = params.serialize_request(&key)?;
    let csr_pem = csr.pem()?;

    let client = reqwest::Client::builder()
        // See module doc: TOFU for this one bootstrap request only.
        .danger_accept_invalid_certs(true)
        .build()?;

    let url = format!("{}/enroll", server_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(&EnrollRequest {
            token: token.to_string(),
            csr_pem,
            hostname: Some(hostname),
        })
        .send()
        .await?;

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
