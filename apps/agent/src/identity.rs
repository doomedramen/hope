//! On-disk agent identity and TLS trust, written with 0600 permissions.

use std::path::{Path, PathBuf};

pub struct Paths {
    pub identity_key: PathBuf,
    pub agent_id: PathBuf,
    pub tls_trust: PathBuf,
}

impl Paths {
    pub fn new(state_dir: &str) -> Self {
        let dir = Path::new(state_dir);
        Self {
            identity_key: dir.join("agent-identity.hex"),
            agent_id: dir.join("agent-id"),
            tls_trust: dir.join("tls-trust"),
        }
    }

    pub fn exist(&self) -> bool {
        self.identity_key.exists() && self.agent_id.exists() && self.tls_trust.exists()
    }
}

pub fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
