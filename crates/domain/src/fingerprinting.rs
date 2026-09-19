//! Pure, explainable product fingerprinting over bounded M2 evidence.
//!
//! The engine never opens a socket or follows a URL. Callers provide the
//! normalized fields collected by M2, then the engine evaluates pluggable
//! rules and returns deterministic candidates. Product rules require at least
//! two independent evidence fields unless the evidence is a structured SSH
//! implementation banner.

use std::collections::BTreeMap;

use serde::de::Error as SerdeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Version of the on-disk fingerprint fixture envelope.
pub const FIXTURE_FORMAT_VERSION: u32 = 1;
/// Version stamped on candidates emitted by the built-in rules.
pub const BUILTIN_FIXTURE_VERSION: u32 = 1;

const MAX_HEADERS: usize = 32;
const MAX_HEADER_NAME_BYTES: usize = 64;
const MAX_HEADER_VALUE_BYTES: usize = 4096;
const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_SAMPLE_BYTES: usize = 4096;
const MAX_BANNER_BYTES: usize = 512;
const MAX_REDIRECTS: usize = 8;
const MAX_REDIRECT_BYTES: usize = 4096;
const MAX_ALPN_BYTES: usize = 64;
const MAX_CERTIFICATES: usize = 4;
const MAX_CERT_TEXT_BYTES: usize = 512;
const MAX_CERT_SAN_BYTES: usize = 256;
const MAX_CERT_SANS: usize = 32;
const MAX_EVIDENCE_FIELDS: usize = 16;
const MAX_EVIDENCE_PATH_BYTES: usize = 128;
const MAX_EVIDENCE_VALUE_BYTES: usize = 512;
const MAX_REASONS: usize = 16;
const MAX_REASON_BYTES: usize = 256;
const MAX_PRODUCT_BYTES: usize = 128;
const M2_HTTP_HEADERS: &[&str] = &[
    "alt-svc",
    "content-length",
    "content-type",
    "location",
    "server",
    "via",
    "www-authenticate",
];

/// Protocol value emitted by M2 and carried into fingerprinting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FingerprintProtocol {
    Tcp,
    Http,
    Https,
    Tls,
    Ssh,
}

impl FingerprintProtocol {
    /// Return the stable wire spelling used in M2 evidence and candidates.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Http => "http",
            Self::Https => "https",
            Self::Tls => "tls",
            Self::Ssh => "ssh",
        }
    }

    const fn default_confidence(self) -> f32 {
        match self {
            Self::Tcp => 0.45,
            Self::Http | Self::Ssh => 0.98,
            Self::Https | Self::Tls => 0.95,
        }
    }
}

/// Bounded certificate metadata already reduced by M2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsCertificate {
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub not_before: Option<String>,
    #[serde(default)]
    pub not_after: Option<String>,
    #[serde(default)]
    pub subject_alt_names: Vec<String>,
    #[serde(default)]
    pub matches_target: Option<bool>,
}

/// Bounded TLS metadata already reduced by M2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsEvidence {
    #[serde(default)]
    pub alpn: Option<String>,
    #[serde(default, rename = "certificate", alias = "certificates")]
    pub certificates: Vec<TlsCertificate>,
}

/// Normalized input accepted by the pure rule engine.
///
/// Fields are private so callers cannot bypass the size and text checks. Use
/// [`FingerprintInput::builder`] or [`FingerprintInput::from_m2_evidence`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FingerprintInput {
    protocol: FingerprintProtocol,
    protocol_confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_sample: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    banner: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    redirects: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tls: Option<TlsEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alpn: Option<String>,
}

impl<'de> Deserialize<'de> for FingerprintInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawFingerprintInput::deserialize(deserializer)?;
        let protocol = raw
            .protocol
            .ok_or_else(|| D::Error::custom("fingerprint input requires protocol"))?;
        let confidence = raw
            .protocol_confidence
            .unwrap_or_else(|| protocol.default_confidence());
        raw.into_input(protocol, confidence)
            .map_err(D::Error::custom)
    }
}

impl FingerprintInput {
    /// Start building normalized input for one M2 protocol result.
    pub fn builder(protocol: FingerprintProtocol) -> FingerprintInputBuilder {
        FingerprintInputBuilder::new(protocol)
    }

    /// Convert the JSON object stored by M2 into normalized, bounded input.
    ///
    /// This function only parses existing evidence. It performs no network
    /// access, URL resolution, credential use, or persistence.
    pub fn from_m2_evidence(
        protocol: FingerprintProtocol,
        protocol_confidence: f32,
        evidence: &Value,
    ) -> Result<Self, FingerprintInputError> {
        let raw: RawFingerprintInput =
            serde_json::from_value(evidence.clone()).map_err(|error| {
                FingerprintInputError::InvalidValue {
                    field: "m2_evidence",
                    reason: error.to_string(),
                }
            })?;
        raw.into_input(protocol, protocol_confidence)
    }

    pub fn protocol(&self) -> FingerprintProtocol {
        self.protocol
    }

    pub fn protocol_confidence(&self) -> f32 {
        self.protocol_confidence
    }

    pub fn status(&self) -> Option<u16> {
        self.status
    }

    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn body_sample(&self) -> Option<&str> {
        self.body_sample.as_deref()
    }

    pub fn banner(&self) -> Option<&str> {
        self.banner.as_deref()
    }

    pub fn redirects(&self) -> &[String] {
        &self.redirects
    }

    pub fn tls(&self) -> Option<&TlsEvidence> {
        self.tls.as_ref()
    }

