use crate::{
    error::{ApiError, ApiResult},
    state::State,
};
use axum::{
    extract::{Request, State as Extract},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::path::Path;

/// Uninstall never downloads or executes a release binary, so it must remain available even
/// when no signed release is published. All service-manager and path handling stays shared.
pub async fn uninstaller() -> impl IntoResponse {
    (
        [
            ("content-type", "text/x-shellscript"),
            ("cache-control", "no-store"),
        ],
        include_str!("../../../scripts/install-agent.sh").replace("@@HOPE_PUBLIC_KEY@@", ""),
    )
}

pub async fn installer(Extract(state): Extract<State>) -> ApiResult<impl IntoResponse> {
    let directory = release_directory(&state.0.release_dir).await;
    let key = tokio::fs::read_to_string(directory.join("release-public.pem"))
        .await
    .map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "No signed release manifest is published. Publish a release with scripts/assemble-release.sh (local development: scripts/package-agent.sh).".into(),
        )
    })?;
    if !key.starts_with("-----BEGIN PUBLIC KEY-----") || key.len() > 16384 {
        return Err(ApiError::bad("Invalid release verification key"));
    }
    let script = include_str!("../../../scripts/install-agent.sh")
        .replace("@@HOPE_PUBLIC_KEY@@", key.trim());
    Ok((
        [
            ("content-type", "text/x-shellscript"),
            ("cache-control", "no-store"),
        ],
        script,
    ))
}

/// The directory holding the live release. With the `current` symlink layout written by
/// `assemble-release.sh` (`<root>/current -> <version>-<random>/`) it is resolved fresh on every call,
/// so a publish is seen without a restart and one request never mixes two releases. Without `current`
/// the configured directory is itself the release (tests and flat development trees).
pub async fn release_directory(root: &str) -> std::path::PathBuf {
    let root = Path::new(root);
    match tokio::fs::canonicalize(root.join("current")).await {
        Ok(path) if path.is_dir() => path,
        _ => root.to_path_buf(),
    }
}

/// Serves `/agent/releases/*` from the live release directory.
pub async fn serve(Extract(state): Extract<State>, request: Request) -> Response {
    use tower::ServiceExt;
    let directory = release_directory(&state.0.release_dir).await;
    match tower_http::services::ServeDir::new(directory)
        .oneshot(request)
        .await
    {
        Ok(response) => response.map(axum::body::Body::new),
        Err(never) => match never {},
    }
}

const PLATFORMS: [(&str, &str); 1] = [("linux", "x86_64")];
const MANIFEST_LIMIT: u64 = 64 * 1024;
const SIGNATURE_LIMIT: u64 = 16 * 1024;

#[derive(Deserialize)]
struct Manifest {
    format: u32,
    version: String,
    key_id: String,
    #[serde(default)]
    partial: bool,
    artifacts: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    platform: String,
    arch: String,
    file: String,
    sha256: String,
    size: u64,
    #[allow(dead_code)]
    min_os: String,
}

/// Cache policy for everything under `/agent/releases`. Names are not versioned, so artifacts must
/// be revalidated; the manifest and its signature decide which version is current and are never stored.
pub async fn cache_headers(request: Request, next: Next) -> Response {
    let metadata = request.uri().path().rsplit('/').next().is_some_and(|name| {
        matches!(
            name,
            "manifest.json" | "manifest.json.sig" | "release-public.pem"
        )
    });
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if metadata { "no-store" } else { "no-cache" }),
    );
    response
}

async fn is_file_within(path: &Path, minimum: u64, maximum: u64) -> Option<u64> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    (meta.is_file() && meta.len() >= minimum && meta.len() <= maximum).then_some(meta.len())
}

