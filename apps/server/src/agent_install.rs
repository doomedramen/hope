//! Public bootstrap endpoints for the manual Linux agent installer.
//!
//! The installer is intentionally served by the same control-plane origin as
//! the UI. Release downloads are resolved through the verified, filesystem
//! backed repository; GitHub is never consulted by an agent host.

use axum::Extension;
use axum::Json;
use axum::body::Body;
use axum::extract::Path;
use axum::http::header;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::agents;
use crate::auth_mw::CurrentUser;
use crate::config::Config;
use crate::pki;
use crate::release_repository::{ReleaseRepository, RepositoryError};

const INSTALL_SCRIPT: &str = include_str!("../../../deploy/agent/install-agent.sh");
const SUPPORTED_PLATFORM: &str = "linux";
const ENROLLMENT_TTL_MINUTES: i64 = 15;

#[derive(Debug, Deserialize)]
pub struct ReleasePath {
    version: String,
    platform: String,
    arch: String,
    file: String,
}

/// Create the short-lived bootstrap code used by the one-command installer.
/// The plaintext code is returned only to the authenticated operator that
/// requested it; the database stores only its hash.
pub async fn create_enrollment(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    Extension(_user): Extension<CurrentUser>,
) -> Response {
    let config = match Config::load() {
        Ok(config) => config,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent enrollment is not configured",
            );
        }
    };
    let token = match agents::create_enrollment_token(&state.pool, ENROLLMENT_TTL_MINUTES).await {
        Ok(token) => token,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "could not create an agent enrollment code",
            );
        }
    };
    let cert_bytes = match std::fs::read(&config.server_cert_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent TLS certificate is unavailable",
            );
        }
    };
    let mut cert_reader = cert_bytes.as_slice();
    let Some(Ok(leaf)) = rustls_pemfile::certs(&mut cert_reader).next() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent TLS certificate is invalid",
        );
    };
    let code = format!("{token}.{}", pki::fingerprint_der(leaf.as_ref()));
    let Ok((_, parsed)) = x509_parser::parse_x509_certificate(leaf.as_ref()) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent TLS certificate is invalid",
        );
    };
    let tls_pin = format!(
        "sha256//{}",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(parsed.public_key().raw)),
    );
    (
        StatusCode::CREATED,
        Json(json!({
            "code": code,
            "expires_in_minutes": ENROLLMENT_TTL_MINUTES,
            "tls_pin": tls_pin,
        })),
    )
        .into_response()
}

/// GET /agent/install.sh
pub async fn script() -> Response {
    bytes_response(
        INSTALL_SCRIPT.as_bytes().to_vec(),
        "text/x-shellscript; charset=utf-8",
        "no-store",
        None,
    )
}

/// GET /agent/v1/releases/latest/:platform/:arch
///
/// Returns only the selected version. Keeping this separate from the bundle
/// files gives the shell installer a stable, dependency-free way to pin all
/// three subsequent requests to one verified release.
pub async fn latest_version(Path((platform, arch)): Path<(String, String)>) -> Response {
    if !supported_target(&platform, &arch) {
        return error_response(StatusCode::NOT_FOUND, "unsupported agent target");
    }

    let repository = match ReleaseRepository::from_environment() {
        Ok(repository) => repository,
        Err(error) => return repository_error_response(error),
    };
    match repository.latest_compatible_for_channel(
        &platform,
        &arch,
        protocol::PROTOCOL_VERSION,
        release::ReleaseChannel::Stable,
    ) {
        Ok((release, _)) => bytes_response(
            format!("{}\n", release.manifest.version).into_bytes(),
            "text/plain; charset=utf-8",
            "no-store",
            None,
        ),
        Err(error) => repository_error_response(error),
    }
}

/// GET /agent/v1/releases/:version/:platform/:arch/:file
///
/// `file` is deliberately an allow-list rather than a path. The repository
/// validates the complete release before any bytes are returned, and
/// `read_artifact` repeats the artifact verification after reading it.
pub async fn file(Path(path): Path<ReleasePath>) -> Response {
    if !supported_target(&path.platform, &path.arch) {
        return error_response(StatusCode::NOT_FOUND, "unsupported agent target");
    }

    let repository = match ReleaseRepository::from_environment() {
        Ok(repository) => repository,
        Err(error) => return repository_error_response(error),
    };
    let release = match repository.load(&path.version) {
        Ok(release) => release,
        Err(error) => return repository_error_response(error),
    };
    let artifact = match repository.artifact_for(
        &release,
        &path.platform,
        &path.arch,
        protocol::PROTOCOL_VERSION,
    ) {
        Ok(artifact) => artifact,
        Err(error) => return repository_error_response(error),
    };

    match path.file.as_str() {
        "manifest.json" => match repository.read_manifest_bundle(&release) {
            Ok((manifest, _)) => bytes_response(
                manifest,
                "application/json",
                "public, max-age=31536000, immutable",
                None,
            ),
            Err(error) => repository_error_response(error),
        },
        "manifest.json.sig" => match repository.read_manifest_bundle(&release) {
            Ok((_, signature)) => bytes_response(
                signature,
                "text/plain; charset=utf-8",
                "public, max-age=31536000, immutable",
                None,
            ),
            Err(error) => repository_error_response(error),
        },
        "agent" => match repository.read_artifact(&release, &artifact) {
            Ok(binary) => bytes_response(
                binary,
                "application/octet-stream",
                "public, max-age=31536000, immutable",
                Some(format!(
                    "attachment; filename=\"hope-agent-linux-{}\"",
                    path.arch
                )),
            ),
            Err(error) => repository_error_response(error),
        },
        _ => error_response(StatusCode::NOT_FOUND, "release file not found"),
    }
}

fn supported_target(platform: &str, arch: &str) -> bool {
    platform == SUPPORTED_PLATFORM && matches!(arch, "amd64" | "arm64")
}

fn bytes_response(
    bytes: Vec<u8>,
    content_type: &'static str,
    cache_control: &'static str,
    content_disposition: Option<String>,
) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(value) = content_disposition
        && let Ok(value) = HeaderValue::from_str(&value)
    {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn error_response(status: StatusCode, message: &'static str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

fn repository_error_response(error: RepositoryError) -> Response {
    let (status, message) = match error {
        RepositoryError::InvalidVersion => (StatusCode::BAD_REQUEST, "invalid release version"),
        RepositoryError::NotFound => (StatusCode::NOT_FOUND, "release not found"),
        RepositoryError::Incompatible => (StatusCode::NOT_FOUND, "no compatible release"),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "verified release repository is unavailable",
        ),
    };
    error_response(status, message)
}