    pub fn alpn(&self) -> Option<&str> {
        self.alpn.as_deref()
    }
}

/// Builder for [`FingerprintInput`].
#[derive(Debug)]
pub struct FingerprintInputBuilder {
    protocol: FingerprintProtocol,
    protocol_confidence: Option<f32>,
    status: Option<u16>,
    headers: BTreeMap<String, String>,
    title: Option<String>,
    body_sample: Option<String>,
    banner: Option<String>,
    redirects: Vec<String>,
    tls: Option<TlsEvidence>,
    alpn: Option<String>,
}

impl Default for FingerprintInputBuilder {
    fn default() -> Self {
        Self {
            protocol: FingerprintProtocol::Tcp,
            protocol_confidence: None,
            status: None,
            headers: BTreeMap::new(),
            title: None,
            body_sample: None,
            banner: None,
            redirects: Vec::new(),
            tls: None,
            alpn: None,
        }
    }
}

impl FingerprintInputBuilder {
    fn new(protocol: FingerprintProtocol) -> Self {
        Self {
            protocol,
            ..Self::default()
        }
    }

    pub fn protocol_confidence(mut self, confidence: f32) -> Self {
        self.protocol_confidence = Some(confidence);
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn body_sample(mut self, body_sample: impl Into<String>) -> Self {
        self.body_sample = Some(body_sample.into());
        self
    }

    pub fn banner(mut self, banner: impl Into<String>) -> Self {
        self.banner = Some(banner.into());
        self
    }

    pub fn redirects(mut self, redirects: Vec<String>) -> Self {
        self.redirects = redirects;
        self
    }

    pub fn tls(mut self, tls: TlsEvidence) -> Self {
        self.tls = Some(tls);
        self
    }

    pub fn alpn(mut self, alpn: impl Into<String>) -> Self {
        self.alpn = Some(alpn.into());
        self
    }

    pub fn build(self) -> Result<FingerprintInput, FingerprintInputError> {
        let protocol_confidence = self
            .protocol_confidence
            .unwrap_or_else(|| self.protocol.default_confidence());
        validate_confidence(protocol_confidence, "protocol_confidence")?;

        if self.headers.len() > MAX_HEADERS {
            return Err(FingerprintInputError::TooManyValues {
                field: "headers",
                max: MAX_HEADERS,
            });
        }
        let mut headers = BTreeMap::new();
        for (name, value) in self.headers {
            let name = normalize_text("header_name", &name, MAX_HEADER_NAME_BYTES, false)?
                .to_ascii_lowercase();
            if !M2_HTTP_HEADERS.contains(&name.as_str()) {
                return Err(FingerprintInputError::InvalidValue {
                    field: "header_name",
                    reason: "not present in the M2 safe header allowlist".to_string(),
                });
            }
            let value = normalize_text("header_value", &value, MAX_HEADER_VALUE_BYTES, true)?;
            headers.insert(name, value);
        }

        let title = normalize_optional(self.title, "title", MAX_TITLE_BYTES, true)?;
        let body_sample = normalize_optional(
            self.body_sample,
            "body_sample",
            MAX_BODY_SAMPLE_BYTES,
            false,
        )?;
        let banner = normalize_optional(self.banner, "banner", MAX_BANNER_BYTES, false)?;
        if self.redirects.len() > MAX_REDIRECTS {
            return Err(FingerprintInputError::TooManyValues {
                field: "redirects",
                max: MAX_REDIRECTS,
            });
        }
        let redirects = self
            .redirects
            .into_iter()
            .map(|redirect| normalize_text("redirect", &redirect, MAX_REDIRECT_BYTES, true))
            .collect::<Result<Vec<_>, _>>()?;
        let tls = self.tls.map(normalize_tls).transpose()?;
        let alpn = normalize_optional(self.alpn, "alpn", MAX_ALPN_BYTES, true)?;

        Ok(FingerprintInput {
            protocol: self.protocol,
            protocol_confidence,
            status: self.status,
            headers,
            title,
            body_sample,
            banner,
            redirects,
            tls,
            alpn,
        })
    }
}

fn normalize_tls(tls: TlsEvidence) -> Result<TlsEvidence, FingerprintInputError> {
    let alpn = normalize_optional(tls.alpn, "tls.alpn", MAX_ALPN_BYTES, true)?;
    if tls.certificates.len() > MAX_CERTIFICATES {
        return Err(FingerprintInputError::TooManyValues {
            field: "tls.certificate",
            max: MAX_CERTIFICATES,
        });
    }
    let certificates = tls
        .certificates
        .into_iter()
        .map(|certificate| {
            let sha256 = normalize_optional(
                certificate.sha256,
                "tls.certificate.sha256",
                MAX_CERT_TEXT_BYTES,
                true,
            )?;
            let subject = normalize_optional(
                certificate.subject,
                "tls.certificate.subject",
                MAX_CERT_TEXT_BYTES,
                true,
            )?;
            let issuer = normalize_optional(
                certificate.issuer,
                "tls.certificate.issuer",
                MAX_CERT_TEXT_BYTES,
                true,
            )?;
            let not_before = normalize_optional(
                certificate.not_before,
                "tls.certificate.not_before",
                MAX_CERT_TEXT_BYTES,
                true,
            )?;
            let not_after = normalize_optional(
                certificate.not_after,
                "tls.certificate.not_after",
                MAX_CERT_TEXT_BYTES,
                true,
            )?;
            if certificate.subject_alt_names.len() > MAX_CERT_SANS {
                return Err(FingerprintInputError::TooManyValues {
                    field: "tls.certificate.subject_alt_names",
                    max: MAX_CERT_SANS,
                });
            }
            let subject_alt_names = certificate
                .subject_alt_names
                .into_iter()
                .map(|name| {
                    normalize_text(
                        "tls.certificate.subject_alt_name",
                        &name,
                        MAX_CERT_SAN_BYTES,
                        true,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(TlsCertificate {
                sha256,
                subject,
                issuer,
                not_before,
                not_after,
                subject_alt_names,
                matches_target: certificate.matches_target,
            })
        })
        .collect::<Result<Vec<_>, FingerprintInputError>>()?;
    Ok(TlsEvidence { alpn, certificates })
}

fn normalize_optional(
    value: Option<String>,
    field: &'static str,
    max: usize,
    trim: bool,
) -> Result<Option<String>, FingerprintInputError> {
    match value {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) => normalize_text(field, &value, max, trim).map(Some),
    }
}

fn normalize_text(
    field: &'static str,
    value: &str,
    max: usize,
    trim: bool,
) -> Result<String, FingerprintInputError> {
    let value = if trim { value.trim() } else { value };
    if value.is_empty() {
        return Err(FingerprintInputError::InvalidValue {
            field,
            reason: "must not be empty".to_string(),
        });
    }
    if value.len() > max {
        return Err(FingerprintInputError::FieldTooLong { field, max });
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(FingerprintInputError::InvalidValue {
            field,
            reason: "contains a control character".to_string(),
        });
    }
    Ok(value.to_string())
}

fn validate_confidence(confidence: f32, field: &'static str) -> Result<(), FingerprintInputError> {
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(FingerprintInputError::InvalidConfidence { field, confidence });
    }
    Ok(())
}

/// Errors returned while constructing normalized input.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum FingerprintInputError {
    #[error("{field} exceeds {max} bytes")]
    FieldTooLong { field: &'static str, max: usize },
    #[error("{field} has more than {max} values")]
    TooManyValues { field: &'static str, max: usize },
    #[error("{field} has invalid confidence {confidence}")]
    InvalidConfidence {
        field: &'static str,
        confidence: f32,
    },
    #[error("{field} is invalid: {reason}")]
    InvalidValue { field: &'static str, reason: String },
}

#[derive(Debug, Deserialize)]
struct RawFingerprintInput {
    #[serde(default)]
    protocol: Option<FingerprintProtocol>,
    #[serde(default)]
    protocol_confidence: Option<f32>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body_sample: Option<String>,
    #[serde(default)]
    banner: Option<String>,
    #[serde(default)]
    redirects: Vec<String>,
    #[serde(default)]
    tls: Option<TlsEvidence>,
    #[serde(default)]
    alpn: Option<String>,
    #[serde(default, rename = "certificate", alias = "certificates")]
    certificates: Vec<TlsCertificate>,
}

impl RawFingerprintInput {
    fn into_input(
        self,
        protocol: FingerprintProtocol,
        protocol_confidence: f32,
    ) -> Result<FingerprintInput, FingerprintInputError> {
        let tls = self.tls.or_else(|| {
            (self.alpn.is_some() || !self.certificates.is_empty()).then_some(TlsEvidence {
                alpn: self.alpn.clone(),
                certificates: self.certificates.clone(),
            })
        });
        FingerprintInput::builder(protocol)
            .protocol_confidence(protocol_confidence)
            .status_option(self.status)
            .headers(self.headers)
            .title_option(self.title)
            .body_sample_option(self.body_sample)
            .banner_option(self.banner)
            .redirects(self.redirects)
            .tls_option(tls)
            .alpn_option(self.alpn)
            .build()
    }
}

trait FingerprintInputBuilderExt {
    fn status_option(self, status: Option<u16>) -> Self;
    fn title_option(self, title: Option<String>) -> Self;
    fn body_sample_option(self, body_sample: Option<String>) -> Self;
    fn banner_option(self, banner: Option<String>) -> Self;
    fn tls_option(self, tls: Option<TlsEvidence>) -> Self;
    fn alpn_option(self, alpn: Option<String>) -> Self;
}

impl FingerprintInputBuilderExt for FingerprintInputBuilder {
    fn status_option(mut self, status: Option<u16>) -> Self {
        self.status = status;
        self
    }

    fn title_option(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    fn body_sample_option(mut self, body_sample: Option<String>) -> Self {
        self.body_sample = body_sample;
        self
    }

    fn banner_option(mut self, banner: Option<String>) -> Self {
        self.banner = banner;
        self
    }

    fn tls_option(mut self, tls: Option<TlsEvidence>) -> Self {
        self.tls = tls;
        self
    }

    fn alpn_option(mut self, alpn: Option<String>) -> Self {
        self.alpn = alpn;
        self
    }
}

/// Stable identity and fixture version for a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMetadata {
    rule_id: String,
    fixture_version: u32,
}

impl RuleMetadata {
    pub fn new(
        rule_id: impl Into<String>,
        fixture_version: u32,
    ) -> Result<Self, RuleMetadataError> {
        let rule_id = rule_id.into();
        if rule_id.is_empty() || rule_id.len() > MAX_EVIDENCE_PATH_BYTES {
            return Err(RuleMetadataError::InvalidRuleId);
        }
        Ok(Self {
            rule_id,
            fixture_version,
        })
    }

    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    pub const fn fixture_version(&self) -> u32 {
        self.fixture_version
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RuleMetadataError {
    #[error("rule id must contain 1..={MAX_EVIDENCE_PATH_BYTES} bytes")]
    InvalidRuleId,
}

/// One exact bounded field used by a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceField {
    pub path: String,
    pub value: String,
}

/// Validated output payload produced by a rule before the engine stamps rule
/// metadata and applies protocol confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatch {
    protocol: FingerprintProtocol,
    product: Option<String>,
    version: Option<String>,
    confidence: f32,
    evidence_fields: Vec<EvidenceField>,
    reasons: Vec<String>,
}

impl RuleMatch {
    pub fn new(
        protocol: FingerprintProtocol,
        product: Option<String>,
        version: Option<String>,
        confidence: f32,
        evidence_fields: Vec<EvidenceField>,
        reasons: Vec<String>,
    ) -> Result<Self, RuleMatchError> {
        validate_confidence_output(confidence)?;
        let product = normalize_output_optional(product, "product", MAX_PRODUCT_BYTES)?;
        let version = normalize_output_optional(version, "version", MAX_PRODUCT_BYTES)?;
        if evidence_fields.is_empty() || evidence_fields.len() > MAX_EVIDENCE_FIELDS {
            return Err(RuleMatchError::InvalidEvidenceFieldCount);
        }
        for field in &evidence_fields {
            validate_output_text(&field.path, MAX_EVIDENCE_PATH_BYTES, "evidence path")?;
            validate_output_text(&field.value, MAX_EVIDENCE_VALUE_BYTES, "evidence value")?;
        }
        if reasons.is_empty() || reasons.len() > MAX_REASONS {
            return Err(RuleMatchError::InvalidReasonCount);
        }
        for reason in &reasons {
            validate_output_text(reason, MAX_REASON_BYTES, "reason")?;
        }
        Ok(Self {
            protocol,
            product,
            version,
            confidence,
            evidence_fields,
            reasons,
        })
    }
}

fn validate_confidence_output(confidence: f32) -> Result<(), RuleMatchError> {
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(RuleMatchError::InvalidConfidence { confidence });
    }
    Ok(())
}

fn normalize_output_optional(
    value: Option<String>,
    field: &'static str,
    max: usize,
) -> Result<Option<String>, RuleMatchError> {
    value
        .map(|value| {
            validate_output_text(&value, max, field)?;
            Ok(value)
        })
        .transpose()
}

fn validate_output_text(
    value: &str,
    max: usize,
    field: &'static str,
) -> Result<(), RuleMatchError> {
    if value.is_empty() || value.len() > max {
        return Err(RuleMatchError::InvalidOutputText { field, max });
    }
    Ok(())
}

#[derive(Debug, Clone, Error, PartialEq)]
pub enum RuleMatchError {
    #[error("rule match confidence must be finite and within 0..=1; got {confidence}")]
    InvalidConfidence { confidence: f32 },
    #[error("rule match has invalid evidence field count")]
    InvalidEvidenceFieldCount,
    #[error("rule match has invalid reason count")]
    InvalidReasonCount,
    #[error("rule match {field} must contain 1..={max} bytes")]
    InvalidOutputText { field: &'static str, max: usize },
}

/// Candidate returned to reconciliation and the UI's Why? view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintCandidate {
    pub rule_id: String,
    pub fixture_version: u32,
    pub protocol: FingerprintProtocol,
    pub product: Option<String>,
    pub version: Option<String>,
    pub confidence: f32,
    pub evidence_fields: Vec<EvidenceField>,
    pub reasons: Vec<String>,
}

/// Ordered candidate set. First candidate is highest confidence, then stable
/// rule ID order breaks ties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintReport {
    pub candidates: Vec<FingerprintCandidate>,
}

impl FingerprintReport {
    pub fn best(&self) -> Option<&FingerprintCandidate> {
        self.candidates.first()
    }

    pub fn has_product(&self) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.product.is_some())
    }
}