/// Platforms a *verified-format* manifest vouches for. `None` means there is no manifest at all
/// (legacy per-file layout); `Some(vec![])` means a manifest exists but is unusable, so nothing is
/// offered. The RSA signature is checked by the installer, not here; the server checks that the
/// manifest is well formed and that each listed file and signature exists with the listed size, and
/// never lists a platform the manifest does not contain.
async fn manifest_platforms(directory: &Path) -> Option<Vec<String>> {
    let path = directory.join("manifest.json");
    if tokio::fs::symlink_metadata(&path).await.is_err() {
        return None;
    }
    let usable = async {
        is_file_within(&path, 1, MANIFEST_LIMIT).await?;
        is_file_within(&directory.join("manifest.json.sig"), 1, SIGNATURE_LIMIT).await?;
        let manifest: Manifest =
            serde_json::from_slice(&tokio::fs::read(&path).await.ok()?).ok()?;
        let pem = tokio::fs::read_to_string(directory.join("release-public.pem"))
            .await
            .ok()?;
        if public_key_id(&pem)? != manifest.key_id.to_ascii_lowercase() {
            tracing::warn!("release manifest key_id does not match release-public.pem");
            return None;
        }
        let hex = |value: &str, length: usize| {
            value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
        };
        if manifest.format != 1
            || manifest.version.is_empty()
            || manifest.version.len() > 64
            || !hex(&manifest.key_id, 64)
        {
            return None;
        }
        let mut found: Vec<String> = vec![];
        for entry in &manifest.artifacts {
            let platform = format!("{}-{}", entry.platform, entry.arch);
            let known = PLATFORMS
                .iter()
                .any(|(os, arch)| *os == entry.platform && *arch == entry.arch);
            if !known
                || entry.file != format!("hope-agent-{platform}")
                || !hex(&entry.sha256, 64)
                || found.contains(&platform)
            {
                return None; // an unknown, mislabeled or duplicated entry invalidates the manifest
            }
            let actual = is_file_within(&directory.join(&entry.file), 1, u64::MAX).await?;
            if actual != entry.size {
                tracing::warn!(file = %entry.file, listed = entry.size, actual, "release artifact size differs from manifest");
                return None;
            }
            is_file_within(
                &directory.join(format!("{}.sig", entry.file)),
                1,
                SIGNATURE_LIMIT,
            )
            .await?;
            found.push(platform);
        }
        if !manifest.partial && found.len() != PLATFORMS.len() {
            return None; // a manifest claiming completeness must list every platform
        }
        Some(found)
    }
    .await;
    if usable.is_none() {
        tracing::warn!("release manifest.json is present but invalid; offering no agent platforms");
    }
    Some(usable.unwrap_or_default())
}

pub async fn packaged_platforms(directory: &Path) -> Vec<String> {
    if is_file_within(&directory.join("release-public.pem"), 1, 16384)
        .await
        .is_none()
    {
        return vec![];
    }
    // Only a manifest makes a platform available. Without one (including the pre-manifest per-file
    // layout) nothing is offered: the installer refuses such a release anyway, and the console must
    // not claim availability the installer will reject.
    manifest_platforms(directory).await.unwrap_or_default()
}

/// sha256 of the SubjectPublicKeyInfo DER, which is the body of a `PUBLIC KEY` PEM.
fn public_key_id(pem: &str) -> Option<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut lines = pem.trim().lines();
    if lines.next()?.trim() != "-----BEGIN PUBLIC KEY-----" {
        return None;
    }
    let mut body = String::new();
    for line in lines {
        if line.trim() == "-----END PUBLIC KEY-----" {
            let der = base64::engine::general_purpose::STANDARD
                .decode(body)
                .ok()?;
            return Some(
                Sha256::digest(der)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            );
        }
        body.push_str(line.trim());
    }
    None
}

/// Version of the release in `directory`, when it offers at least one platform.
pub async fn published_version(directory: &Path) -> Option<String> {
    if packaged_platforms(directory).await.is_empty() {
        return None;
    }
    let manifest: Manifest = serde_json::from_slice(
        &tokio::fs::read(directory.join("manifest.json"))
            .await
            .ok()?,
    )
    .ok()?;
    Some(manifest.version)
}

