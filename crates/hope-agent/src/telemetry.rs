//! Isolate invalid optional collectors using the same contract as server ingestion.
use hope_protocol::{
    AgentReport, CollectorStatus, Inventory, MAX_AGENT_REPORT_BYTES, Metrics, validation,
};

/// Keep a large optional collection from causing HTTP 413 for the heartbeat.
pub fn bound_report(report: &mut AgentReport) -> anyhow::Result<()> {
    if serde_json::to_vec(report)?.len() <= MAX_AGENT_REPORT_BYTES {
        return Ok(());
    }
    macro_rules! omit {
        ($field:ident, $name:literal) => {
            if !report.metrics.$field.is_empty() {
                report.metrics.$field.clear();
                degraded(
                    &mut report.collectors,
                    $name,
                    "report exceeds the server payload budget",
                );
                if serde_json::to_vec(report)?.len() <= MAX_AGENT_REPORT_BYTES {
                    return Ok(());
                }
            }
        };
    }
    omit!(containers, "docker_containers");
    omit!(services, "services");
    omit!(monitors, "monitors");
    omit!(temperatures, "temperatures");
    omit!(disk_io, "disk_devices");
    omit!(pools, "storage_pools");
    omit!(gpus, "gpu_metrics");
    omit!(network_interfaces, "network");
    omit!(disks, "storage");
    if report.inventory.take().is_some() {
        degraded(
            &mut report.collectors,
            "inventory",
            "inventory exceeds the server payload budget",
        );
    }
    anyhow::ensure!(
        serde_json::to_vec(report)?.len() <= MAX_AGENT_REPORT_BYTES,
        "Core report exceeds the server payload budget"
    );
    Ok(())
}

fn degraded(collectors: &mut Vec<CollectorStatus>, name: &str, reason: &str) {
    if let Some(status) = collectors.iter_mut().find(|status| status.name == name) {
        status.available = false;
        status.detail = Some(format!("Invalid collector output omitted: {reason}"));
    } else {
        collectors.push(CollectorStatus {
            name: name.into(),
            available: false,
            detail: Some(format!("Invalid collector output omitted: {reason}")),
        });
    }
}

