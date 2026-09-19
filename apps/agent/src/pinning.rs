//! Certificate-fingerprint pinning for the enroll bootstrap TLS connection.
//!
//! Replaces TOFU (`danger_accept_invalid_certs`): the operator supplies the
//! server's CA SHA-256 fingerprint out of band (printed once by
//! `server enroll-token create`), and this verifier accepts the connection
//! only if that fingerprint appears somewhere in the presented certificate
//! chain. Normal PKI chain-of-trust validation is deliberately skipped —
//! the trust anchor here is the pinned fingerprint, not a public CA — but
//! signature verification over the handshake still runs via rustls'
//! standard webpki signature checks.

use std::fmt;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use sha2::{Digest, Sha256};

pub fn sha256_hex(der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(der);
    hex::encode(hasher.finalize())
}

pub struct PinnedFingerprintVerifier {
    fingerprint_hex: String,
    provider: Arc<CryptoProvider>,
}

impl PinnedFingerprintVerifier {
    pub fn new(fingerprint_hex: String, provider: Arc<CryptoProvider>) -> Self {
        Self {
            fingerprint_hex: fingerprint_hex.to_lowercase(),
            provider,
        }
    }
}

impl fmt::Debug for PinnedFingerprintVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedFingerprintVerifier").finish()
    }
}

impl ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let pinned_match = std::iter::once(end_entity)
            .chain(intermediates.iter())
            .any(|cert| sha256_hex(cert.as_ref()) == self.fingerprint_hex);

        if pinned_match {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "server certificate chain does not match the pinned CA fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::UnixTime;

    fn self_signed_der() -> Vec<u8> {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        cert.der().to_vec()
    }

    #[test]
    fn matching_fingerprint_is_accepted() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let der = self_signed_der();
        let fingerprint = sha256_hex(&der);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = PinnedFingerprintVerifier::new(fingerprint, provider);

        let end_entity = CertificateDer::from(der);
        let server_name = ServerName::try_from("localhost").unwrap();
        let result =
            verifier.verify_server_cert(&end_entity, &[], &server_name, &[], UnixTime::now());
        assert!(result.is_ok());
    }

    #[test]
    fn wrong_fingerprint_is_rejected() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let der = self_signed_der();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = PinnedFingerprintVerifier::new(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            provider,
        );

        let end_entity = CertificateDer::from(der);
        let server_name = ServerName::try_from("localhost").unwrap();
        let result =
            verifier.verify_server_cert(&end_entity, &[], &server_name, &[], UnixTime::now());
        assert!(result.is_err(), "wrong fingerprint must be rejected");
    }
}