pub async fn availability(
    Extract(state): Extract<State>,
) -> ApiResult<axum::Json<hope_protocol::ReleaseAvailability>> {
    let directory = release_directory(&state.0.release_dir).await;
    Ok(axum::Json(hope_protocol::ReleaseAvailability {
        packaged_platforms: packaged_platforms(&directory).await,
        version: published_version(&directory).await,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEA2WA6+KWaU2QKPUoNvpE8
uWs6EqZAlw32jXt3QJ4WB3vQPZMmdrq8qLyt2HMnsCGJ4a4J0qCGZeuAp9GbxdQy
Nct276PRAZixbaOcEJfJuPKaLziaWsi/UUjHvEv9MoFC8kjYU5XTFcV3ZJeslmkK
gLFFmNAjA78DTKGOK8jE/3YS4fQOYgpmGOqzFEqcJroJzoCCsMfgWvAIiStS6pFL
j1O8SL4xOSjA6SMbjBBCbt8iifWHOoiHg/PvjywBr2UC4wfWKyP0Rp8IH9VRR52D
XvJ24CepV5rzEndRwLDyi7CF1ryFSB8hwiFWmUbGerk1d6YmWQN8t50hxTnhAYZ4
YYydjBsAO+Uv3g3hSeEsVKGEUJAFc1IeSXkUI+07zMi73yZCI4QC66auJI+pqptc
DRoUsHNM4hkMBCjhBKvyB1uo19I09MQ9e38VaL++yWp5cLNRiYv2T+2YKqw1kAnv
LrmW2OY0Z47dqhFZxgYSGmKSGPOX8uIferQ1dBjahj6ZAgMBAAE=
-----END PUBLIC KEY-----
";
    const KEY_ID: &str = "be0e49f69d223eabbd7a66c2d5cb91ce7ff9d504c1032b2f480ef85e238f5ae5";

    fn write(dir: &Path, name: &str, data: &[u8]) {
        std::fs::write(dir.join(name), data).expect("write");
    }

    /// Lay down a release the way assemble-release.sh does (signatures are placeholders: the server
    /// does not verify RSA).
    fn release(dir: &Path, platforms: &[&str], partial: bool) {
        write(dir, "release-public.pem", PEM.as_bytes());
        let mut rows = vec![];
        for platform in platforms {
            let name = format!("hope-agent-{platform}");
            write(dir, &name, b"binary");
            write(dir, &format!("{name}.sig"), b"sig");
            let (os, arch) = platform.split_once('-').expect("platform");
            rows.push(format!(
                r#"{{"platform":"{os}","arch":"{arch}","file":"{name}","sha256":"{}","size":6,"min_os":"x"}}"#,
                "a".repeat(64)
            ));
        }
        let manifest = format!(
            r#"{{"format":1,"version":"0.1.0","created":"2026-10-14T00:00:00Z","key_id":"{}","partial":{partial},"artifacts":[{}]}}"#,
            KEY_ID,
            rows.join(",")
        );
        write(dir, "manifest.json", manifest.as_bytes());
        write(dir, "manifest.json.sig", b"sig");
    }

    const ALL: [&str; 1] = ["linux-x86_64"];

    #[tokio::test]
    async fn manifest_lists_exactly_its_platforms() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &ALL, false);
        assert_eq!(packaged_platforms(dir.path()).await, ALL);
        let unsupported = tempfile::tempdir().expect("dir");
        release(unsupported.path(), &["macos-aarch64"], true);
        assert!(packaged_platforms(unsupported.path()).await.is_empty());
    }

    #[tokio::test]
    async fn files_absent_from_the_manifest_are_not_offered() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &["linux-x86_64"], true);
        // a stray, validly named artifact and signature that the manifest does not vouch for
        write(dir.path(), "hope-agent-macos-aarch64", b"binary");
        write(dir.path(), "hope-agent-macos-aarch64.sig", b"sig");
        assert_eq!(packaged_platforms(dir.path()).await, ["linux-x86_64"]);
    }

    #[tokio::test]
    async fn invalid_manifests_offer_nothing_and_never_fall_back() {
        type Damage = Box<dyn Fn(&Path)>;
        let broken: [(&str, Damage); 6] = [
            (
                "size mismatch",
                Box::new(|d| write(d, "hope-agent-linux-x86_64", b"longer binary")),
            ),
            (
                "missing signature",
                Box::new(|d| {
                    std::fs::remove_file(d.join("hope-agent-linux-x86_64.sig")).expect("rm")
                }),
            ),
            ("not json", Box::new(|d| write(d, "manifest.json", b"nope"))),
            (
                "unsupported format",
                Box::new(|d| {
                    let text = std::fs::read_to_string(d.join("manifest.json")).expect("read");
                    write(
                        d,
                        "manifest.json",
                        text.replace("\"format\":1", "\"format\":2").as_bytes(),
                    );
                }),
            ),
            (
                "mislabeled file",
                Box::new(|d| {
                    let text = std::fs::read_to_string(d.join("manifest.json")).expect("read");
                    write(
                        d,
                        "manifest.json",
                        text.replace(
                            "hope-agent-linux-x86_64\",\"sha",
                            "hope-agent-linux-aarch64\",\"sha",
                        )
                        .as_bytes(),
                    );
                }),
            ),
            (
                "missing manifest signature",
                Box::new(|d| std::fs::remove_file(d.join("manifest.json.sig")).expect("rm")),
            ),
        ];
        for (name, damage) in broken {
            let dir = tempfile::tempdir().expect("dir");
            release(dir.path(), &["linux-x86_64"], true);
            damage(dir.path());
            assert!(packaged_platforms(dir.path()).await.is_empty(), "{name}");
        }
    }

    #[tokio::test]
    async fn a_complete_manifest_must_list_the_supported_platform() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &[], false);
        assert!(packaged_platforms(dir.path()).await.is_empty());
    }

    #[tokio::test]
    async fn without_a_manifest_nothing_is_offered() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &["macos-aarch64"], true);
        std::fs::remove_file(dir.path().join("manifest.json")).expect("rm");
        assert!(packaged_platforms(dir.path()).await.is_empty());
    }

    #[tokio::test]
    async fn manifest_key_id_must_match_the_published_key() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &["linux-x86_64"], true);
        let text = std::fs::read_to_string(dir.path().join("manifest.json")).expect("read");
        write(
            dir.path(),
            "manifest.json",
            text.replace(KEY_ID, &"c".repeat(64)).as_bytes(),
        );
        assert!(packaged_platforms(dir.path()).await.is_empty());
        assert_eq!(public_key_id(PEM).as_deref(), Some(KEY_ID));
    }

    #[tokio::test]
    async fn current_symlink_is_followed_and_swaps_are_seen() {
        let root = tempfile::tempdir().expect("dir");
        for (name, platforms) in [
            ("1.0.0-aaaa", vec!["linux-x86_64"]),
            ("1.1.0-bbbb", vec!["linux-x86_64"]),
        ] {
            std::fs::create_dir(root.path().join(name)).expect("mkdir");
            let directory = root.path().join(name);
            release(&directory, &platforms, false);
            let manifest = std::fs::read_to_string(directory.join("manifest.json")).unwrap();
            write(
                &directory,
                "manifest.json",
                manifest
                    .replace("0.1.0", name.split('-').next().unwrap())
                    .as_bytes(),
            );
        }
        let current = root.path().join("current");
        std::os::unix::fs::symlink("1.0.0-aaaa", &current).expect("link");
        let live = release_directory(root.path().to_str().expect("path")).await;
        assert_eq!(packaged_platforms(&live).await, ["linux-x86_64"]);
        assert_eq!(published_version(&live).await.as_deref(), Some("1.0.0"));
        std::fs::remove_file(&current).expect("rm");
        std::os::unix::fs::symlink("1.1.0-bbbb", &current).expect("link");
        let live = release_directory(root.path().to_str().expect("path")).await;
        assert_eq!(packaged_platforms(&live).await, ["linux-x86_64"]);
        assert_eq!(published_version(&live).await.as_deref(), Some("1.1.0"));
        // No `current`: the directory itself is the release.
        std::fs::remove_file(&current).expect("rm");
        let flat = release_directory(root.path().to_str().expect("path")).await;
        assert_eq!(flat, root.path());
    }

    #[tokio::test]
    async fn no_public_key_means_nothing_is_available() {
        let dir = tempfile::tempdir().expect("dir");
        release(dir.path(), &ALL, false);
        std::fs::remove_file(dir.path().join("release-public.pem")).expect("rm");
        assert!(packaged_platforms(dir.path()).await.is_empty());
    }
}