/// Pure extension point for product and protocol signatures.
pub trait FingerprintRule: Send + Sync {
    fn metadata(&self) -> RuleMetadata;
    fn evaluate(&self, input: &FingerprintInput) -> Option<RuleMatch>;
}

/// Deterministic fingerprint rule engine.
pub struct FingerprintEngine {
    rules: Vec<Box<dyn FingerprintRule>>,
}

impl Default for FingerprintEngine {
    fn default() -> Self {
        Self::with_builtin_rules()
    }
}

impl FingerprintEngine {
    /// Create an engine from caller-supplied rules. Rules run in vector order;
    /// output ordering remains deterministic after confidence sorting.
    pub fn new(rules: Vec<Box<dyn FingerprintRule>>) -> Self {
        Self { rules }
    }

    /// Create an engine with built-in safe homelab signatures.
    pub fn with_builtin_rules() -> Self {
        let mut rules: Vec<Box<dyn FingerprintRule>> = HTTP_SIGNATURES
            .iter()
            .map(|signature| Box::new(HttpSignatureRule { signature }) as Box<dyn FingerprintRule>)
            .collect();
        rules.push(Box::new(SshBannerRule {
            product: "OpenSSH",
            rule_id: "ssh.openssh",
            prefix: "SSH-2.0-OpenSSH_",
        }));
        rules.push(Box::new(SshBannerRule {
            product: "Dropbear",
            rule_id: "ssh.dropbear",
            prefix: "SSH-2.0-dropbear",
        }));
        Self { rules }
    }

