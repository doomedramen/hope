//! Bounded, read-only protocol classification for open TCP observations.
//!
//! M2 only identifies the transport-level protocol. It does not try product
//! signatures, authenticate, or execute protocol-specific actions. Every
//! probe has a connection budget, per-operation timeout, bounded headers and
//! body, and an overall deadline. HTTP probes send one ordinary GET request
//! without cookies or credentials. TLS inspection accepts a handshake only
//! for observation, never as proof of trust; certificate trust remains a
//! separate operator concern.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

const USER_AGENT: &str = "hope-discovery/0.1";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_OVERALL_TIMEOUT: Duration = Duration::from_secs(4);
const DEFAULT_MAX_HEADER_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_BODY_BYTES: usize = 32 * 1024;
const DEFAULT_MAX_BANNER_BYTES: usize = 512;
const DEFAULT_MAX_REDIRECTS: usize = 3;
const DEFAULT_MAX_CONNECTIONS: usize = 8;
const MAX_HTTP_PATH_BYTES: usize = 2048;
const MAX_STORED_TEXT_BYTES: usize = 4096;
const MAX_BANNER_WAIT: Duration = Duration::from_millis(200);

const HTTP_SAFE_HEADERS: &[&str] = &[
    "alt-svc",
    "content-length",
    "content-type",
    "location",
    "server",
    "via",
    "www-authenticate",
];

/// Protocol classes supported by the M2 classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceProtocol {
    /// An open TCP port whose application protocol was not identified.
    GenericTcp,
    /// Plain HTTP over TCP.
    Http,
    /// HTTP over TLS.
    Https,
    /// TLS without an HTTP response.
    Tls,
    /// SSH, identified from its server banner.
    Ssh,
}

impl ServiceProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GenericTcp => "tcp",
            Self::Http => "http",
            Self::Https => "https",
            Self::Tls => "tls",
            Self::Ssh => "ssh",
        }
    }

    const fn confidence(self) -> f32 {
        match self {
            Self::GenericTcp => 0.45,
            Self::Http | Self::Ssh => 0.98,
            Self::Https | Self::Tls => 0.95,
        }
    }
}

impl fmt::Display for ServiceProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Limits for one open-port classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassificationConfig {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub overall_timeout: Duration,
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub max_banner_bytes: usize,
    pub max_redirects: usize,
    pub max_connections: usize,
}

impl Default for ClassificationConfig {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            overall_timeout: DEFAULT_OVERALL_TIMEOUT,
            max_header_bytes: DEFAULT_MAX_HEADER_BYTES,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_banner_bytes: DEFAULT_MAX_BANNER_BYTES,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

impl ClassificationConfig {
    fn validate(self) -> Result<Self, ClassificationError> {
        if self.connect_timeout.is_zero() {
            return Err(ClassificationError::InvalidConfig(
                "classification connect timeout must be greater than zero",
            ));
        }
        if self.read_timeout.is_zero() {
            return Err(ClassificationError::InvalidConfig(
                "classification read timeout must be greater than zero",
            ));
        }
        if self.overall_timeout.is_zero() {
            return Err(ClassificationError::InvalidConfig(
                "classification overall timeout must be greater than zero",
            ));
        }
        if self.max_header_bytes == 0 || self.max_body_bytes == 0 || self.max_banner_bytes == 0 {
            return Err(ClassificationError::InvalidConfig(
                "classification read bounds must be greater than zero",
            ));
        }
        if self.max_connections == 0 {
            return Err(ClassificationError::InvalidConfig(
                "classification connection bound must be greater than zero",
            ));
        }
        Ok(self)
    }
}

/// One bounded classification result. Evidence excludes cookies,
/// authorization values, raw TLS certificates, and unbounded peer data.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationResult {
    pub protocol: ServiceProtocol,
    pub confidence: f32,
    pub evidence: Value,
}

impl ClassificationResult {
    fn new(protocol: ServiceProtocol, evidence: Value) -> Self {
        Self {
            protocol,
            confidence: protocol.confidence(),
            evidence,
        }
    }
}

