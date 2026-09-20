//! Verified, filesystem-backed agent release repository (M7).
//!
//! Binaries are deliberately kept out of PostgreSQL. A release directory is
//! accepted only after the detached manifest signature, every artifact
//! signature, and every artifact's size/hash/platform/architecture check
//! succeeds. The worker reads the same verified bundle immediately before an
//! SSH update, so a file changed after discovery is rejected again.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const DEFAULT_ROOT: &str = "data/agent-releases";
pub const MAX_RELEASES: usize = 256;
pub const MAX_ARTIFACTS: usize = 16;
pub const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
pub const MAX_SIGNATURE_BYTES: u64 = 4 * 1024;
pub const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_VERSION_BYTES: usize = 128;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("agent release repository is not configured")]
    NotConfigured,
    #[error("agent release repository is unavailable")]
    Unavailable,
    #[error("no trusted release public key is configured")]
    NoTrustedKeys,
    #[error("too many trusted release public keys")]
    TooManyTrustedKeys { max: usize, actual: usize },
    #[error("invalid release public key")]
    InvalidPublicKey,
    #[error("invalid release version")]
    InvalidVersion,
    #[error("release not found")]
    NotFound,
    #[error("release manifest is too large")]
    ManifestTooLarge,
    #[error("release signature is too large")]
    SignatureTooLarge,
    #[error("release manifest is invalid: {0}")]
    Manifest(String),
    #[error("release manifest signature is invalid")]
    InvalidManifestSignature,
    #[error("release artifact is missing")]
    ArtifactMissing,
    #[error("release artifact is not a regular file")]
    ArtifactNotRegular,
    #[error("release artifact is too large")]
    ArtifactTooLarge,
    #[error("release artifact signature is invalid")]
    InvalidArtifactSignature,
    #[error("release artifact verification failed: {0}")]
    ArtifactVerification(String),
    #[error("release has no compatible artifact")]
    Incompatible,
    #[error("release file error")]
    File(#[source] std::io::Error),
    #[error("release JSON error")]
    Json(#[source] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, RepositoryError>;

#[derive(Debug, Clone)]
struct TrustedKey {
    fingerprint: String,
    key: VerifyingKey,
}

#[derive(Debug, Clone)]
pub struct ReleaseRepository {
    root: PathBuf,
    trusted_keys: Vec<TrustedKey>,
}

#[derive(Debug, Clone)]
pub struct VerifiedRelease {
    pub manifest: release::ManifestWithMetadata,
    pub manifest_sha256: String,
    pub manifest_signing_key_fingerprint: String,
    directory: PathBuf,
    artifacts: Vec<VerifiedArtifact>,
}

#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub record: release::ArtifactRecord,
    pub signature: String,
    pub signing_key_fingerprint: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseSummary {
    pub version: String,
    pub channel: release::ReleaseChannel,
    pub release_notes: Option<String>,
    pub manifest_sha256: String,
    pub signing_key_fingerprint: String,
    pub artifacts: Vec<ArtifactSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactSummary {
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
    pub min_protocol_version: u32,
    pub signing_key_fingerprint: String,
}

impl ReleaseRepository {
    pub fn from_environment() -> Result<Self> {
        let root =
            std::env::var("HOPE_AGENT_RELEASE_DIR").unwrap_or_else(|_| DEFAULT_ROOT.to_string());
        if root.trim().is_empty() || root.len() > 4096 {
            return Err(RepositoryError::NotConfigured);
        }

        let mut encoded_keys: Vec<String> = Vec::new();
        for name in ["HOPE_RELEASE_PUBLIC_KEYS", "HOPE_RELEASE_PUBLIC_KEY"] {
            if let Ok(value) = std::env::var(name) {
                encoded_keys.extend(
                    value
                        .split(|character: char| {
                            character == ',' || character.is_ascii_whitespace()
                        })
                        .map(ToOwned::to_owned),
                );
            }
        }
        for name in [
            "HOPE_RELEASE_PUBLIC_KEYS_FILE",
            "HOPE_RELEASE_PUBLIC_KEY_FILE",
        ] {
            if let Ok(path) = std::env::var(name) {
                let contents = fs::read_to_string(path).map_err(RepositoryError::File)?;
                encoded_keys.extend(
                    contents
                        .split(|character: char| {
                            character == ',' || character.is_ascii_whitespace()
                        })
                        .map(ToOwned::to_owned),
                );
            }
        }

        let keys = encoded_keys
            .iter()
            .filter(|value| !value.trim().is_empty())
            .map(|value| parse_trusted_key(value))
            .collect::<Result<Vec<_>>>()?;
        if keys.len() > release::MAX_TRUSTED_KEYS {
            return Err(RepositoryError::TooManyTrustedKeys {
                max: release::MAX_TRUSTED_KEYS,
                actual: keys.len(),
            });
        }
        Self::new(root, keys.into_iter().map(|key| key.key).collect())
    }

    pub fn new(root: impl Into<PathBuf>, keys: Vec<VerifyingKey>) -> Result<Self> {
        if keys.is_empty() {
            return Err(RepositoryError::NoTrustedKeys);
        }
        if keys.len() > release::MAX_TRUSTED_KEYS {
            return Err(RepositoryError::TooManyTrustedKeys {
                max: release::MAX_TRUSTED_KEYS,
                actual: keys.len(),
            });
        }
        let mut seen = BTreeSet::new();
        let trusted_keys = keys
            .into_iter()
            .filter_map(|key| {
                let fingerprint = key_fingerprint(&key);
                seen.insert(fingerprint.clone())
                    .then_some(TrustedKey { fingerprint, key })
            })
            .collect::<Vec<_>>();
        if trusted_keys.is_empty() {
            return Err(RepositoryError::NoTrustedKeys);
        }
        Ok(Self {
            root: root.into(),
            trusted_keys,
        })
    }

    pub fn list(&self) -> Result<Vec<VerifiedRelease>> {
        if !self.root.exists() {
            return Err(RepositoryError::Unavailable);
        }
        let mut directories = vec![self.root.clone()];
        let entries = fs::read_dir(&self.root).map_err(RepositoryError::File)?;
        for entry in entries {
            let entry = entry.map_err(RepositoryError::File)?;
            let file_type = entry.file_type().map_err(RepositoryError::File)?;
            if file_type.is_dir() && valid_component(&entry.file_name().to_string_lossy()) {
                directories.push(entry.path());
            }
        }
        if directories.len() > MAX_RELEASES + 1 {
            return Err(RepositoryError::Manifest(
                "release repository contains too many bundles".to_string(),
            ));
        }

        let mut releases = Vec::new();
        for directory in directories {
            if directory.join("manifest.json").exists() {
                releases.push(self.load_directory(&directory)?);
            }
        }
        releases.sort_by(|left, right| {
            compare_versions(&right.manifest.version, &left.manifest.version)
        });
        Ok(releases)
    }

    pub fn load(&self, version: &str) -> Result<VerifiedRelease> {
        validate_version(version)?;
        let root_manifest = self.root.join("manifest.json");
        if root_manifest.exists() {
            let release = self.load_directory(&self.root)?;
            if release.manifest.version == version {
                return Ok(release);
            }
        }

        let directory = self.root.join(version);
        let directory_type = fs::symlink_metadata(&directory).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RepositoryError::NotFound
            } else {
                RepositoryError::File(error)
            }
        })?;
        if !directory_type.file_type().is_dir() {
            return Err(RepositoryError::NotFound);
        }
        let release = self.load_directory(&directory)?;
        if release.manifest.version != version {
            return Err(RepositoryError::Manifest(
                "directory name does not match manifest version".to_string(),
            ));
        }
        Ok(release)
    }

    pub fn latest_compatible_for_channel(
        &self,
        platform: &str,
        arch: &str,
        protocol_version: u32,
        channel: release::ReleaseChannel,
    ) -> Result<(VerifiedRelease, VerifiedArtifact)> {
        for release in self.list()? {
            if release.manifest.channel != channel {
                continue;
            }
            if let Some(artifact) = compatible_artifact(&release, platform, arch, protocol_version)
            {
                return Ok((release, artifact));
            }
        }
        Err(RepositoryError::Incompatible)
    }

    pub fn artifact_for(
        &self,
        release: &VerifiedRelease,
        platform: &str,
        arch: &str,
        protocol_version: u32,
    ) -> Result<VerifiedArtifact> {
        compatible_artifact(release, platform, arch, protocol_version)
            .ok_or(RepositoryError::Incompatible)
    }

    pub fn read_artifact(
        &self,
        release: &VerifiedRelease,
        artifact: &VerifiedArtifact,
    ) -> Result<Vec<u8>> {
        if !artifact.path.starts_with(&release.directory) {
            return Err(RepositoryError::ArtifactMissing);
        }
        let binary = read_bounded(&artifact.path, MAX_AGENT_BINARY_BYTES)?;
        let key = self
            .trusted_keys
            .iter()
            .find(|trusted| trusted.fingerprint == artifact.signing_key_fingerprint)
            .ok_or(RepositoryError::InvalidArtifactSignature)?;
        release::verify_binary(
            &artifact.record,
            &artifact.signature,
            &key.key,
            &binary,
            &artifact.record.platform,
            &artifact.record.arch,
        )
        .map_err(|error| RepositoryError::ArtifactVerification(error.to_string()))?;
        Ok(binary)
    }

    /// Read the exact manifest bytes and detached signature that were
    /// verified when this release was loaded. Re-verify the files on read so
    /// a repository change between discovery and download cannot turn into a
    /// mismatched bundle response.
    pub fn read_manifest_bundle(&self, release: &VerifiedRelease) -> Result<(Vec<u8>, Vec<u8>)> {
        let manifest_path = release.directory.join("manifest.json");
        let signature_path = release.directory.join("manifest.json.sig");
        let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES).map_err(|error| {
            if matches!(&error, RepositoryError::ArtifactTooLarge) {
                RepositoryError::ManifestTooLarge
            } else {
                error
            }
        })?;
        if sha256(&manifest_bytes) != release.manifest_sha256 {
            return Err(RepositoryError::InvalidManifestSignature);
        }
        let signature_bytes =
            read_bounded(&signature_path, MAX_SIGNATURE_BYTES).map_err(|error| {
                if matches!(&error, RepositoryError::ArtifactTooLarge) {
                    RepositoryError::SignatureTooLarge
                } else {
                    error
                }
            })?;
        let signature = decode_signature(&signature_bytes)?;
        let key = self
            .trusted_keys
            .iter()
            .find(|trusted| trusted.fingerprint == release.manifest_signing_key_fingerprint)
            .ok_or(RepositoryError::InvalidManifestSignature)?;
        key.key
            .verify(&manifest_bytes, &signature)
            .map_err(|_| RepositoryError::InvalidManifestSignature)?;
        Ok((manifest_bytes, signature_bytes))
    }

    fn load_directory(&self, directory: &Path) -> Result<VerifiedRelease> {
        let manifest_path = directory.join("manifest.json");
        let manifest_bytes =
            read_bounded(&manifest_path, MAX_MANIFEST_BYTES).map_err(|error| match error {
                RepositoryError::ArtifactTooLarge => RepositoryError::ManifestTooLarge,
                other => other,
            })?;
        let manifest_signature =
            read_bounded(&directory.join("manifest.json.sig"), MAX_SIGNATURE_BYTES).map_err(
                |error| match error {
                    RepositoryError::ArtifactTooLarge => RepositoryError::SignatureTooLarge,
                    other => other,
                },
            )?;
        let manifest_signature = decode_signature(&manifest_signature)?;
        let manifest_signing_key = self
            .trusted_keys
            .iter()
            .find(|trusted| {
                trusted
                    .key
                    .verify(&manifest_bytes, &manifest_signature)
                    .is_ok()
            })
            .ok_or(RepositoryError::InvalidManifestSignature)?;
        let manifest: release::ManifestWithMetadata =
            serde_json::from_slice(&manifest_bytes).map_err(RepositoryError::Json)?;
        validate_version(&manifest.version)?;
        if manifest.artifacts.is_empty() || manifest.artifacts.len() > MAX_ARTIFACTS {
            return Err(RepositoryError::Manifest(
                "manifest artifact count is outside the supported bound".to_string(),
            ));
        }

        let mut seen_platform_arch = BTreeSet::new();
        let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
        for signed in &manifest.artifacts {
            if signed.record.version != manifest.version {
                return Err(RepositoryError::Manifest(
                    "artifact version does not match manifest version".to_string(),
                ));
            }
            if !valid_component(&signed.record.platform) || !valid_component(&signed.record.arch) {
                return Err(RepositoryError::Manifest(
                    "artifact platform or architecture is invalid".to_string(),
                ));
            }
            if !seen_platform_arch
                .insert((signed.record.platform.clone(), signed.record.arch.clone()))
            {
                return Err(RepositoryError::Manifest(
                    "manifest contains duplicate platform/architecture artifacts".to_string(),
                ));
            }
            let key = self
                .trusted_keys
                .iter()
                .find(|trusted| {
                    release::verify_record_signature(
                        &signed.record,
                        &signed.signature,
                        &trusted.key,
                    )
                    .is_ok()
                })
                .ok_or(RepositoryError::InvalidArtifactSignature)?;
            let artifact_path = directory.join(format!(
                "agent-{}-{}",
                signed.record.platform, signed.record.arch
            ));
            let binary = read_bounded(&artifact_path, MAX_AGENT_BINARY_BYTES)?;
            release::verify_binary(
                &signed.record,
                &signed.signature,
                &key.key,
                &binary,
                &signed.record.platform,
                &signed.record.arch,
            )
            .map_err(|error| RepositoryError::ArtifactVerification(error.to_string()))?;
            artifacts.push(VerifiedArtifact {
                record: signed.record.clone(),
                signature: signed.signature.clone(),
                signing_key_fingerprint: key.fingerprint.clone(),
                path: artifact_path,
            });
        }

        Ok(VerifiedRelease {
            manifest,
            manifest_sha256: sha256(&manifest_bytes),
            manifest_signing_key_fingerprint: manifest_signing_key.fingerprint.clone(),
            directory: directory.to_path_buf(),
            artifacts,
        })
    }
}

