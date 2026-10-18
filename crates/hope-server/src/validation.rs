use crate::error::{ApiError, ApiResult};
use hope_protocol::{CollectorStatus, Inventory, Metrics};

pub fn collectors(value: &[CollectorStatus]) -> ApiResult<()> {
    hope_protocol::validation::collectors(value).map_err(ApiError::bad)
}
pub fn inventory(value: &Inventory) -> ApiResult<()> {
    hope_protocol::validation::inventory(value).map_err(ApiError::bad)
}
pub fn metrics(value: &Metrics) -> ApiResult<()> {
    hope_protocol::validation::metrics(value).map_err(ApiError::bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::Container;
    #[test]
    fn container_stats_must_be_finite_and_nonnegative() {
        let mut report = Metrics {
            containers: vec![Container {
                id: "id".into(),
                name: "web".into(),
                image: "web:latest".into(),
                state: "running".into(),
                status: "Up".into(),
                ports: String::new(),
                created_at: None,
                health: None,
                compose_project: None,
                compose_service: None,
                networks: vec![],
                published_ports: vec![],
                restart_count: None,
                cpu_percent: Some(1.5),
                memory_used_bytes: Some(10.0),
                memory_limit_bytes: Some(100.0),
                memory_percent: Some(10.0),
                network_rx_bytes: Some(1.0),
                network_tx_bytes: Some(2.0),
                block_read_bytes: Some(3.0),
                block_write_bytes: Some(4.0),
                pids: Some(1),
            }],
            ..Default::default()
        };
        assert!(metrics(&report).is_ok());
        report.containers.push(report.containers[0].clone());
        assert!(
            metrics(&report).is_err(),
            "duplicate container IDs cannot be aggregated independently"
        );
        report.containers.pop();
        report.containers[0].memory_used_bytes = Some(-1.0);
        assert!(metrics(&report).is_err());
    }

    #[test]
    fn missing_is_valid_but_invalid_numbers_are_rejected() {
        assert!(metrics(&Metrics::default()).is_ok());
        for cpu in [f64::NAN, f64::INFINITY, -1.0, 101.0] {
            assert!(
                metrics(&Metrics {
                    cpu_percent: Some(cpu),
                    ..Default::default()
                })
                .is_err()
            );
        }
        assert!(
            metrics(&Metrics {
                memory_used_bytes: Some(2.0),
                memory_total_bytes: Some(1.0),
                ..Default::default()
            })
            .is_err()
        );
    }
    #[test]
    fn extended_metrics_are_bounded_and_keyed_uniquely() {
        use hope_protocol::{
            DiskIo, LoadAverage, MonitorResult, ServiceSummary, StoragePool, Temperature,
        };
        let sensor = |sensor: &str, celsius| Temperature {
            sensor: sensor.into(),
            label: "Core".into(),
            celsius,
        };
        let valid = Metrics {
            load_average: Some(LoadAverage {
                one: 0.5,
                five: 0.4,
                fifteen: 0.3,
            }),
            temperatures: vec![sensor("a", 40.0), sensor("b", -20.0)],
            disk_io: vec![DiskIo {
                device: "nvme0n1".into(),
                read_bytes_per_second: Some(1.0),
                write_bytes_per_second: None,
                busy_percent: Some(100.0),
                root: true,
            }],
            pools: vec![StoragePool {
                name: "rpool".into(),
                kind: "zfs".into(),
                size_bytes: 10.0,
                allocated_bytes: 10.0,
                health: "ONLINE".into(),
                read_bytes_per_second: None,
                write_bytes_per_second: Some(0.0),
                root: true,
            }],
            service_summary: Some(ServiceSummary {
                active: 3,
                failed: 1,
                total: 4,
            }),
            monitors: vec![MonitorResult {
                target: "tcp://10.0.0.1:22".into(),
                kind: "tcp".into(),
                up: true,
                latency_ms: Some(3.5),
                status_code: None,
                detail: None,
            }],
            ..Default::default()
        };
        assert!(metrics(&valid).is_ok());
        let rejected = [
            Metrics {
                temperatures: vec![sensor("a", 40.0), sensor("a", 41.0)],
                ..Default::default()
            },
            Metrics {
                temperatures: vec![sensor("a", 400.0)],
                ..Default::default()
            },
            Metrics {
                load_average: Some(LoadAverage {
                    one: -1.0,
                    five: 0.0,
                    fifteen: 0.0,
                }),
                ..Default::default()
            },
            Metrics {
                disk_io: vec![DiskIo {
                    busy_percent: Some(101.0),
                    ..valid.disk_io[0].clone()
                }],
                ..Default::default()
            },
            Metrics {
                pools: vec![StoragePool {
                    allocated_bytes: 11.0,
                    ..valid.pools[0].clone()
                }],
                ..Default::default()
            },
            Metrics {
                service_summary: Some(ServiceSummary {
                    active: 5,
                    failed: 0,
                    total: 4,
                }),
                ..Default::default()
            },
            Metrics {
                monitors: vec![valid.monitors[0].clone(), valid.monitors[0].clone()],
                ..Default::default()
            },
            Metrics {
                swap_used_bytes: Some(2.0),
                swap_total_bytes: Some(1.0),
                ..Default::default()
            },
        ];
        for sample in rejected {
            assert!(metrics(&sample).is_err());
        }
    }
}
