//! Bounded, read-only monitor checks.
//!
//! A monitor is created only from an approved inventory proposal. This module
//! still treats every target and response as untrusted: it sends one request,
//! follows no redirects, retains no response body, and enforces small limits
//! on paths, headers, and body assertions.

use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

const MAX_PATH_BYTES: usize = 2_048;
const MAX_HOST_BYTES: usize = 255;
const MAX_BODY_ASSERTION_BYTES: usize = 4_096;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_STORED_HEADER_VALUE_BYTES: usize = 256;
const MAX_ERROR_BYTES: usize = 512;
const USER_AGENT: &str = "hope-monitor/0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckProtocol {
    Tcp,
    Http,
    Https,
}

impl CheckProtocol {
    pub fn parse(value: &str) -> Result<Self, CheckError> {
        match value {
            "tcp" => Ok(Self::Tcp),
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            other => Err(CheckError::InvalidRequest(format!(
                "unsupported monitor protocol `{other}`"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub protocol: CheckProtocol,
    pub address: IpAddr,
    pub port: u16,
    pub path: Option<String>,
    pub host: Option<String>,
    pub expected_status: Option<u16>,
    pub body_contains: Option<String>,
    pub timeout: Duration,
}

impl CheckRequest {
    fn validate(&self) -> Result<(), CheckError> {
        if self.port == 0 {
            return Err(CheckError::InvalidRequest(
                "monitor port must be greater than zero".to_string(),
            ));
        }
        if self.timeout.is_zero() || self.timeout > Duration::from_secs(60) {
            return Err(CheckError::InvalidRequest(
                "monitor timeout must be between 1ms and 60s".to_string(),
            ));
        }
        if let Some(path) = self.path.as_deref() {
            if path.len() > MAX_PATH_BYTES {
                return Err(CheckError::InvalidRequest(
                    "monitor HTTP path is too long".to_string(),
                ));
            }
            if !path.starts_with('/') || path.contains(['\r', '\n']) {
                return Err(CheckError::InvalidRequest(
                    "monitor HTTP path must be an absolute path without newlines".to_string(),
                ));
            }
        }
        if let Some(host) = self.host.as_deref()
            && (host.is_empty() || host.len() > MAX_HOST_BYTES || host.contains(['\r', '\n']))
        {
            return Err(CheckError::InvalidRequest(
                "monitor HTTP host is invalid or too long".to_string(),
            ));
        }
        if let Some(status) = self.expected_status
            && !(100..=599).contains(&status)
        {
            return Err(CheckError::InvalidRequest(
                "expected HTTP status must be between 100 and 599".to_string(),
            ));
        }
        if let Some(assertion) = self.body_contains.as_deref() {
            if assertion.is_empty() || assertion.len() > MAX_BODY_ASSERTION_BYTES {
                return Err(CheckError::InvalidRequest(
                    "HTTP body assertion is empty or too long".to_string(),
                ));
            }
            if assertion.contains(['\r', '\n']) {
                return Err(CheckError::InvalidRequest(
                    "HTTP body assertion contains a newline".to_string(),
                ));
            }
        }
        if self.protocol == CheckProtocol::Tcp
            && (self.path.is_some()
                || self.host.is_some()
                || self.expected_status.is_some()
                || self.body_contains.is_some())
        {
            return Err(CheckError::InvalidRequest(
                "HTTP options are not valid for a TCP monitor".to_string(),
            ));
        }
        Ok(())
    }

    fn path(&self) -> &str {
        self.path.as_deref().unwrap_or("/")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Success,
    Failure,
    Timeout,
    Error,
}

impl CheckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub status: CheckStatus,
    pub latency_ms: i32,
    pub error: Option<String>,
    pub details: Value,
}

pub async fn run(request: CheckRequest) -> CheckOutcome {
    let started = Instant::now();
    if let Err(error) = request.validate() {
        return outcome(
            CheckStatus::Error,
            started,
            Some(error.to_string()),
            json!({"protocol": request.protocol.as_str()}),
        );
    }

    let result = timeout(request.timeout, execute(&request)).await;
    match result {
        Err(_) => outcome(
            CheckStatus::Timeout,
            started,
            Some("monitor check timed out".to_string()),
            base_details(&request),
        ),
        Ok(Ok(details)) => outcome(CheckStatus::Success, started, None, details),
        Ok(Err(CheckError::Assertion(message))) => outcome(
            CheckStatus::Failure,
            started,
            Some(message),
            base_details(&request),
        ),
        Ok(Err(error)) => outcome(
            CheckStatus::Failure,
            started,
            Some(error.to_string()),
            base_details(&request),
        ),
    }
}

async fn execute(request: &CheckRequest) -> Result<Value, CheckError> {
    match request.protocol {
        CheckProtocol::Tcp => {
            TcpStream::connect(SocketAddr::new(request.address, request.port)).await?;
            Ok(base_details(request))
        }
        CheckProtocol::Http => {
            let stream = TcpStream::connect(SocketAddr::new(request.address, request.port)).await?;
            execute_http(request, stream).await
        }
        CheckProtocol::Https => {
            let stream = TcpStream::connect(SocketAddr::new(request.address, request.port)).await?;
            let tls = tls_connector()?;
            let server_name = ServerName::IpAddress(request.address.into());
            let stream = tls
                .connect(server_name, stream)
                .await
                .map_err(|error| CheckError::Tls(error.to_string()))?;
            execute_http(request, stream).await
        }
    }
}

async fn execute_http<S>(request: &CheckRequest, mut stream: S) -> Result<Value, CheckError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let host = request
        .host
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| match request.address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        });
    let request_bytes = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        request.path()
    );
    stream.write_all(request_bytes.as_bytes()).await?;
    let response = read_response(&mut stream).await?;
    let mut details = base_details(request);
    details["status_code"] = json!(response.status);
    details["headers"] = response.headers;
    details["body_truncated"] = json!(response.body_truncated);
    details["body_bytes"] = json!(response.body.len());

    if let Some(expected) = request.expected_status
        && response.status != expected
    {
        return Err(CheckError::Assertion(format!(
            "expected HTTP status {expected}, received {}",
            response.status
        )));
    }
    if let Some(assertion) = request.body_contains.as_deref()
        && !String::from_utf8_lossy(&response.body).contains(assertion)
    {
        return Err(CheckError::Assertion(
            "HTTP body assertion did not match".to_string(),
        ));
    }
    if !(200..=399).contains(&response.status) {
        return Err(CheckError::Assertion(format!(
            "HTTP status {} is not successful",
            response.status
        )));
    }
    Ok(details)
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Value,
    body: Vec<u8>,
    body_truncated: bool,
}

async fn read_response<S>(stream: &mut S) -> Result<HttpResponse, CheckError>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(2_048);
    let mut chunk = [0u8; 1_024];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(CheckError::Protocol(
                "HTTP response ended before headers".to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_header_end(&bytes) {
            break end;
        }
        if bytes.len() > MAX_HEADER_BYTES {
            return Err(CheckError::Protocol(
                "HTTP headers exceed bound".to_string(),
            ));
        }
    };

    let (header_bytes, initial_body) = bytes.split_at(header_end);
    let (status, headers) = parse_headers(header_bytes)?;
    let mut body = initial_body.to_vec();
    let mut truncated = body.len() > MAX_BODY_BYTES;
    body.truncate(MAX_BODY_BYTES);
    while body.len() < MAX_BODY_BYTES && !truncated {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = MAX_BODY_BYTES - body.len();
        body.extend_from_slice(&chunk[..read.min(remaining)]);
        if read > remaining {
            truncated = true;
        }
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
        body_truncated: truncated,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn parse_headers(bytes: &[u8]) -> Result<(u16, Value), CheckError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CheckError::Protocol("HTTP headers are not UTF-8".to_string()))?;
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| CheckError::Protocol("HTTP response has no status line".to_string()))?;
    let mut status_parts = status_line.split_whitespace();
    let version = status_parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err(CheckError::Protocol("invalid HTTP status line".to_string()));
    }
    let status = status_parts
        .next()
        .ok_or_else(|| CheckError::Protocol("HTTP response has no status code".to_string()))?
        .parse::<u16>()
        .map_err(|_| CheckError::Protocol("HTTP status code is invalid".to_string()))?;

    let mut headers = serde_json::Map::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(CheckError::Protocol("HTTP header is malformed".to_string()));
        };
        let name = name.trim().to_ascii_lowercase();
        if !matches!(name.as_str(), "content-type" | "content-length" | "server") {
            continue;
        }
        let value = bounded_text(value.trim(), MAX_STORED_HEADER_VALUE_BYTES);
        headers.insert(name, Value::String(value));
    }
    Ok((status, Value::Object(headers)))
}