    pub fn add_rule<R>(&mut self, rule: R)
    where
        R: FingerprintRule + 'static,
    {
        self.rules.push(Box::new(rule));
    }

    pub fn fingerprint(&self, input: &FingerprintInput) -> FingerprintReport {
        let mut candidates: Vec<_> = self
            .rules
            .iter()
            .filter_map(|rule| {
                let output = rule.evaluate(input)?;
                Some(candidate_from_rule(rule.metadata(), output, input))
            })
            .collect();

        if candidates.is_empty() {
            candidates.push(generic_fallback(input));
        }
        candidates.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then_with(|| left.rule_id.cmp(&right.rule_id))
        });
        FingerprintReport { candidates }
    }
}

fn candidate_from_rule(
    metadata: RuleMetadata,
    output: RuleMatch,
    input: &FingerprintInput,
) -> FingerprintCandidate {
    let mut evidence_fields = output.evidence_fields;
    evidence_fields.sort_by(|left, right| left.path.cmp(&right.path));
    let confidence = output.confidence.min(input.protocol_confidence());
    FingerprintCandidate {
        rule_id: metadata.rule_id,
        fixture_version: metadata.fixture_version,
        protocol: output.protocol,
        product: output.product,
        version: output.version,
        confidence,
        evidence_fields,
        reasons: output.reasons,
    }
}

