//! Developer/CI tooling for the signed agent release pipeline (spec
//! §7.5/§7.7). Not part of the shipped product — run via `cargo xtask ...`
//! or the `just release-*` recipes.

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use release::{ArtifactRecord, Manifest, ManifestMetadata, ReleaseChannel, SignedArtifact};

#[derive(Parser)]
#[command(name = "xtask", about = "hope release tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a new Ed25519 release-signing keypair. Never overwrites an
    /// existing key. The private key file is 0600 and MUST NOT be
    /// committed (the repo's .gitignore excludes *.key, /data/, /dist/,
    /// but a custom --out path is the caller's responsibility).
    Keygen {
        /// Directory to write `signing-key.hex` (private) and
        /// `public-key.hex` (public) into.
        #[arg(long, default_value = "data/release-keys")]
        out: PathBuf,
    },
    /// Sign one or more agent binaries into a manifest.json +
    /// manifest.json.sig + SHA256SUMS release bundle.
    Sign {
        /// Release version, e.g. 0.1.0.
        #[arg(long)]
        version: String,
        /// Release channel. Older manifests default to stable.
        #[arg(long, default_value = "stable", value_parser = parse_release_channel)]
        channel: ReleaseChannel,
        /// Optional release notes, bounded to the shared manifest limit.
        #[arg(long, alias = "notes")]
        release_notes: Option<String>,
        #[arg(long, default_value_t = 1)]
        min_protocol_version: u32,
        /// Path to the signing-key.hex private key file (from `keygen`).
        #[arg(long)]
        key: PathBuf,
        /// Output directory for manifest.json, manifest.json.sig, and
        /// SHA256SUMS.
        #[arg(long, default_value = "dist/release")]
        out: PathBuf,
        /// One or more PATH=PLATFORM=ARCH artifacts, e.g.
        /// target/x86_64-unknown-linux-musl/release/agent=linux=amd64
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::Sign {
            version,
            channel,
            release_notes,
            min_protocol_version,
            key,
            out,
            artifacts,
        } => sign(
            &version,
            channel,
            release_notes,
            min_protocol_version,
            &key,
            &out,
            &artifacts,
        ),
    }
}

fn keygen(out: &Path) -> anyhow::Result<()> {
    let signing_key_path = out.join("signing-key.hex");
    let public_key_path = out.join("public-key.hex");

    if signing_key_path.exists() || public_key_path.exists() {
        anyhow::bail!(
            "refusing to overwrite existing key material in {}",
            out.display()
        );
    }

    fs::create_dir_all(out)?;

    let signing_key = SigningKey::generate(&mut rand_core::OsRng);
    let verifying_key = signing_key.verifying_key();

    write_private(&signing_key_path, &hex::encode(signing_key.to_bytes()))?;
    fs::write(&public_key_path, hex::encode(verifying_key.to_bytes()))?;

    println!("wrote {}", signing_key_path.display());
    println!("wrote {}", public_key_path.display());
    println!("public key: {}", hex::encode(verifying_key.to_bytes()));
    println!(
        "NEVER commit {} — it is gitignored by default under data/, but double check.",
        signing_key_path.display()
    );

    Ok(())
}

fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn sign(
    version: &str,
    channel: ReleaseChannel,
    release_notes: Option<String>,
    min_protocol_version: u32,
    key_path: &Path,
    out: &Path,
    artifact_specs: &[String],
) -> anyhow::Result<()> {
    let metadata = ManifestMetadata {
        channel,
        release_notes,
    };
    metadata.validate()?;

    let key_hex = fs::read_to_string(key_path)?;
    let key_bytes: [u8; 32] = hex::decode(key_hex.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must decode to 32 bytes"))?;
    let signing_key = SigningKey::from_bytes(&key_bytes);
    let verifying_key = signing_key.verifying_key();

    fs::create_dir_all(out)?;

    let mut artifacts = Vec::new();
    let mut sha256sums = String::new();

    for spec in artifact_specs {
        let parts: Vec<&str> = spec.splitn(3, '=').collect();
        let [path, platform, arch] = parts.as_slice() else {
            anyhow::bail!("invalid --artifact {spec:?}: expected PATH=PLATFORM=ARCH");
        };

        let binary = fs::read(path)?;
        let sha256 = ArtifactRecord::sha256_of(&binary);
        let record = ArtifactRecord {
            version: version.to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            size: binary.len() as u64,
            sha256: sha256.clone(),
            min_protocol_version,
        };

        let signature = release::sign_record(&record, &signing_key);

        // Self-check: fail loudly at sign time, not at first verify time,
        // if something about the record/signature is inconsistent.
        release::verify_binary(&record, &signature, &verifying_key, &binary, platform, arch)
            .map_err(|err| anyhow::anyhow!("self-verification of {spec} failed: {err}"))?;

        let filename = Path::new(path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("agent-{platform}-{arch}"));
        sha256sums.push_str(&format!("{sha256}  {filename}\n"));

        let dest = out.join(format!("agent-{platform}-{arch}"));
        fs::copy(path, &dest)?;

        println!("signed {spec} ({} bytes, sha256 {sha256})", record.size);
        artifacts.push(SignedArtifact { record, signature });
    }

    let manifest = Manifest {
        version: version.to_string(),
        artifacts,
    }
    .with_metadata(metadata);
    manifest.validate()?;
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;

    let manifest_path = out.join("manifest.json");
    let manifest_signature = release::sign_manifest(&manifest_json, &signing_key);
    release::verify_manifest_with_metadata(
        &manifest_json,
        &manifest_signature,
        std::slice::from_ref(&verifying_key),
    )
    .map_err(|error| anyhow::anyhow!("self-verification of manifest failed: {error}"))?;

    fs::write(&manifest_path, &manifest_json)?;
    fs::write(out.join("manifest.json.sig"), manifest_signature)?;

    fs::write(out.join("SHA256SUMS"), sha256sums)?;

    println!("wrote {}", manifest_path.display());
    println!(
        "public key for verification: {}",
        hex::encode(verifying_key.to_bytes())
    );

    Ok(())
}

fn parse_release_channel(value: &str) -> Result<ReleaseChannel, String> {
    value
        .parse()
        .map_err(|error: &'static str| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_release_channel;
    use release::ReleaseChannel;

    #[test]
    fn artifact_spec_parses_path_platform_arch() {
        let spec = "target/release/agent=linux=amd64";
        let parts: Vec<&str> = spec.splitn(3, '=').collect();
        assert_eq!(parts, vec!["target/release/agent", "linux", "amd64"]);
    }

    #[test]
    fn release_channel_parser_accepts_only_stable_or_canary() {
        assert_eq!(parse_release_channel("stable"), Ok(ReleaseChannel::Stable));
        assert_eq!(parse_release_channel("canary"), Ok(ReleaseChannel::Canary));
        assert!(parse_release_channel("beta").is_err());
    }
}
