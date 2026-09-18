//! On-disk agent identity: private key, signed client cert, and the CA
//! cert used to verify the gateway's server cert. Written with 0600
//! permissions; never logged.

use std::path::{Path, PathBuf};

pub struct Paths {
    pub key: PathBuf,
    pub cert: PathBuf,
    pub ca: PathBuf,
}

impl Paths {
    pub fn new(state_dir: &str) -> Self {
        let dir = Path::new(state_dir);
        Self {
            key: dir.join("agent-key.pem"),
            cert: dir.join("agent-cert.pem"),
            ca: dir.join("ca-cert.pem"),
        }
    }

    pub fn exist(&self) -> bool {
        self.key.exists() && self.cert.exists() && self.ca.exists()
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