fn generic_fallback(input: &FingerprintInput) -> FingerprintCandidate {
    let protocol = input.protocol();
    FingerprintCandidate {
        rule_id: format!("fallback.{}", protocol.as_str()),
        fixture_version: BUILTIN_FIXTURE_VERSION,
        protocol,
        product: None,
        version: None,
        confidence: input.protocol_confidence(),
        evidence_fields: vec![EvidenceField {
            path: "protocol".to_string(),
            value: protocol.as_str().to_string(),
        }],
        reasons: vec![format!(
            "protocol is `{}`; no product signature matched bounded M2 evidence",
            protocol.as_str()
        )],
    }
}

#[derive(Debug, Clone, Copy)]
enum SignatureField {
    Body,
    CertificateSubject,
    CertificateSan,
    Server,
    Title,
}

impl SignatureField {
    const fn path(self) -> &'static str {
        match self {
            Self::Body => "body_sample",
            Self::CertificateSubject => "tls.certificate[].subject",
            Self::CertificateSan => "tls.certificate[].subject_alt_names",
            Self::Server => "headers.server",
            Self::Title => "title",
        }
    }

    const fn order(self) -> u8 {
        match self {
            Self::Body => 0,
            Self::CertificateSubject => 1,
            Self::CertificateSan => 2,
            Self::Server => 3,
            Self::Title => 4,
        }
    }
}

struct HttpSignature {
    rule_id: &'static str,
    product: &'static str,
    title_needles: &'static [&'static str],
    server_needles: &'static [&'static str],
    body_needles: &'static [&'static str],
    certificate_needles: &'static [&'static str],
}

struct MatchedMarker {
    field: SignatureField,
    needle: &'static str,
    observed: String,
}

struct HttpSignatureRule {
    signature: &'static HttpSignature,
}

impl FingerprintRule for HttpSignatureRule {
    fn metadata(&self) -> RuleMetadata {
        builtin_metadata(self.signature.rule_id)
    }

    fn evaluate(&self, input: &FingerprintInput) -> Option<RuleMatch> {
        if !matches!(
            input.protocol(),
            FingerprintProtocol::Http | FingerprintProtocol::Https
        ) {
            return None;
        }

        let mut matches = Vec::new();
        if let Some(title) = input.title()
            && let Some((needle, _)) = matching_marker(title, self.signature.title_needles)
        {
            matches.push(MatchedMarker {
                field: SignatureField::Title,
                needle,
                observed: title.to_string(),
            });
        }
        if let Some(server) = input.header("server")
            && let Some((needle, _)) = matching_marker(server, self.signature.server_needles)
        {
            matches.push(MatchedMarker {
                field: SignatureField::Server,
                needle,
                observed: server.to_string(),
            });
        }
        if let Some(body) = input.body_sample()
            && let Some((needle, _)) = matching_marker(body, self.signature.body_needles)
        {
            matches.push(MatchedMarker {
                field: SignatureField::Body,
                needle,
                observed: needle.to_string(),
            });
        }
        if let Some(tls) = input.tls() {
            for certificate in &tls.certificates {
                if let Some(subject) = certificate.subject.as_deref()
                    && let Some((needle, _)) =
                        matching_marker(subject, self.signature.certificate_needles)
                {
                    matches.push(MatchedMarker {
                        field: SignatureField::CertificateSubject,
                        needle,
                        observed: subject.to_string(),
                    });
                    break;
                }
                if certificate
                    .subject_alt_names
                    .iter()
                    .any(|san| matching_marker(san, self.signature.certificate_needles).is_some())
                    && let Some(san) = certificate.subject_alt_names.iter().find(|san| {
                        matching_marker(san, self.signature.certificate_needles).is_some()
                    })
                {
                    let (needle, _) = matching_marker(san, self.signature.certificate_needles)?;
                    matches.push(MatchedMarker {
                        field: SignatureField::CertificateSan,
                        needle,
                        observed: san.clone(),
                    });
                    break;
                }
            }
        }

        if matches.len() < 2 {
            return None;
        }
        matches.sort_by_key(|matched| matched.field.order());
        let evidence_fields = matches
            .iter()
            .map(|matched| EvidenceField {
                path: matched.field.path().to_string(),
                value: matched.observed.clone(),
            })
            .collect::<Vec<_>>();
        let reasons = matches
            .iter()
            .map(|matched| {
                format!(
                    "{} contains signature marker `{}`",
                    matched.field.path(),
                    matched.needle
                )
            })
            .collect::<Vec<_>>();
        let version = matches.iter().find_map(|matched| {
            matches!(
                matched.field,
                SignatureField::Server | SignatureField::Title
            )
            .then(|| extract_version(&matched.observed, matched.needle))
            .flatten()
        });
        let confidence = product_confidence(matches.len()).min(input.protocol_confidence());
        RuleMatch::new(
            input.protocol(),
            Some(self.signature.product.to_string()),
            version,
            confidence,
            evidence_fields,
            reasons,
        )
        .ok()
    }
}