impl VerifiedRelease {
    pub fn summary(&self) -> ReleaseSummary {
        ReleaseSummary {
            version: self.manifest.version.clone(),
            channel: self.manifest.channel,
            release_notes: self.manifest.release_notes.clone(),
            manifest_sha256: self.manifest_sha256.clone(),
            signing_key_fingerprint: self.manifest_signing_key_fingerprint.clone(),
            artifacts: self
                .artifacts
                .iter()
                .map(|artifact| ArtifactSummary {
                    version: artifact.record.version.clone(),
                    platform: artifact.record.platform.clone(),
                    arch: artifact.record.arch.clone(),
                    size: artifact.record.size,
                    sha256: artifact.record.sha256.clone(),
                    signature: artifact.signature.clone(),
                    min_protocol_version: artifact.record.min_protocol_version,
                    signing_key_fingerprint: artifact.signing_key_fingerprint.clone(),
                })
                .collect(),
        }
    }
}

fn compatible_artifact(
    release: &VerifiedRelease,
    platform: &str,
    arch: &str,
    protocol_version: u32,
) -> Option<VerifiedArtifact> {
    release
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.record.platform == platform
                && artifact.record.arch == arch
                && artifact.record.min_protocol_version <= protocol_version
        })
        .cloned()
}

fn parse_trusted_key(encoded: &str) -> Result<TrustedKey> {
    let bytes: [u8; 32] = hex::decode(encoded.trim())
        .map_err(|_| RepositoryError::InvalidPublicKey)?
        .try_into()
        .map_err(|_| RepositoryError::InvalidPublicKey)?;
    let key = VerifyingKey::from_bytes(&bytes).map_err(|_| RepositoryError::InvalidPublicKey)?;
    Ok(TrustedKey {
        fingerprint: key_fingerprint(&key),
        key,
    })
}

