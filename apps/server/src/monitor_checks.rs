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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::process::Command;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use x509_parser::parse_x509_certificate;

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
    Icmp,
    Tcp,
    Http,
    Https,
    Dns,
    Tls,
}

impl CheckProtocol {
    pub fn parse(value: &str) -> Result<Self, CheckError> {
        match value {
            "icmp" => Ok(Self::Icmp),
            "tcp" => Ok(Self::Tcp),
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            "dns" => Ok(Self::Dns),
            "tls" => Ok(Self::Tls),
            other => Err(CheckError::InvalidRequest(format!(
                "unsupported monitor protocol `{other}`"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Icmp => "icmp",
            Self::Tcp => "tcp",
            Self::Http => "http",
            Self::Https => "https",
            Self::Dns => "dns",
            Self::Tls => "tls",
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
    pub dns_name: Option<String>,
    pub dns_record_type: Option<String>,
    pub tls_min_valid_days: Option<i64>,
    pub timeout: Duration,
}

impl CheckRequest {
    fn validate(&self) -> Result<(), CheckError> {
        if self.port == 0 && !matches!(self.protocol, CheckProtocol::Icmp) {
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
        if let Some(name) = self.dns_name.as_deref() {
            validate_dns_name(name)?;
        }
        if let Some(record_type) = self.dns_record_type.as_deref() {
            dns_record_type(record_type)?;
        }
        if let Some(days) = self.tls_min_valid_days
            && days < 0
        {
            return Err(CheckError::InvalidRequest(
                "TLS minimum validity must not be negative".to_string(),
            ));
        }

        let has_http_options = self.path.is_some()
            || self.host.is_some()
            || self.expected_status.is_some()
            || self.body_contains.is_some();
        match self.protocol {
            CheckProtocol::Icmp => {
                if has_http_options
                    || self.dns_name.is_some()
                    || self.dns_record_type.is_some()
                    || self.tls_min_valid_days.is_some()
                {
                    return Err(CheckError::InvalidRequest(
                        "options are not valid for an ICMP monitor".to_string(),
                    ));
                }
            }
            CheckProtocol::Tcp => {
                if has_http_options
                    || self.dns_name.is_some()
                    || self.dns_record_type.is_some()
                    || self.tls_min_valid_days.is_some()
                {
                    return Err(CheckError::InvalidRequest(
                        "HTTP, DNS, and TLS options are not valid for a TCP monitor".to_string(),
                    ));
                }
            }
            CheckProtocol::Http | CheckProtocol::Https => {
                if self.dns_name.is_some()
                    || self.dns_record_type.is_some()
                    || self.tls_min_valid_days.is_some()
                {
                    return Err(CheckError::InvalidRequest(
                        "DNS and TLS options are not valid for an HTTP monitor".to_string(),
                    ));
                }
            }
            CheckProtocol::Dns => {
                if has_http_options || self.tls_min_valid_days.is_some() {
                    return Err(CheckError::InvalidRequest(
                        "HTTP and TLS options are not valid for a DNS monitor".to_string(),
                    ));
                }
                if self.dns_name.is_none() {
                    return Err(CheckError::InvalidRequest(
                        "DNS monitor requires a DNS name".to_string(),
                    ));
                }
            }
            CheckProtocol::Tls => {
                if self.path.is_some()
                    || self.expected_status.is_some()
                    || self.body_contains.is_some()
                    || self.dns_name.is_some()
                    || self.dns_record_type.is_some()
                {
                    return Err(CheckError::InvalidRequest(
                        "HTTP and DNS options are not valid for a TLS monitor".to_string(),
                    ));
                }
            }
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
        CheckProtocol::Icmp => execute_icmp(request).await,
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
            let server_name = tls_server_name(request)?;
            let stream = tls
                .connect(server_name, stream)
                .await
                .map_err(|error| CheckError::Tls(error.to_string()))?;
            execute_http(request, stream).await
        }
        CheckProtocol::Dns => execute_dns(request).await,
        CheckProtocol::Tls => execute_tls(request).await,
    }
}

async fn execute_icmp(request: &CheckRequest) -> Result<Value, CheckError> {
    let mut command = Command::new("ping");
    command.args(["-n", "-c", "1"]);
    #[cfg(target_os = "linux")]
    command.args(["-W", &request.timeout.as_secs().max(1).to_string()]);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    command.args(["-W", &request.timeout.as_millis().to_string()]);
    command.arg(request.address.to_string());
    let output = command
        .output()
        .await
        .map_err(|error| CheckError::Protocol(format!("ICMP ping unavailable: {error}")))?;
    if !output.status.success() {
        return Err(CheckError::Assertion(
            "ICMP echo request failed".to_string(),
        ));
    }
    let mut details = base_details(request);
    details["command"] = json!("ping");
    Ok(details)
}

async fn execute_dns(request: &CheckRequest) -> Result<Value, CheckError> {
    let name = request
        .dns_name
        .as_deref()
        .ok_or_else(|| CheckError::InvalidRequest("DNS monitor requires a DNS name".to_string()))?;
    let record_type = request.dns_record_type.as_deref().unwrap_or("A");
    let (record_code, record_label) = dns_record_type(record_type)?;
    let id = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        & u32::from(u16::MAX)) as u16;
    let packet = dns_query(id, name, record_code)?;
    let socket = if request.address.is_ipv4() {
        UdpSocket::bind("0.0.0.0:0").await?
    } else {
        UdpSocket::bind("[::]:0").await?
    };
    socket
        .send_to(
            &packet,
            SocketAddr::new(
                request.address,
                if request.port == 0 { 53 } else { request.port },
            ),
        )
        .await?;
    let mut response = [0u8; 2_048];
    let (length, source) = socket.recv_from(&mut response).await?;
    let (response_id, flags, answers) = parse_dns_header(&response[..length])?;
    if response_id != id {
        return Err(CheckError::Protocol(
            "DNS response ID did not match".to_string(),
        ));
    }
    let response_code = flags & 0x000f;
    if response_code != 0 {
        return Err(CheckError::Assertion(format!(
            "DNS response code {response_code}"
        )));
    }
    if flags & 0x8000 == 0 || answers == 0 {
        return Err(CheckError::Assertion(
            "DNS response contains no answers".to_string(),
        ));
    }
    let mut details = base_details(request);
    details["dns_name"] = json!(name);
    details["record_type"] = json!(record_label);
    details["answers"] = json!(answers);
    details["resolver"] = json!(source.ip().to_string());
    Ok(details)
}

async fn execute_tls(request: &CheckRequest) -> Result<Value, CheckError> {
    let stream = TcpStream::connect(SocketAddr::new(request.address, request.port)).await?;
    let tls = tls_connector()?;
    let server_name = tls_server_name(request)?;
    let stream = tls
        .connect(server_name, stream)
        .await
        .map_err(|error| CheckError::Tls(error.to_string()))?;
    let certificate = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| CheckError::Protocol("TLS server sent no certificate".to_string()))?;
    let (_, certificate) = parse_x509_certificate(certificate.as_ref())
        .map_err(|error| CheckError::Protocol(format!("TLS certificate is invalid: {error}")))?;
    let not_after = certificate.validity().not_after.timestamp();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days_remaining = (not_after - now) / 86_400;
    let minimum_days = request.tls_min_valid_days.unwrap_or(0);
    let mut details = base_details(request);
    details["not_after"] = json!(certificate.validity().not_after.to_string());
    details["days_remaining"] = json!(days_remaining);
    details["certificate_trust"] = json!("not_validated");
    if days_remaining < minimum_days {
        return Err(CheckError::Assertion(format!(
            "TLS certificate expires in {days_remaining} days"
        )));
    }
    Ok(details)
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

fn validate_dns_name(name: &str) -> Result<(), CheckError> {
    let trimmed = name.strip_suffix('.').unwrap_or(name);
    if trimmed.is_empty() || trimmed.len() > 253 || trimmed.contains(['\r', '\n']) {
        return Err(CheckError::InvalidRequest(
            "DNS name is empty or too long".to_string(),
        ));
    }
    for label in trimmed.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return Err(CheckError::InvalidRequest(
                "DNS name contains an invalid label".to_string(),
            ));
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(CheckError::InvalidRequest(
                "DNS name contains an invalid character".to_string(),
            ));
        }
    }
    Ok(())
}

fn dns_record_type(value: &str) -> Result<(u16, &'static str), CheckError> {
    match value.to_ascii_uppercase().as_str() {
        "A" => Ok((1, "A")),
        "AAAA" => Ok((28, "AAAA")),
        "CNAME" => Ok((5, "CNAME")),
        "MX" => Ok((15, "MX")),
        "NS" => Ok((2, "NS")),
        "SRV" => Ok((33, "SRV")),
        "TXT" => Ok((16, "TXT")),
        other => Err(CheckError::InvalidRequest(format!(
            "unsupported DNS record type `{other}`"
        ))),
    }
}

fn dns_query(id: u16, name: &str, record_type: u16) -> Result<Vec<u8>, CheckError> {
    validate_dns_name(name)?;
    let mut packet = Vec::with_capacity(512);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100u16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    let trimmed = name.strip_suffix('.').unwrap_or(name);
    for label in trimmed.split('.') {
        packet.push(
            u8::try_from(label.len())
                .map_err(|_| CheckError::InvalidRequest("DNS label is too long".to_string()))?,
        );
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&record_type.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    Ok(packet)
}

fn parse_dns_header(bytes: &[u8]) -> Result<(u16, u16, u16), CheckError> {
    if bytes.len() < 12 {
        return Err(CheckError::Protocol(
            "DNS response is too short".to_string(),
        ));
    }
    Ok((
        u16::from_be_bytes([bytes[0], bytes[1]]),
        u16::from_be_bytes([bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
    ))
}

fn tls_server_name(request: &CheckRequest) -> Result<ServerName<'static>, CheckError> {
    let Some(host) = request.host.as_deref() else {
        return Ok(ServerName::IpAddress(request.address.into()));
    };
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(address.into()));
    }
    ServerName::try_from(host.to_string())
        .map_err(|_| CheckError::InvalidRequest("TLS host is invalid".to_string()))
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
            dns_name: None,
            dns_record_type: None,
            tls_min_valid_days: None,
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
            dns_name: None,
            dns_record_type: None,
            tls_min_valid_days: None,
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
            dns_name: None,
            dns_record_type: None,
            tls_min_valid_days: None,
            timeout: Duration::from_secs(1),
        };
        let error = request
            .validate()
            .expect_err("TCP must reject HTTP options");
        assert!(error.to_string().contains("HTTP"));
    }

    #[tokio::test]
    async fn dns_answer_is_successful() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut query = [0u8; 512];
            let (length, peer) = socket.recv_from(&mut query).await.unwrap();
            let id = [query[0], query[1]];
            assert!(length >= 12);
            let mut response = Vec::from(id);
            response.extend_from_slice(&0x8180u16.to_be_bytes());
            response.extend_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            socket.send_to(&response, peer).await.unwrap();
        });

        let outcome = run(CheckRequest {
            protocol: CheckProtocol::Dns,
            address: IpAddr::from([127, 0, 0, 1]),
            port,
            path: None,
            host: None,
            expected_status: None,
            body_contains: None,
            dns_name: Some("example.test".to_string()),
            dns_record_type: Some("A".to_string()),
            tls_min_valid_days: None,
            timeout: Duration::from_secs(1),
        })
        .await;

        assert_eq!(outcome.status, CheckStatus::Success);
        assert_eq!(outcome.details["answers"], 1);
        task.await.unwrap();
    }

    #[test]
    fn icmp_does_not_require_a_port() {
        let request = CheckRequest {
            protocol: CheckProtocol::Icmp,
            address: IpAddr::from([127, 0, 0, 1]),
            port: 0,
            path: None,
            host: None,
            expected_status: None,
            body_contains: None,
            dns_name: None,
            dns_record_type: None,
            tls_min_valid_days: None,
            timeout: Duration::from_secs(1),
        };
        request.validate().expect("ICMP has no port");
    }

    #[test]
    fn invalid_dns_names_fail_closed() {
        let request = CheckRequest {
            protocol: CheckProtocol::Dns,
            address: IpAddr::from([127, 0, 0, 1]),
            port: 53,
            path: None,
            host: None,
            expected_status: None,
            body_contains: None,
            dns_name: Some("bad name".to_string()),
            dns_record_type: None,
            tls_min_valid_days: None,
            timeout: Duration::from_secs(1),
        };
        let error = request
            .validate()
            .expect_err("spaces are invalid in DNS names");
        assert!(error.to_string().contains("DNS name"));
    }
}