fn matching_marker<'a>(text: &str, needles: &'a [&'a str]) -> Option<(&'a str, &'a str)> {
    let text = text.to_ascii_lowercase();
    needles.iter().find_map(|needle| {
        text.contains(&needle.to_ascii_lowercase())
            .then_some((*needle, *needle))
    })
}

fn product_confidence(field_count: usize) -> f32 {
    match field_count {
        0 | 1 => 0.0,
        2 => 0.86,
        3 => 0.93,
        _ => 0.96,
    }
}

fn extract_version(text: &str, marker: &str) -> Option<String> {
    let text_lower = text.to_ascii_lowercase();
    let marker_lower = marker.to_ascii_lowercase();
    let marker_start = text_lower.find(&marker_lower)? + marker.len();
    let suffix = text
        .get(marker_start..)?
        .trim_start_matches(|character: char| {
            matches!(character, ' ' | '\t' | '/' | ':' | '-' | '_' | 'v' | 'V')
        });
    let first_digit = suffix.find(|character: char| character.is_ascii_digit())?;
    let suffix = &suffix[first_digit..];
    let mut end = 0;
    let mut dots = 0;
    for character in suffix.chars() {
        if character.is_ascii_digit() {
            end += character.len_utf8();
        } else if character == '.' {
            dots += 1;
            end += 1;
        } else {
            break;
        }
    }
    (dots > 0 && end > 0).then(|| suffix[..end].to_string())
}

struct SshBannerRule {
    product: &'static str,
    rule_id: &'static str,
    prefix: &'static str,
}

impl FingerprintRule for SshBannerRule {
    fn metadata(&self) -> RuleMetadata {
        builtin_metadata(self.rule_id)
    }

    fn evaluate(&self, input: &FingerprintInput) -> Option<RuleMatch> {
        if input.protocol() != FingerprintProtocol::Ssh {
            return None;
        }
        let banner = input.banner()?.lines().next()?.trim();
        if !banner.starts_with(self.prefix) {
            return None;
        }
        let version = extract_version(banner, self.prefix);
        RuleMatch::new(
            FingerprintProtocol::Ssh,
            Some(self.product.to_string()),
            version,
            0.92_f32.min(input.protocol_confidence()),
            vec![EvidenceField {
                path: "banner".to_string(),
                value: banner.to_string(),
            }],
            vec![format!(
                "banner starts with structured SSH implementation prefix `{}`",
                self.prefix
            )],
        )
        .ok()
    }
}

fn builtin_metadata(rule_id: &str) -> RuleMetadata {
    RuleMetadata {
        rule_id: rule_id.to_string(),
        fixture_version: BUILTIN_FIXTURE_VERSION,
    }
}