fn decode_signature(bytes: &[u8]) -> Result<Signature> {
    let text = std::str::from_utf8(bytes).map_err(|_| RepositoryError::InvalidManifestSignature)?;
    let decoded: [u8; 64] = hex::decode(text.trim())
        .map_err(|_| RepositoryError::InvalidManifestSignature)?
        .try_into()
        .map_err(|_| RepositoryError::InvalidManifestSignature)?;
    Ok(Signature::from_bytes(&decoded))
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RepositoryError::ArtifactMissing
        } else {
            RepositoryError::File(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(RepositoryError::ArtifactNotRegular);
    }
    if metadata.len() > max_bytes {
        return Err(RepositoryError::ArtifactTooLarge);
    }
    let file = fs::File::open(path).map_err(RepositoryError::File)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(RepositoryError::File)?;
    if bytes.len() as u64 > max_bytes {
        return Err(RepositoryError::ArtifactTooLarge);
    }
    Ok(bytes)
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.len() <= MAX_VERSION_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_version(version: &str) -> Result<()> {
    if valid_component(version) {
        Ok(())
    } else {
        Err(RepositoryError::InvalidVersion)
    }
}

fn key_fingerprint(key: &VerifyingKey) -> String {
    let mut digest = Sha256::new();
    digest.update(key.to_bytes());
    hex::encode(digest.finalize())
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    match (Version::parse(left), Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        (Ok(_), Err(_)) => Ordering::Greater,
        (Err(_), Ok(_)) => Ordering::Less,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand_core::OsRng;
    use tempfile::tempdir;

    fn write_bundle(root: &Path, key: &SigningKey, version: &str, binary: &[u8]) {
        write_bundle_with_channel(root, key, version, binary, release::ReleaseChannel::Stable);
    }

    fn write_bundle_with_channel(
        root: &Path,
        key: &SigningKey,
        version: &str,
        binary: &[u8],
        channel: release::ReleaseChannel,
    ) {
        fs::create_dir_all(root).unwrap();
        let record = release::ArtifactRecord {
            version: version.to_string(),
            platform: "linux".to_string(),
            arch: "amd64".to_string(),
            size: binary.len() as u64,
            sha256: release::ArtifactRecord::sha256_of(binary),
            min_protocol_version: 1,
        };
        let signed = release::SignedArtifact {
            signature: release::sign_record(&record, key),
            record,
        };
        let manifest = release::Manifest {
            version: version.to_string(),
            artifacts: vec![signed],
        }
        .with_metadata(release::ManifestMetadata {
            channel,
            release_notes: None,
        });
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        fs::write(root.join("manifest.json"), &bytes).unwrap();
        fs::write(
            root.join("manifest.json.sig"),
            hex::encode(key.sign(&bytes).to_bytes()),
        )
        .unwrap();
        fs::write(root.join("agent-linux-amd64"), binary).unwrap();
    }

    #[test]
    fn verifies_bundle_and_rechecks_artifact_on_read() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        write_bundle(temp.path(), &key, "1.2.3", b"agent");
        let repo = ReleaseRepository::new(temp.path(), vec![key.verifying_key()]).unwrap();
        let release = repo.load("1.2.3").unwrap();
        let artifact = repo.artifact_for(&release, "linux", "amd64", 1).unwrap();
        assert_eq!(repo.read_artifact(&release, &artifact).unwrap(), b"agent");
        let (manifest, signature) = repo.read_manifest_bundle(&release).unwrap();
        assert_eq!(
            manifest,
            fs::read(temp.path().join("manifest.json")).unwrap()
        );
        assert_eq!(
            signature,
            fs::read(temp.path().join("manifest.json.sig")).unwrap()
        );

        fs::write(temp.path().join("manifest.json"), b"tampered").unwrap();
        assert!(matches!(
            repo.read_manifest_bundle(&release),
            Err(RepositoryError::InvalidManifestSignature)
        ));
    }

    #[test]
    fn tampered_manifest_and_artifact_are_rejected() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        write_bundle(temp.path(), &key, "1.2.3", b"agent");
        fs::write(temp.path().join("agent-linux-amd64"), b"tampered").unwrap();
        assert!(matches!(
            ReleaseRepository::new(temp.path(), vec![key.verifying_key()])
                .unwrap()
                .load("1.2.3"),
            Err(RepositoryError::ArtifactVerification(_))
        ));
    }

    #[test]
    fn key_rotation_accepts_any_trusted_key() {
        let temp = tempdir().unwrap();
        let old = SigningKey::generate(&mut OsRng);
        let new = SigningKey::generate(&mut OsRng);
        write_bundle(temp.path(), &new, "2.0.0", b"agent");
        let repo =
            ReleaseRepository::new(temp.path(), vec![old.verifying_key(), new.verifying_key()])
                .unwrap();
        assert_eq!(repo.load("2.0.0").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn incompatible_protocol_is_not_selected() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        write_bundle(temp.path(), &key, "1.2.3", b"agent");
        let release = ReleaseRepository::new(temp.path(), vec![key.verifying_key()])
            .unwrap()
            .load("1.2.3")
            .unwrap();
        assert!(matches!(
            ReleaseRepository::new(temp.path(), vec![key.verifying_key()])
                .unwrap()
                .artifact_for(&release, "linux", "amd64", 0),
            Err(RepositoryError::Incompatible)
        ));
    }

    #[test]
    fn traversal_versions_are_rejected() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let repository = ReleaseRepository::new(temp.path(), vec![key.verifying_key()]).unwrap();
        assert!(matches!(
            repository.load(".."),
            Err(RepositoryError::InvalidVersion)
        ));
    }

    #[test]
    fn trusted_key_count_is_bounded() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let keys = vec![key.verifying_key(); release::MAX_TRUSTED_KEYS + 1];
        assert!(matches!(
            ReleaseRepository::new(temp.path(), keys),
            Err(RepositoryError::TooManyTrustedKeys { .. })
        ));
    }

    #[test]
    fn latest_compatible_release_respects_channel() {
        let temp = tempdir().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        write_bundle_with_channel(
            &temp.path().join("stable"),
            &key,
            "1.0.0",
            b"stable",
            release::ReleaseChannel::Stable,
        );
        write_bundle_with_channel(
            &temp.path().join("canary"),
            &key,
            "2.0.0",
            b"canary",
            release::ReleaseChannel::Canary,
        );
        let repo = ReleaseRepository::new(temp.path(), vec![key.verifying_key()]).unwrap();
        let (release, artifact) = repo
            .latest_compatible_for_channel("linux", "amd64", 1, release::ReleaseChannel::Canary)
            .unwrap();
        assert_eq!(release.manifest.channel, release::ReleaseChannel::Canary);
        assert_eq!(release.manifest.version, "2.0.0");
        assert_eq!(repo.read_artifact(&release, &artifact).unwrap(), b"canary");
    }
}