pub fn isolate(
    inventory: &mut Inventory,
    metrics: &mut Metrics,
    collectors: &mut Vec<CollectorStatus>,
) {
    // Validate each group on its own so one optional source cannot suppress the
    // heartbeat, CPU, memory, or unrelated collectors. Move values instead of cloning them.
    macro_rules! group {
        ($field:ident, $name:literal) => {{
            let mut probe = Metrics {
                $field: std::mem::take(&mut metrics.$field),
                ..Default::default()
            };
            match validation::metrics(&probe) {
                Ok(()) => metrics.$field = std::mem::take(&mut probe.$field),
                Err(reason) => degraded(collectors, $name, reason),
            }
        }};
    }
    group!(containers, "docker_containers");
    group!(monitors, "monitors");
    group!(disks, "storage");
    group!(gpus, "gpu_metrics");
    group!(disk_io, "disk_devices");
    group!(pools, "storage_pools");
    group!(temperatures, "temperatures");
    group!(network_interfaces, "network");
    group!(system_uptime, "uptime");

    let mut services = Metrics {
        services: std::mem::take(&mut metrics.services),
        service_summary: metrics.service_summary.take(),
        ..Default::default()
    };
    match validation::metrics(&services) {
        Ok(()) => {
            metrics.services = std::mem::take(&mut services.services);
            metrics.service_summary = services.service_summary;
        }
        Err(reason) => degraded(collectors, "services", reason),
    }
    if !validation::disks(&inventory.disks) {
        inventory.disks.clear();
        degraded(collectors, "storage", "invalid filesystem inventory");
    }
    if !validation::gpus(&inventory.gpus) {
        inventory.gpus.clear();
        degraded(collectors, "gpu_inventory", "invalid GPU inventory");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::{Container, Disk, Gpu, SCHEMA_VERSION};

    #[test]
    fn an_oversized_optional_collection_preserves_the_heartbeat() {
        let container = Container {
            id: "a".repeat(128),
            name: "n".repeat(256),
            image: "i".repeat(512),
            state: "running".into(),
            status: "s".repeat(256),
            ports: "p".repeat(512),
            cpu_percent: None,
            memory_used_bytes: None,
            memory_limit_bytes: None,
            memory_percent: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            block_read_bytes: None,
            block_write_bytes: None,
            pids: None,
        };
        let mut report = AgentReport {
            schema_version: SCHEMA_VERSION,
            report_id: uuid::Uuid::new_v4().to_string(),
            agent_id: uuid::Uuid::new_v4().to_string(),
            collected_at: 1.0,
            inventory: None,
            collectors: vec![],
            metrics: Metrics {
                cpu_percent: Some(42.0),
                containers: (0..128)
                    .map(|index| hope_protocol::Container {
                        id: format!("{index:0128x}"),
                        ..container.clone()
                    })
                    .collect(),
                disks: vec![
                    Disk {
                        name: "disk".into(),
                        mount: format!("/{}", "d".repeat(4095)),
                        filesystem: "ext4".into(),
                        total_bytes: 100.0,
                        available_bytes: 50.0,
                    };
                    16
                ],
                ..Default::default()
            },
        };
        assert!(validation::metrics(&report.metrics).is_ok());
        assert!(serde_json::to_vec(&report).unwrap().len() > MAX_AGENT_REPORT_BYTES);
        bound_report(&mut report).unwrap();
        assert!(serde_json::to_vec(&report).unwrap().len() <= MAX_AGENT_REPORT_BYTES);
        assert_eq!(report.metrics.cpu_percent, Some(42.0));
        assert_eq!(report.metrics.disks.len(), 16);
        assert!(report.metrics.containers.is_empty());
        assert!(
            report
                .collectors
                .iter()
                .any(|status| status.name == "docker_containers" && !status.available)
        );
    }

    #[test]
    fn malformed_optional_data_preserves_core_metrics_and_reports_coverage_loss() {
        let mut inventory = Inventory {
            hostname: "host".into(),
            os: "test".into(),
            architecture: "test".into(),
            cpu_model: "test".into(),
            cpu_cores: 1,
            memory_total_bytes: 100.0,
            interfaces: vec![],
            disks: vec![],
            gpus: vec![],
            collectors: vec![],
        };
        let mut metrics = Metrics {
            cpu_percent: Some(42.0),
            system_uptime: Some(hope_protocol::SystemUptime {
                seconds: -1.0,
                boot_time: None,
                boot_id: None,
                scope: hope_protocol::UptimeScope::System,
            }),
            memory_total_bytes: Some(100.0),
            memory_used_bytes: Some(50.0),
            disks: vec![Disk {
                name: "disk".into(),
                mount: "/".into(),
                filesystem: "ext4".into(),
                total_bytes: 100.0,
                available_bytes: 50.0,
            }],
            gpus: vec![Gpu {
                id: Some("gpu".into()),
                name: "GPU".into(),
                vendor: "test".into(),
                utilization_percent: Some(200.0),
                memory_used_bytes: None,
                memory_total_bytes: None,
                temperature_celsius: None,
            }],
            ..Default::default()
        };
        let mut collectors = vec![];
        isolate(&mut inventory, &mut metrics, &mut collectors);
        assert!(validation::metrics(&metrics).is_ok());
        assert_eq!(metrics.cpu_percent, Some(42.0));
        assert!(metrics.system_uptime.is_none());
        assert_eq!(metrics.memory_used_bytes, Some(50.0));
        assert_eq!(metrics.disks.len(), 1);
        assert!(metrics.gpus.is_empty());
        assert!(
            collectors
                .iter()
                .any(|status| status.name == "gpu_metrics" && !status.available)
        );
        assert!(
            collectors
                .iter()
                .any(|status| status.name == "uptime" && !status.available)
        );
    }
}