fn base_details(request: &CheckRequest) -> Value {
    json!({
        "protocol": request.protocol.as_str(),
        "address": request.address.to_string(),
        "port": request.port,
    })
}

fn outcome(
    status: CheckStatus,
    started: Instant,
    error: Option<String>,
    details: Value,
) -> CheckOutcome {
    CheckOutcome {
        status,
        latency_ms: started.elapsed().as_millis().min(i32::MAX as u128) as i32,
        error: error.map(|error| bounded_text(&error, MAX_ERROR_BYTES)),
        details,
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn tls_connector() -> Result<TlsConnector, CheckError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ObservationVerifier { provider }))
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

struct ObservationVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl fmt::Debug for ObservationVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ObservationVerifier").finish()
    }
}

impl ServerCertVerifier for ObservationVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        // Health checks measure reachability. Certificate trust and expiry are
        // separate monitor types; this deliberately does not make an
        // untrusted certificate appear trusted in the result details.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug, Error)]
pub enum CheckError {
    #[error("invalid monitor request: {0}")]
    InvalidRequest(String),
    #[error("monitor assertion failed: {0}")]
    Assertion(String),
    #[error("monitor protocol error: {0}")]
    Protocol(String),
    #[error("monitor TLS error: {0}")]
    Tls(String),
    #[error("monitor I/O error: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn request(protocol: CheckProtocol, port: u16) -> CheckRequest {
        CheckRequest {
            protocol,
            address: IpAddr::from([127, 0, 0, 1]),
            port,
            path: Some("/health".to_string()),
            host: None,
            expected_status: Some(200),
            body_contains: Some("ok".to_string()),
            timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn tcp_connect_is_successful() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
        });

        let outcome = run(CheckRequest {
            protocol: CheckProtocol::Tcp,
            address: IpAddr::from([127, 0, 0, 1]),
            port,
            path: None,
            host: None,
            expected_status: None,
            body_contains: None,
            timeout: Duration::from_secs(1),
        })
        .await;

        assert_eq!(outcome.status, CheckStatus::Success);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn http_status_and_body_assertions_pass() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 512];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nok\r\n")
                .await
                .unwrap();
        });

        let outcome = run(request(CheckProtocol::Http, port)).await;

        assert_eq!(outcome.status, CheckStatus::Success);
        assert_eq!(outcome.details["status_code"], 200);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn http_status_failure_is_a_failure_not_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 512];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 3\r\n\r\nbad")
                .await
                .unwrap();
        });

        let outcome = run(request(CheckProtocol::Http, port)).await;

        assert_eq!(outcome.status, CheckStatus::Failure);
        assert!(outcome.error.unwrap().contains("503"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn check_timeout_is_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let mut request = request(CheckProtocol::Http, port);
        request.timeout = Duration::from_millis(10);
        let outcome = run(request).await;

        assert_eq!(outcome.status, CheckStatus::Timeout);
        task.await.unwrap();
    }

    #[test]
    fn invalid_http_options_fail_closed() {
        let request = CheckRequest {
            protocol: CheckProtocol::Tcp,
            address: IpAddr::from([127, 0, 0, 1]),
            port: 80,
            path: Some("/".to_string()),
            host: None,
            expected_status: None,
            body_contains: None,
            timeout: Duration::from_secs(1),
        };
        let error = request
            .validate()
            .expect_err("TCP must reject HTTP options");
        assert!(error.to_string().contains("HTTP options"));
    }
}