const HTTP_SIGNATURES: &[HttpSignature] = &[
    HttpSignature {
        rule_id: "http.adguard_home",
        product: "AdGuard Home",
        title_needles: &["adguard home"],
        server_needles: &["adguard home", "adguardhome"],
        body_needles: &["adguard home", "adguardhome"],
        certificate_needles: &["adguard"],
    },
    HttpSignature {
        rule_id: "http.grafana",
        product: "Grafana",
        title_needles: &["grafana"],
        server_needles: &["grafana"],
        body_needles: &["grafana"],
        certificate_needles: &["grafana"],
    },
    HttpSignature {
        rule_id: "http.home_assistant",
        product: "Home Assistant",
        title_needles: &["home assistant"],
        server_needles: &["home assistant", "home-assistant"],
        body_needles: &["home assistant", "home-assistant"],
        certificate_needles: &["home assistant", "home-assistant"],
    },
    HttpSignature {
        rule_id: "http.immich",
        product: "Immich",
        title_needles: &["immich"],
        server_needles: &["immich"],
        body_needles: &["immich"],
        certificate_needles: &["immich"],
    },
    HttpSignature {
        rule_id: "http.jellyfin",
        product: "Jellyfin",
        title_needles: &["jellyfin"],
        server_needles: &["jellyfin"],
        body_needles: &["jellyfin"],
        certificate_needles: &["jellyfin"],
    },
    HttpSignature {
        rule_id: "http.lidarr",
        product: "Lidarr",
        title_needles: &["lidarr"],
        server_needles: &["lidarr"],
        body_needles: &["lidarr"],
        certificate_needles: &["lidarr"],
    },
    HttpSignature {
        rule_id: "http.nextcloud",
        product: "Nextcloud",
        title_needles: &["nextcloud"],
        server_needles: &["nextcloud"],
        body_needles: &["nextcloud"],
        certificate_needles: &["nextcloud"],
    },
    HttpSignature {
        rule_id: "http.opnsense",
        product: "OPNsense",
        title_needles: &["opnsense"],
        server_needles: &["opnsense"],
        body_needles: &["opnsense"],
        certificate_needles: &["opnsense"],
    },
    HttpSignature {
        rule_id: "http.pi_hole",
        product: "Pi-hole",
        title_needles: &["pi-hole", "pi hole"],
        server_needles: &["pi-hole", "pi hole"],
        body_needles: &["pi-hole", "pi hole"],
        certificate_needles: &["pi-hole", "pi hole"],
    },
    HttpSignature {
        rule_id: "http.plex",
        product: "Plex",
        title_needles: &["plex"],
        server_needles: &["plex media server", "plex"],
        body_needles: &["plex"],
        certificate_needles: &["plex"],
    },
    HttpSignature {
        rule_id: "http.portainer",
        product: "Portainer",
        title_needles: &["portainer"],
        server_needles: &["portainer"],
        body_needles: &["portainer"],
        certificate_needles: &["portainer"],
    },
    HttpSignature {
        rule_id: "http.proxmox",
        product: "Proxmox VE",
        title_needles: &["proxmox"],
        server_needles: &["proxmox"],
        body_needles: &["proxmox"],
        certificate_needles: &["proxmox"],
    },
    HttpSignature {
        rule_id: "http.radarr",
        product: "Radarr",
        title_needles: &["radarr"],
        server_needles: &["radarr"],
        body_needles: &["radarr"],
        certificate_needles: &["radarr"],
    },
    HttpSignature {
        rule_id: "http.readarr",
        product: "Readarr",
        title_needles: &["readarr"],
        server_needles: &["readarr"],
        body_needles: &["readarr"],
        certificate_needles: &["readarr"],
    },
    HttpSignature {
        rule_id: "http.sonarr",
        product: "Sonarr",
        title_needles: &["sonarr"],
        server_needles: &["sonarr"],
        body_needles: &["sonarr"],
        certificate_needles: &["sonarr"],
    },
    HttpSignature {
        rule_id: "http.truenas",
        product: "TrueNAS",
        title_needles: &["truenas", "true nas"],
        server_needles: &["truenas", "true nas"],
        body_needles: &["truenas", "true nas"],
        certificate_needles: &["truenas", "true nas"],
    },
    HttpSignature {
        rule_id: "http.unifi",
        product: "UniFi",
        title_needles: &["unifi", "uniﬁ"],
        server_needles: &["unifi"],
        body_needles: &["unifi"],
        certificate_needles: &["unifi"],
    },
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct FixtureCase {
        format_version: u32,
        input: FingerprintInput,
        expected: FixtureExpected,
    }

    #[derive(Debug, Deserialize)]
    struct FixtureExpected {
        rule_id: String,
        fixture_version: u32,
        protocol: FingerprintProtocol,
        product: Option<String>,
        version: Option<String>,
        confidence: f32,
        evidence_paths: Vec<String>,
    }

    fn fixture(raw: &str) -> FixtureCase {
        serde_json::from_str(raw).expect("valid fingerprint fixture")
    }

    fn assert_fixture(raw: &str) {
        let fixture = fixture(raw);
        assert_eq!(fixture.format_version, FIXTURE_FORMAT_VERSION);
        let report = FingerprintEngine::default().fingerprint(&fixture.input);
        let candidate = report.best().expect("fixture must emit candidate");
        assert_eq!(candidate.rule_id, fixture.expected.rule_id);
        assert_eq!(candidate.fixture_version, fixture.expected.fixture_version);
        assert_eq!(candidate.protocol, fixture.expected.protocol);
        assert_eq!(candidate.product, fixture.expected.product);
        assert_eq!(candidate.version, fixture.expected.version);
        assert_eq!(candidate.confidence, fixture.expected.confidence);
        assert_eq!(
            candidate
                .evidence_fields
                .iter()
                .map(|field| field.path.clone())
                .collect::<Vec<_>>(),
            fixture.expected.evidence_paths
        );
        for field in &candidate.evidence_fields {
            assert!(
                candidate
                    .reasons
                    .iter()
                    .any(|reason| reason.contains(&field.path)),
                "missing reason for {}",
                field.path
            );
        }
    }

    #[test]
    fn built_in_fixtures_are_deterministic() {
        assert_fixture(include_str!("../fixtures/fingerprinting/v1/plex.json"));
        assert_fixture(include_str!("../fixtures/fingerprinting/v1/jellyfin.json"));
        assert_fixture(include_str!(
            "../fixtures/fingerprinting/v1/home-assistant.json"
        ));
        assert_fixture(include_str!("../fixtures/fingerprinting/v1/proxmox.json"));
        assert_fixture(include_str!("../fixtures/fingerprinting/v1/openssh.json"));
        assert_fixture(include_str!(
            "../fixtures/fingerprinting/v1/unknown-http.json"
        ));
        assert_fixture(include_str!(
            "../fixtures/fingerprinting/v1/unknown-tcp.json"
        ));
        assert_fixture(include_str!("../fixtures/fingerprinting/v1/weak-http.json"));
    }

    #[test]
    fn weak_one_field_evidence_does_not_create_product_guess() {
        let input = FingerprintInput::builder(FingerprintProtocol::Http)
            .title("Plex")
            .build()
            .expect("bounded input");
        let report = FingerprintEngine::default().fingerprint(&input);
        let candidate = report.best().expect("fallback candidate");
        assert_eq!(candidate.product, None);
        assert_eq!(candidate.rule_id, "fallback.http");
    }

    #[test]
    fn m2_evidence_adapter_reads_bounded_http_shape() {
        let evidence = json!({
            "protocol": "https",
            "status": 200,
            "headers": {"server": "Proxmox"},
            "title": "Proxmox VE",
            "body_sample": "proxmox web shell",
            "tls": {
                "alpn": "http/1.1",
                "certificate": [{"subject": "pve.lab"}]
            }
        });
        let input = FingerprintInput::from_m2_evidence(FingerprintProtocol::Https, 0.95, &evidence)
            .expect("M2 evidence is valid");
        let candidate = FingerprintEngine::default()
            .fingerprint(&input)
            .best()
            .expect("product candidate")
            .clone();
        assert_eq!(candidate.product.as_deref(), Some("Proxmox VE"));
        assert_eq!(candidate.protocol, FingerprintProtocol::Https);
        assert!(
            candidate
                .evidence_fields
                .iter()
                .any(|field| field.path == "title")
        );
    }

    #[test]
    fn root_tls_certificate_shape_is_supported() {
        let evidence = json!({
            "protocol": "tls",
            "alpn": "h2",
            "certificate": [{"subject": "lab.example"}]
        });
        let input = FingerprintInput::from_m2_evidence(FingerprintProtocol::Tls, 0.95, &evidence)
            .expect("M2 TLS evidence is valid");
        assert_eq!(input.alpn(), Some("h2"));
        assert_eq!(input.tls().map(|tls| tls.certificates.len()), Some(1));
        assert_eq!(
            FingerprintEngine::default()
                .fingerprint(&input)
                .best()
                .expect("fallback")
                .rule_id,
            "fallback.tls"
        );
    }

    #[test]
    fn input_bounds_reject_unbounded_fields() {
        let too_long = "x".repeat(MAX_BODY_SAMPLE_BYTES + 1);
        let error = FingerprintInput::builder(FingerprintProtocol::Http)
            .body_sample(too_long)
            .build()
            .expect_err("body sample must be bounded");
        assert!(matches!(
            error,
            FingerprintInputError::FieldTooLong {
                field: "body_sample",
                ..
            }
        ));

        let mut headers = BTreeMap::new();
        for index in 0..=MAX_HEADERS {
            headers.insert(format!("x-{index}"), "value".to_string());
        }
        assert!(matches!(
            FingerprintInput::builder(FingerprintProtocol::Http)
                .headers(headers)
                .build(),
            Err(FingerprintInputError::TooManyValues {
                field: "headers",
                ..
            })
        ));
    }

    #[test]
    fn output_constructor_rejects_invalid_confidence() {
        let error = RuleMatch::new(
            FingerprintProtocol::Http,
            Some("Fake".to_string()),
            None,
            1.01,
            vec![EvidenceField {
                path: "title".to_string(),
                value: "Fake".to_string(),
            }],
            vec!["reason".to_string()],
        )
        .expect_err("confidence must stay in range");
        assert!(matches!(error, RuleMatchError::InvalidConfidence { .. }));
    }

    struct CustomRule;

    impl FingerprintRule for CustomRule {
        fn metadata(&self) -> RuleMetadata {
            RuleMetadata::new("custom.example", 7).expect("static metadata")
        }

        fn evaluate(&self, input: &FingerprintInput) -> Option<RuleMatch> {
            (input.header("server") == Some("fixture")).then(|| {
                RuleMatch::new(
                    input.protocol(),
                    Some("Fixture Product".to_string()),
                    Some("1.2.3".to_string()),
                    0.77,
                    vec![EvidenceField {
                        path: "headers.server".to_string(),
                        value: "fixture".to_string(),
                    }],
                    vec!["headers.server contains fixture marker".to_string()],
                )
                .expect("static custom match")
            })
        }
    }

    #[test]
    fn custom_rules_are_pluggable_and_metadata_is_preserved() {
        let mut engine = FingerprintEngine::new(Vec::new());
        engine.add_rule(CustomRule);
        let input = FingerprintInput::builder(FingerprintProtocol::Http)
            .header("Server", "fixture")
            .build()
            .expect("bounded input");
        let report = engine.fingerprint(&input);
        let candidate = report.best().expect("custom candidate");
        assert_eq!(candidate.rule_id, "custom.example");
        assert_eq!(candidate.fixture_version, 7);
        assert_eq!(candidate.product.as_deref(), Some("Fixture Product"));
        assert_eq!(candidate.confidence, 0.77);
    }

    proptest! {
        #[test]
        fn candidate_confidence_and_order_are_always_bounded(
            title in proptest::string::string_regex("[A-Za-z0-9 _.-]{0,128}").expect("regex"),
            body in proptest::string::string_regex("[A-Za-z0-9 _./:-]{0,256}").expect("regex"),
            server in proptest::string::string_regex("[A-Za-z0-9 _./:-]{0,128}").expect("regex"),
        ) {
            let mut builder = FingerprintInput::builder(FingerprintProtocol::Http);
            if !title.is_empty() {
                builder = builder.title(title);
            }
            if !body.is_empty() {
                builder = builder.body_sample(body);
            }
            if !server.trim().is_empty() {
                builder = builder.header("server", server);
            }
            let input = builder.build().expect("generated fields are bounded");
            let report = FingerprintEngine::default().fingerprint(&input);
            prop_assert!(!report.candidates.is_empty());
            for window in report.candidates.windows(2) {
                prop_assert!(window[0].confidence >= window[1].confidence);
                if window[0].confidence == window[1].confidence {
                    prop_assert!(window[0].rule_id <= window[1].rule_id);
                }
            }
            for candidate in report.candidates {
                prop_assert!(candidate.confidence.is_finite());
                prop_assert!((0.0..=1.0).contains(&candidate.confidence));
            }
        }
    }
}
