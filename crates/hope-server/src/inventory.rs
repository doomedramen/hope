//! Storage records and read-only inventory projection are deliberately different types.
use anyhow::{Context, Result, ensure};
use hope_protocol::{DeviceGuess, Host, UptimeSettings};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

/// A scan/agent record, never a projected source-device view.
///
/// ```compile_fail
/// # async fn invalid(state: &hope_server::state::State, view: hope_protocol::Host) {
/// state.save_host(&view).await.unwrap();
/// # }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoredHost(Host);

impl StoredHost {
    pub fn new(id: String, name: String, method: &str) -> Self {
        Self(Host::new(id, name, method))
    }
    pub(crate) fn decode(body: &str) -> Result<Self> {
        let record: Self = serde_json::from_str(body).context("invalid stored host")?;
        record.validate()?;
        Ok(record)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.source.is_none()
                && self.source_device_id.is_none()
                && self.source_observed_at.is_none()
                && self.child.is_none()
                && self.lifecycle.is_none()
                && self.associations.is_empty()
                && self.relationships.is_empty(),
            "a projected device cannot be saved as a host record"
        );
        Ok(())
    }
    pub(crate) fn into_view(self) -> Host {
        self.0
    }
}
impl Deref for StoredHost {
    type Target = Host;
    fn deref(&self) -> &Host {
        &self.0
    }
}
impl DerefMut for StoredHost {
    fn deref_mut(&mut self) -> &mut Host {
        &mut self.0
    }
}
impl std::borrow::Borrow<Host> for StoredHost {
    fn borrow(&self) -> &Host {
        &self.0
    }
}
impl std::borrow::BorrowMut<Host> for StoredHost {
    fn borrow_mut(&mut self) -> &mut Host {
        &mut self.0
    }
}

/// Source-only devices need settings, not an extra materialized host row.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct SourceSettings {
    pub name_override: Option<String>,
    pub icon_override: Option<String>,
    pub device: Option<DeviceGuess>,
    pub uptime: UptimeSettings,
}
impl SourceSettings {
    pub fn from_view(host: &Host) -> Self {
        Self {
            name_override: host.name_override.clone(),
            icon_override: host.icon_override.clone(),
            device: host.device.clone(),
            uptime: host.uptime.clone(),
        }
    }
    pub fn apply(self, host: &mut Host) {
        host.name_override = self.name_override;
        host.icon_override = self.icon_override;
        host.device = self.device;
        host.uptime = self.uptime;
    }
}