/// Errors that prevent classifier setup. A live-port probe turns transport
/// errors into a generic-TCP result so an open endpoint stays visible.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClassificationError {
    #[error("{0}")]
    InvalidConfig(&'static str),
}

/// Safe classifier for one open TCP endpoint.
#[derive(Debug, Clone, Copy)]
pub struct Classifier {
    config: ClassificationConfig,
}

impl Classifier {
    pub fn new(config: ClassificationConfig) -> Result<Self, ClassificationError> {
        Ok(Self {
            config: config.validate()?,
        })
    }

    /// Classify one open port. Any probe failure returns generic TCP evidence
    /// rather than hiding the open port from the inventory.
    pub async fn classify(&self, address: IpAddr, port: u16) -> ClassificationResult {
        let fallback = || {
            ClassificationResult::new(
                ServiceProtocol::GenericTcp,
                json!({
                    "protocol": ServiceProtocol::GenericTcp.as_str(),
                    "transport": "tcp",
                    "address": address.to_string(),
                    "port": port,
                    "probe": "bounded_tcp",
                }),
            )
        };

        let result = timeout(
            self.config.overall_timeout,
            self.classify_inner(address, port),
        )
        .await;
        match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let mut result = fallback();
                if let Some(object) = result.evidence.as_object_mut() {
                    object.insert("probe_error".to_string(), Value::String(error));
                }
                result
            }
            Err(_) => {
                let mut result = fallback();
                if let Some(object) = result.evidence.as_object_mut() {
                    object.insert(
                        "probe_error".to_string(),
                        Value::String("overall_timeout".to_string()),
                    );
                }
                result
            }
        }
    }

    async fn classify_inner(
        &self,
        address: IpAddr,
        port: u16,
    ) -> Result<ClassificationResult, String> {
        let mut budget = ProbeBudget::new(self.config.max_connections);
        let banner = self
            .probe_banner(address, port, &mut budget)
            .await
            .map_err(|error| error.to_string())?;

        if banner.is_ssh() {
            return Ok(ClassificationResult::new(
                ServiceProtocol::Ssh,
                json!({
                    "protocol": ServiceProtocol::Ssh.as_str(),
                    "transport": "tcp",
                    "address": address.to_string(),
                    "port": port,
                    "probe": "ssh_banner",
                    "banner": banner.text,
                    "host_key_collected": false,
                }),
            ));
        }

        let tls_first = is_tls_port_hint(port);

        if tls_first {
            if let Some(mut tls) = self
                .probe_tls(address, port, &mut budget)
                .await
                .map_err(|error| error.to_string())?
            {
                if let Some(response) = self.probe_http_on_tls(&mut tls, address, port).await {
                    return Ok(https_result(address, port, tls, response));
                }
                return Ok(tls_result(address, port, tls));
            }
            if let Some(response) = self
                .probe_plain_http(address, port, &mut budget)
                .await
                .map_err(|error| error.to_string())?
            {
                return Ok(http_result(address, port, response));
            }
        } else {
            if let Some(response) = self
                .probe_plain_http(address, port, &mut budget)
                .await
                .map_err(|error| error.to_string())?
            {
                return Ok(http_result(address, port, response));
            }
            if let Some(mut tls) = self
                .probe_tls(address, port, &mut budget)
                .await
                .map_err(|error| error.to_string())?
            {
                if let Some(response) = self.probe_http_on_tls(&mut tls, address, port).await {
                    return Ok(https_result(address, port, tls, response));
                }
                return Ok(tls_result(address, port, tls));
            }
        }

        let mut evidence = json!({
            "protocol": ServiceProtocol::GenericTcp.as_str(),
            "transport": "tcp",
            "address": address.to_string(),
            "port": port,
            "probe": "bounded_tcp",
        });
        if let Some(text) = banner.text {
            evidence["banner"] = Value::String(text);
        }
        evidence["protocol_evidence"] = Value::String("none".to_string());
        Ok(ClassificationResult::new(
            ServiceProtocol::GenericTcp,
            evidence,
        ))
    }

    async fn probe_banner(
        &self,
        address: IpAddr,
        port: u16,
        budget: &mut ProbeBudget,
    ) -> Result<BannerProbe, ProbeError> {
        let mut stream = self.connect(address, port, budget).await?;
        let mut bytes = vec![0u8; self.config.max_banner_bytes];
        let read_timeout = self.config.read_timeout.min(MAX_BANNER_WAIT);
        let read = timeout(read_timeout, stream.read(&mut bytes)).await;
        let length = match read {
            Ok(Ok(length)) => length,
            Ok(Err(_)) => return Ok(BannerProbe::empty()),
            Err(_) => return Ok(BannerProbe::empty()),
        };
        bytes.truncate(length);
        Ok(BannerProbe::from_bytes(&bytes))
    }

    async fn probe_plain_http(
        &self,
        address: IpAddr,
        port: u16,
        budget: &mut ProbeBudget,
    ) -> Result<Option<HttpResponse>, ProbeError> {
        let mut path = "/".to_string();
        let mut redirects = Vec::new();

        for redirect_index in 0..=self.config.max_redirects {
            let mut stream = match self.connect(address, port, budget).await {
                Ok(stream) => stream,
                Err(ProbeError::ConnectionLimit) => return Ok(None),
                Err(_) => return Ok(None),
            };
            let mut response = match self.http_request(&mut stream, address, port, &path).await {
                Ok(response) => response,
                Err(_) => return Ok(None),
            };
            response.redirects = redirects.clone();

            let Some(location) = response.location.clone() else {
                return Ok(Some(response));
            };
            if !is_redirect(response.status) || redirect_index == self.config.max_redirects {
                return Ok(Some(response));
            }
            let Some(next_path) = same_endpoint_redirect(&location, address, port, "http") else {
                return Ok(Some(response));
            };
            redirects.push(sanitize_text(&location, MAX_STORED_TEXT_BYTES));
            path = next_path;
        }

        Ok(None)
    }

    async fn probe_tls(
        &self,
        address: IpAddr,
        port: u16,
        budget: &mut ProbeBudget,
    ) -> Result<Option<TlsObservation>, ProbeError> {
        let stream = match self.connect(address, port, budget).await {
            Ok(stream) => stream,
            Err(ProbeError::ConnectionLimit) => return Ok(None),
            Err(_) => return Ok(None),
        };

        let _ = rustls::crypto::ring::default_provider().install_default();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = ObservationVerifier {
            provider: Arc::clone(&provider),
        };
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = ServerName::IpAddress(address.into());
        let stream = match timeout(
            self.config.connect_timeout,
            connector.connect(server_name, stream),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) | Err(_) => return Ok(None),
        };

        let (_, connection) = stream.get_ref();
        let certificates = connection
            .peer_certificates()
            .map(|certificates| certificates.to_vec())
            .unwrap_or_default();
        let alpn = connection
            .alpn_protocol()
            .map(|protocol| sanitize_text(&String::from_utf8_lossy(protocol), 64));
        Ok(Some(TlsObservation {
            stream,
            certificates,
            alpn,
        }))
    }

    async fn probe_http_on_tls(
        &self,
        tls: &mut TlsObservation,
        address: IpAddr,
        port: u16,
    ) -> Option<HttpResponse> {
        self.http_request(&mut tls.stream, address, port, "/")
            .await
            .ok()
    }

    async fn connect(
        &self,
        address: IpAddr,
        port: u16,
        budget: &mut ProbeBudget,
    ) -> Result<TcpStream, ProbeError> {
        budget.take()?;
        timeout(
            self.config.connect_timeout,
            TcpStream::connect(SocketAddr::new(address, port)),
        )
        .await
        .map_err(|_| ProbeError::Timeout("connect"))?
        .map_err(ProbeError::Io)
    }

    async fn http_request<S>(
        &self,
        stream: &mut S,
        address: IpAddr,
        port: u16,
        path: &str,
    ) -> Result<HttpResponse, ProbeError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let path = bounded_http_path(path);
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}:{port}\r\nUser-Agent: {USER_AGENT}\r\nAccept: text/html, text/plain;q=0.9, */*;q=0.1\r\nConnection: close\r\n\r\n"
        );
        timeout(
            self.config.read_timeout,
            stream.write_all(request.as_bytes()),
        )
        .await
        .map_err(|_| ProbeError::Timeout("write"))?
        .map_err(ProbeError::Io)?;

        let (header_bytes, initial_body) = self.read_response_head(stream).await?;
        let parsed_headers = parse_response_headers(&header_bytes)?;
        let body = self
            .read_body(stream, initial_body, parsed_headers.content_length)
            .await?;
        let body_sample = sanitize_text(&String::from_utf8_lossy(&body), MAX_STORED_TEXT_BYTES);
        let title = extract_title(&body_sample);

        Ok(HttpResponse {
            status: parsed_headers.status,
            headers: parsed_headers.headers,
            location: parsed_headers.location,
            title,
            body_sample,
            redirects: Vec::new(),
        })
    }

    async fn read_response_head<S>(&self, stream: &mut S) -> Result<(Vec<u8>, Vec<u8>), ProbeError>
    where
        S: AsyncRead + Unpin,
    {
        let mut bytes = Vec::with_capacity(self.config.max_header_bytes.min(4096));
        let mut chunk = [0u8; 1024];
        loop {
            let read = timeout(self.config.read_timeout, stream.read(&mut chunk)).await;
            let length = match read {
                Ok(Ok(length)) => length,
                Ok(Err(error)) => return Err(ProbeError::Io(error)),
                Err(_) => return Err(ProbeError::Timeout("read")),
            };
            if length == 0 {
                return Err(ProbeError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk[..length]);
            if let Some(separator) = find_header_end(&bytes) {
                if separator > self.config.max_header_bytes {
                    return Err(ProbeError::ResponseTooLarge);
                }
                let body_start = separator + 4;
                let mut body = bytes.split_off(body_start);
                body.truncate(self.config.max_body_bytes);
                bytes.truncate(separator);
                return Ok((bytes, body));
            }
            if bytes.len() > self.config.max_header_bytes {
                return Err(ProbeError::ResponseTooLarge);
            }
        }
    }

    async fn read_body<S>(
        &self,
        stream: &mut S,
        mut body: Vec<u8>,
        content_length: Option<usize>,
    ) -> Result<Vec<u8>, ProbeError>
    where
        S: AsyncRead + Unpin,
    {
        let target_length = content_length
            .unwrap_or(self.config.max_body_bytes)
            .min(self.config.max_body_bytes);
        body.truncate(target_length);
        let mut chunk = [0u8; 1024];
        while body.len() < target_length {
            let read = timeout(self.config.read_timeout, stream.read(&mut chunk)).await;
            let length = match read {
                Ok(Ok(length)) => length,
                Ok(Err(error)) => return Err(ProbeError::Io(error)),
                Err(_) => break,
            };
            if length == 0 {
                break;
            }
            let remaining = target_length - body.len();
            body.extend_from_slice(&chunk[..length.min(remaining)]);
        }
        Ok(body)
    }
}

#[derive(Debug)]
struct ProbeBudget {
    remaining: usize,
}

impl ProbeBudget {
    const fn new(max_connections: usize) -> Self {
        Self {
            remaining: max_connections,
        }
    }

    fn take(&mut self) -> Result<(), ProbeError> {
        if self.remaining == 0 {
            return Err(ProbeError::ConnectionLimit);
        }
        self.remaining -= 1;
        Ok(())
    }
}

#[derive(Debug)]
enum ProbeError {
    ConnectionLimit,
    InvalidResponse,
    Io(io::Error),
    ResponseTooLarge,
    Timeout(&'static str),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionLimit => f.write_str("connection_limit"),
            Self::InvalidResponse => f.write_str("invalid_response"),
            Self::Io(error) => write!(f, "io:{}", error.kind()),
            Self::ResponseTooLarge => f.write_str("response_too_large"),
            Self::Timeout(stage) => write!(f, "timeout_{stage}"),
        }
    }
}

#[derive(Debug)]
struct BannerProbe {
    text: Option<String>,
}

impl BannerProbe {
    fn empty() -> Self {
        Self { text: None }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let text = (!bytes.is_empty()).then(|| sanitize_text(&String::from_utf8_lossy(bytes), 512));
        Self { text }
    }

    fn is_ssh(&self) -> bool {
        self.text.as_deref().is_some_and(|text| {
            text.lines()
                .next()
                .is_some_and(|line| line.trim_start().starts_with("SSH-"))
        })
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    location: Option<String>,
    title: Option<String>,
    body_sample: String,
    redirects: Vec<String>,
}

#[derive(Debug)]
struct ParsedResponseHeaders {
    status: u16,
    headers: BTreeMap<String, String>,
    location: Option<String>,
    content_length: Option<usize>,
}

#[derive(Debug)]
struct TlsObservation {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    certificates: Vec<CertificateDer<'static>>,
    alpn: Option<String>,
}

fn http_result(address: IpAddr, port: u16, response: HttpResponse) -> ClassificationResult {
    ClassificationResult::new(
        ServiceProtocol::Http,
        http_evidence(ServiceProtocol::Http, address, port, response, None),
    )
}

fn https_result(
    address: IpAddr,
    port: u16,
    tls: TlsObservation,
    response: HttpResponse,
) -> ClassificationResult {
    ClassificationResult::new(
        ServiceProtocol::Https,
        http_evidence(ServiceProtocol::Https, address, port, response, Some(&tls)),
    )
}

fn tls_result(address: IpAddr, port: u16, tls: TlsObservation) -> ClassificationResult {
    let mut evidence = json!({
        "protocol": ServiceProtocol::Tls.as_str(),
        "transport": "tcp",
        "address": address.to_string(),
        "port": port,
        "probe": "tls_handshake",
        "alpn": tls.alpn,
        "certificate_trust": "not_validated",
        "certificate": certificate_values(&tls.certificates, address),
    });
    if !evidence["certificate"].is_array() {
        evidence["certificate"] = Value::Array(Vec::new());
    }
    ClassificationResult::new(ServiceProtocol::Tls, evidence)
}

fn http_evidence(
    protocol: ServiceProtocol,
    address: IpAddr,
    port: u16,
    response: HttpResponse,
    tls: Option<&TlsObservation>,
) -> Value {
    let mut evidence = json!({
        "protocol": protocol.as_str(),
        "transport": "tcp",
        "address": address.to_string(),
        "port": port,
        "probe": "http_get",
        "status": response.status,
        "headers": response.headers,
        "body_sample": response.body_sample,
        "redirects": response.redirects,
    });
    if let Some(title) = response.title {
        evidence["title"] = Value::String(title);
    }
    if let Some(tls) = tls {
        evidence["tls"] = json!({
            "alpn": tls.alpn,
            "certificate_trust": "not_validated",
            "certificate": certificate_values(&tls.certificates, address),
        });
    }
    evidence
}

fn certificate_values(certificates: &[CertificateDer<'static>], address: IpAddr) -> Value {
    Value::Array(
        certificates
            .iter()
            .take(4)
            .map(|certificate| certificate_value(certificate, address))
            .collect(),
    )
}

fn certificate_value(certificate: &CertificateDer<'static>, address: IpAddr) -> Value {
    let der = certificate.as_ref();
    let mut hasher = Sha256::new();
    hasher.update(der);
    let fingerprint = hex::encode(hasher.finalize());
    let mut value = json!({
        "sha256": fingerprint,
        "certificate_trust": "not_validated",
        "raw_bytes": der.len(),
    });

    let Ok((_, parsed)) = parse_x509_certificate(der) else {
        value["parse_error"] = Value::Bool(true);
        return value;
    };
    let subject = parsed
        .subject()
        .iter_common_name()
        .find_map(|name| name.as_str().ok())
        .map(|name| sanitize_text(name, 256));
    if let Some(subject) = subject {
        value["subject"] = Value::String(subject);
    }
    value["issuer"] = Value::String(sanitize_text(&parsed.issuer().to_string(), 512));
    value["not_before"] = Value::String(parsed.validity().not_before.to_string());
    value["not_after"] = Value::String(parsed.validity().not_after.to_string());
    value["valid_at_observation"] = Value::Bool(parsed.validity().is_valid());

    let mut sans = Vec::new();
    if let Ok(Some(extension)) = parsed.subject_alternative_name() {
        for name in &extension.value.general_names {
            let rendered = match name {
                GeneralName::DNSName(name) => Some((*name).to_string()),
                GeneralName::URI(name) => Some((*name).to_string()),
                GeneralName::IPAddress(bytes) => Some(match bytes.len() {
                    4 => IpAddr::from([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string(),
                    16 => {
                        let mut octets = [0u8; 16];
                        octets.copy_from_slice(bytes);
                        IpAddr::from(octets).to_string()
                    }
                    _ => hex::encode(bytes),
                }),
                _ => None,
            };
            if let Some(rendered) = rendered {
                sans.push(sanitize_text(&rendered, 256));
            }
        }
    }
    value["subject_alt_names"] = Value::Array(sans.into_iter().map(Value::String).collect());
    value["matches_target"] = Value::Bool(certificate_matches_target(&value, address));
    value
}

fn certificate_matches_target(value: &Value, address: IpAddr) -> bool {
    value["subject_alt_names"].as_array().is_some_and(|sans| {
        sans.iter().any(|san| {
            san.as_str()
                .is_some_and(|san| san.eq_ignore_ascii_case(&address.to_string()))
        })
    })
}

fn parse_response_headers(header_bytes: &[u8]) -> Result<ParsedResponseHeaders, ProbeError> {
    let text = std::str::from_utf8(header_bytes).map_err(|_| ProbeError::InvalidResponse)?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or(ProbeError::InvalidResponse)?;
    let mut status_parts = status_line.split_ascii_whitespace();
    let version = status_parts.next().ok_or(ProbeError::InvalidResponse)?;
    if !version.starts_with("HTTP/") {
        return Err(ProbeError::InvalidResponse);
    }
    let status = status_parts
        .next()
        .ok_or(ProbeError::InvalidResponse)?
        .parse::<u16>()
        .map_err(|_| ProbeError::InvalidResponse)?;
    let mut headers = BTreeMap::new();
    let mut location = None;
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = sanitize_text(value.trim(), MAX_STORED_TEXT_BYTES);
        if name == "location" {
            location = Some(value.clone());
        }
        if name == "content-length" {
            content_length = value.parse::<usize>().ok();
        }
        if HTTP_SAFE_HEADERS.contains(&name.as_str()) {
            headers.insert(name, value);
        }
        if headers.len() >= 32 {
            break;
        }
    }
    Ok(ParsedResponseHeaders {
        status,
        headers,
        location,
        content_length,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn extract_title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let content_start = lower[start..].find('>')? + start + 1;
    let content_end = lower[content_start..].find("</title>")? + content_start;
    let title = body[content_start..content_end].trim();
    (!title.is_empty()).then(|| sanitize_text(title, 512))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn same_endpoint_redirect(
    location: &str,
    address: IpAddr,
    port: u16,
    expected_scheme: &str,
) -> Option<String> {
    let location = location.trim();
    if location.starts_with('/') && !location.starts_with("//") {
        return Some(bounded_http_path(location));
    }

    let (scheme, rest) = if let Some(rest) = location.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = location.strip_prefix("https://") {
        ("https", rest)
    } else {
        return None;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let expected_host = address.to_string();
    let (host, explicit_port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let explicit_port = rest[end + 1..]
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok());
        (host, explicit_port)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|char| char.is_ascii_digit()))
            .map_or((authority, None), |(host, port)| {
                (host, port.parse::<u16>().ok())
            });
        (host, port)
    };
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    if scheme != expected_scheme
        || !host.eq_ignore_ascii_case(&expected_host)
        || explicit_port.unwrap_or(default_port) != port
    {
        return None;
    }
    let path = &rest[authority_end..];
    Some(bounded_http_path(if path.is_empty() { "/" } else { path }))
}

fn bounded_http_path(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let path = path.strip_prefix('#').unwrap_or(path);
    let mut bounded = path.chars().take(MAX_HTTP_PATH_BYTES).collect::<String>();
    if !bounded.starts_with('/') {
        bounded.insert(0, '/');
    }
    bounded
}

fn sanitize_text(value: &str, max_bytes: usize) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if result.len() + character.len_utf8() > max_bytes {
            break;
        }
        if character == '\r' || character == '\n' || character.is_control() {
            result.push(' ');
        } else {
            result.push(character);
        }
    }
    result.trim().to_string()
}

fn is_tls_port_hint(port: u16) -> bool {
    matches!(port, 443 | 465 | 636 | 853 | 993 | 995 | 8443 | 9443)
}

struct ObservationVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl fmt::Debug for ObservationVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservationVerifier").finish()
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
        // This verifier is scoped to a no-credential inventory probe. It lets
        // rustls finish a handshake so the classifier can inspect protocol
        // and certificate metadata, but the result is never used as trust.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
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
    ) -> Result<HandshakeSignatureValid, RustlsError> {
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
    use rcgen::{CertificateParams, KeyPair};
    use rustls::pki_types::PrivateKeyDer;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    fn test_config() -> ClassificationConfig {
        ClassificationConfig {
            connect_timeout: Duration::from_millis(250),
            read_timeout: Duration::from_millis(80),
            overall_timeout: Duration::from_secs(2),
            max_header_bytes: 4096,
            max_body_bytes: 1024,
            max_banner_bytes: 256,
            max_redirects: 2,
            max_connections: 8,
        }
    }

    async fn spawn_plain_server(response: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let _ = timeout(Duration::from_secs(1), stream.read(&mut request)).await;
                    let _ = stream.write_all(response).await;
                });
            }
        });
        address
    }

    async fn spawn_redirect_server() -> SocketAddr {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind redirect server");
        let address = listener.local_addr().expect("redirect server address");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let Ok(Ok(length)) =
                        timeout(Duration::from_secs(1), stream.read(&mut request)).await
                    else {
                        return;
                    };
                    let response = if request[..length].starts_with(b"GET / HTTP") {
                        b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice()
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone"
                            .as_slice()
                    };
                    let _ = stream.write_all(response).await;
                });
            }
        });
        address
    }

    async fn spawn_banner_server(banner: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind banner server");
        let address = listener.local_addr().expect("banner server address");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let _ = stream.write_all(banner).await;
                    let mut request = [0u8; 16];
                    let _ = timeout(Duration::from_millis(200), stream.read(&mut request)).await;
                });
            }
        });
        address
    }

    async fn spawn_tls_server() -> SocketAddr {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let key = KeyPair::generate().expect("generate TLS key");
        let params =
            CertificateParams::new(vec!["localhost".to_string()]).expect("TLS certificate params");
        let certificate = params.self_signed(&key).expect("self-sign TLS certificate");
        let certs = vec![CertificateDer::from(certificate.der().to_vec())];
        let key = PrivateKeyDer::try_from(key.serialize_der()).expect("TLS private key");
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("TLS server config");
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind TLS server");
        let address = listener.local_addr().expect("TLS server address");
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut request = [0u8; 2048];
                    let Ok(Ok(length)) =
                        timeout(Duration::from_secs(1), stream.read(&mut request)).await
                    else {
                        return;
                    };
                    if request[..length].starts_with(b"GET ") {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                            )
                            .await;
                    }
                });
            }
        });
        address
    }

    #[tokio::test]
    async fn classifies_plain_http_with_bounded_metadata() {
        let address = spawn_plain_server(
            b"HTTP/1.1 200 OK\r\nServer: fixture\r\nContent-Type: text/html\r\nContent-Length: 37\r\n\r\n<html><title>Fixture</title></html>",
        )
        .await;
        let classifier = Classifier::new(test_config()).expect("classifier config");
        let result = classifier.classify(address.ip(), address.port()).await;
        assert_eq!(result.protocol, ServiceProtocol::Http);
        assert_eq!(result.evidence["status"], 200);
        assert_eq!(result.evidence["title"], "Fixture");
        assert_eq!(result.evidence["headers"]["server"], "fixture");
    }

    #[tokio::test]
    async fn follows_bounded_same_endpoint_redirects() {
        let address = spawn_redirect_server().await;
        let classifier = Classifier::new(test_config()).expect("classifier config");
        let result = classifier.classify(address.ip(), address.port()).await;
        assert_eq!(result.protocol, ServiceProtocol::Http);
        assert_eq!(result.evidence["status"], 200);
        assert_eq!(result.evidence["redirects"][0], "/next");
    }

    #[tokio::test]
    async fn classifies_tls_and_http_without_trusting_certificate() {
        let address = spawn_tls_server().await;
        let classifier = Classifier::new(test_config()).expect("classifier config");
        let result = classifier.classify(address.ip(), address.port()).await;
        assert_eq!(result.protocol, ServiceProtocol::Https);
        assert_eq!(result.evidence["status"], 200);
        assert_eq!(result.evidence["tls"]["certificate_trust"], "not_validated");
        assert_eq!(
            result.evidence["tls"]["certificate"]
                .as_array()
                .expect("certificate array")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn classifies_ssh_from_banner_without_login() {
        let address = spawn_banner_server(b"SSH-2.0-OpenSSH_fixture\r\n").await;
        let classifier = Classifier::new(test_config()).expect("classifier config");
        let result = classifier.classify(address.ip(), address.port()).await;
        assert_eq!(result.protocol, ServiceProtocol::Ssh);
        assert_eq!(result.evidence["host_key_collected"], false);
        assert!(
            result.evidence["banner"]
                .as_str()
                .is_some_and(|banner| banner.starts_with("SSH-2.0-"))
        );
    }

    #[tokio::test]
    async fn preserves_generic_tcp_when_no_protocol_answers() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind generic server");
        let address = listener.local_addr().expect("generic server address");
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    drop(stream);
                });
            }
        });
        let classifier = Classifier::new(test_config()).expect("classifier config");
        let result = classifier.classify(address.ip(), address.port()).await;
        assert_eq!(result.protocol, ServiceProtocol::GenericTcp);
    }

    #[test]
    fn redirect_stays_on_same_endpoint() {
        let address: IpAddr = "192.168.1.20".parse().expect("fixture IP");
        assert_eq!(
            same_endpoint_redirect("/next?x=1", address, 8080, "http"),
            Some("/next?x=1".to_string())
        );
        assert_eq!(
            same_endpoint_redirect("http://192.168.1.21:8080/next", address, 8080, "http"),
            None
        );
        assert_eq!(
            same_endpoint_redirect("http://192.168.1.20:8081/next", address, 8080, "http"),
            None
        );
    }

    #[test]
    fn sensitive_headers_are_not_persisted() {
        let header = b"HTTP/1.1 200 OK\r\nSet-Cookie: secret=one\r\nServer: safe\r\n\r\n";
        let parsed = parse_response_headers(header).expect("parse headers");
        assert!(!parsed.headers.contains_key("set-cookie"));
        assert_eq!(parsed.headers.get("server"), Some(&"safe".to_string()));
    }

    #[test]
    fn config_rejects_unbounded_zero_limits() {
        let mut config = test_config();
        config.max_connections = 0;
        assert!(matches!(
            Classifier::new(config),
            Err(ClassificationError::InvalidConfig(_))
        ));
    }
}
