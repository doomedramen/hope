//! OS boot observations use the existing sysinfo collector, not report receipt
//! time or a reconstructed clock. Containers may expose their host's kernel.
use hope_protocol::{SystemUptime, UptimeScope};

pub async fn sample(container: bool) -> Option<SystemUptime> {
    #[cfg(target_os = "linux")]
    let boot_id =
        crate::config::read_bounded(std::path::Path::new("/proc/sys/kernel/random/boot_id"), 128)
            .await
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
    #[cfg(not(target_os = "linux"))]
    let boot_id: Option<String> = None;
    observation(
        sysinfo::System::uptime(),
        sysinfo::System::boot_time(),
        boot_id.as_deref(),
        container,
    )
}

fn observation(
    seconds: u64,
    boot_time: u64,
    boot_id: Option<&str>,
    container: bool,
) -> Option<SystemUptime> {
    // sysinfo returns zero if it cannot read uptime. Preserve unknown rather
    // than suggesting a reboot on every sample with unavailable counters.
    (seconds > 0).then(|| SystemUptime {
        seconds: seconds as f64,
        boot_time: (boot_time > 0).then_some(boot_time as f64),
        boot_id: boot_id
            .and_then(|id| uuid::Uuid::parse_str(id.trim()).ok())
            .map(|id| id.to_string()),
        scope: if container {
            UptimeScope::ContainerKernel
        } else {
            UptimeScope::System
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observations_keep_boot_identity_and_container_scope_without_fabricating_unknowns() {
        let id = uuid::Uuid::new_v4().to_string();
        let host = observation(172_801, 1_700_000_000, Some(&format!("{id}\n")), false)
            .expect("host uptime");
        assert_eq!(host.seconds, 172_801.0);
        assert_eq!(host.boot_time, Some(1_700_000_000.0));
        assert_eq!(host.boot_id.as_deref(), Some(id.as_str()));
        assert_eq!(host.scope, UptimeScope::System);
        let guest = observation(10, 0, Some("invalid"), true).expect("kernel uptime");
        assert_eq!(guest.scope, UptimeScope::ContainerKernel);
        assert!(guest.boot_time.is_none());
        assert!(guest.boot_id.is_none());
        assert!(observation(0, 1_700_000_000, Some(&id), false).is_none());
    }
}
