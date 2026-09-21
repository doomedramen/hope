//! Persistent internal TLS identity for Hope's direct HTTPS listener.

use std::path::Path;

use rand::RngCore;
#[cfg(test)]
use rcgen::CertificateSigningRequestParams;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType, SerialNumber,
};

use crate::config::Config;

pub struct Ca {
    pub cert: rcgen::Certificate,
    #[cfg(test)]
    pub key: KeyPair,
}

fn random_serial() -> SerialNumber {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    SerialNumber::from_slice(&bytes)
}

fn ca_params() -> anyhow::Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "hope internal CA");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.serial_number = Some(random_serial());
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    Ok(params)
}

/// Generate a new CA keypair/cert and a server leaf cert (signed by the
/// CA, used for TLS server-auth on the enroll + gateway listeners), and
/// write all four PEM files to the paths in `config`. Errors if any file
/// already exists, to avoid silently clobbering an existing CA.
pub fn init(config: &Config) -> anyhow::Result<()> {
    for path in [
        &config.ca_cert_path,
        &config.ca_key_path,
        &config.server_cert_path,
        &config.server_key_path,
    ] {
        if Path::new(path).exists() {
            anyhow::bail!("refusing to overwrite existing PKI file: {path}");
        }
    }

    let ca_key = KeyPair::generate()?;
    let ca_cert = ca_params()?.self_signed(&ca_key)?;

    let mut server_params = CertificateParams::new(vec![
        "localhost".to_string(),
        // Docker-backed agent acceptance tests and local Compose deployments
        // reach the host through these conventional bridge aliases. Keeping
        // them on the development certificate makes the mTLS gateway usable
        // from the disposable target without weakening certificate checks.
        "host.docker.internal".to_string(),
        "host.containers.internal".to_string(),
    ])?;
    let mut server_dn = DistinguishedName::new();
    server_dn.push(DnType::CommonName, "hope server");
    server_params.distinguished_name = server_dn;
    server_params.serial_number = Some(random_serial());
    server_params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into()?),
        SanType::DnsName("host.docker.internal".try_into()?),
        SanType::DnsName("host.containers.internal".try_into()?),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
    ];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate()?;
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)?;

    // The server cert file holds leaf + CA (a full chain), not just the
    // leaf: clients pinning the CA fingerprint (agent enroll bootstrap)
    // need the CA cert to actually be present in the TLS handshake's
    // certificate message to find a match.
    let server_chain_pem = format!("{}{}", server_cert.pem(), ca_cert.pem());

    for (path, contents) in [
        (&config.ca_cert_path, ca_cert.pem()),
        (&config.ca_key_path, ca_key.serialize_pem()),
        (&config.server_cert_path, server_chain_pem),
        (&config.server_key_path, server_key.serialize_pem()),
    ] {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }

    Ok(())
}

pub fn load_ca(config: &Config) -> anyhow::Result<Ca> {
    let cert_pem = std::fs::read_to_string(&config.ca_cert_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", config.ca_cert_path))?;
    let key_pem = std::fs::read_to_string(&config.ca_key_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", config.ca_key_path))?;

    let key = KeyPair::from_pem(&key_pem)?;
    let cert_params = CertificateParams::from_ca_cert_pem(&cert_pem)?;
    let cert = cert_params.self_signed(&key)?;

    Ok(Ca {
        cert,
        #[cfg(test)]
        key,
    })
}

/// Sign an agent-submitted CSR (PEM) with the CA, producing a short-lived
/// client certificate (30 day validity; renewal is a TODO for a later
/// slice). Returns `(cert_pem, serial_hex, sha256_fingerprint_hex)`.
#[cfg(test)]
pub fn sign_agent_csr(ca: &Ca, csr_pem: &str) -> anyhow::Result<(String, String, String)> {
    let csr = CertificateSigningRequestParams::from_pem(csr_pem)?;

    let mut params = csr.params;
    params.not_before = time::OffsetDateTime::now_utc();
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(30);
    params.is_ca = IsCa::NoCa;
    params.serial_number = Some(random_serial());
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let cert = params.signed_by(&csr.public_key, &ca.cert, &ca.key)?;

    let cert_pem = cert.pem();
    let serial = cert
        .params()
        .serial_number
        .as_ref()
        .map(|s| hex::encode(s.to_bytes()))
        .unwrap_or_default();
    let fingerprint = fingerprint_der(cert.der());

    Ok((cert_pem, serial, fingerprint))
}

pub fn fingerprint_der(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(der);
    hex::encode(hasher.finalize())
}
