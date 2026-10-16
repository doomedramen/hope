use crate::error::{ApiError, ApiResult};
use hope_protocol::{CollectorStatus, Disk, Gpu, Inventory, Metrics};

fn text(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.chars().any(char::is_control)
}
fn number(value: f64, maximum: f64) -> bool {
    value.is_finite() && value >= 0.0 && value <= maximum
}
fn optional(value: Option<f64>, maximum: f64) -> bool {
    value.is_none_or(|value| number(value, maximum))
}
fn disks(values: &[Disk]) -> bool {
    values.len() <= 128
        && values.iter().all(|disk| {
            text(&disk.name, 255)
                && text(&disk.mount, 4096)
                && text(&disk.filesystem, 128)
                && number(disk.total_bytes, 1e18)
                && number(disk.available_bytes, disk.total_bytes)
        })
}
fn gpus(values: &[Gpu]) -> bool {
    values.len() <= 32
        && values.iter().enumerate().all(|(index, gpu)| {
            gpu.id.is_none() || !values[..index].iter().any(|other| other.id == gpu.id)
        })
        && values.iter().all(|gpu| {
            gpu.id
                .as_ref()
                .is_none_or(|id| !id.is_empty() && text(id, 255))
                && text(&gpu.name, 255)
                && text(&gpu.vendor, 128)
                && optional(gpu.utilization_percent, 100.0)
                && optional(
                    gpu.memory_used_bytes,
                    gpu.memory_total_bytes.unwrap_or(1e18),
                )
                && optional(gpu.memory_total_bytes, 1e18)
                && gpu
                    .temperature_celsius
                    .is_none_or(|v| v.is_finite() && (-100.0..=300.0).contains(&v))
        })
}
pub fn collectors(values: &[CollectorStatus]) -> ApiResult<()> {
    if values.len() > 64
        || values
            .iter()
            .any(|c| !text(&c.name, 128) || c.detail.as_ref().is_some_and(|d| !text(d, 1024)))
    {
        return Err(ApiError::bad("Invalid collector availability"));
    }
    Ok(())
}
pub fn inventory(value: &Inventory) -> ApiResult<()> {
    if value.hostname.is_empty()
        || !text(&value.hostname, 255)
        || !text(&value.os, 255)
        || !text(&value.architecture, 64)
        || !text(&value.cpu_model, 255)
        || value.cpu_cores > 65536
        || !number(value.memory_total_bytes, 1e18)
        || !disks(&value.disks)
        || !gpus(&value.gpus)
        || value.interfaces.len() > 128
        || value.interfaces.iter().any(|i| {
            !text(&i.name, 128)
                || i.addresses.len() > 64
                || i.addresses
                    .iter()
                    .any(|a| a.parse::<std::net::IpAddr>().is_err())
                || i.gateway
                    .as_ref()
                    .is_some_and(|a| a.parse::<std::net::IpAddr>().is_err())
                || i.mac.as_ref().is_some_and(|m| {
                    m.len() != 17
                        || m.split(':').count() != 6
                        || m.split(':').any(|part| {
                            part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                })
        })
    {
        return Err(ApiError::bad(
            "Inventory exceeds supported limits or contains invalid values",
        ));
    }
    collectors(&value.collectors)
}
fn unique<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = std::collections::HashSet::new();
    names.into_iter().all(|name| seen.insert(name))
}
fn extended(value: &Metrics) -> Result<(), &'static str> {
    if !optional(value.memory_cached_bytes, 1e18)
        || !optional(value.memory_zfs_arc_bytes, 1e18)
        || !optional(value.swap_total_bytes, 1e18)
        || !optional(
            value.swap_used_bytes,
            value.swap_total_bytes.unwrap_or(1e18),
        )
    {
        return Err("Memory breakdown contains invalid values");
    }
    if value
        .load_average
        .is_some_and(|l| ![l.one, l.five, l.fifteen].iter().all(|v| number(*v, 1e6)))
    {
        return Err("Load average contains invalid values");
    }
    if value.temperatures.len() > 64
        || !unique(value.temperatures.iter().map(|t| t.sensor.as_str()))
        || value.temperatures.iter().any(|t| {
            t.sensor.is_empty()
                || !text(&t.sensor, 128)
                || !text(&t.label, 128)
                || !t.celsius.is_finite()
                || !(-100.0..=300.0).contains(&t.celsius)
        })
    {
        return Err("Invalid temperature readings");
    }
    if value.disk_io.len() > 64
        || !unique(value.disk_io.iter().map(|d| d.device.as_str()))
        || value.disk_io.iter().any(|d| {
            d.device.is_empty()
                || !text(&d.device, 128)
                || !optional(d.read_bytes_per_second, 1e15)
                || !optional(d.write_bytes_per_second, 1e15)
                || !optional(d.busy_percent, 100.0)
        })
    {
        return Err("Invalid device throughput");
    }
    if value.pools.len() > 64
        || !unique(value.pools.iter().map(|p| p.name.as_str()))
        || value.pools.iter().any(|p| {
            p.name.is_empty()
                || !text(&p.name, 128)
                || !text(&p.kind, 32)
                || !text(&p.health, 32)
                || !number(p.size_bytes, 1e20)
                || !number(p.allocated_bytes, p.size_bytes)
                || !optional(p.read_bytes_per_second, 1e15)
                || !optional(p.write_bytes_per_second, 1e15)
        })
    {
        return Err("Invalid storage pools");
    }
    if value.services.len() > 128
        || value.services.iter().any(|u| {
            u.name.is_empty()
                || !text(&u.name, 256)
                || !text(&u.state, 32)
                || !text(&u.sub_state, 32)
                || !text(&u.description, 256)
        })
        || value
            .service_summary
            .is_some_and(|s| s.total > 100_000 || s.active > s.total || s.failed > s.total)
    {
        return Err("Invalid service units");
    }
    if value.monitors.len() > 32
        || !unique(value.monitors.iter().map(|m| m.target.as_str()))
        || value.monitors.iter().any(|m| {
            m.target.is_empty()
                || !text(&m.target, 255)
                || !text(&m.kind, 16)
                || !optional(m.latency_ms, 1e9)
                || m.detail.as_ref().is_some_and(|d| !text(d, 256))
        })
    {
        return Err("Invalid monitor results");
    }
    Ok(())
}
pub fn metrics(value: &Metrics) -> ApiResult<()> {
    if let Err(message) = extended(value) {
        return Err(ApiError::bad(message));
    }
    if value.network_interfaces.len() > 128
        || value.network_interfaces.iter().any(|i| {
            !text(&i.name, 128)
                || !number(i.received_bytes, 1e20)
                || !number(i.transmitted_bytes, 1e20)
                || !optional(i.rx_bytes_per_second, 1e15)
                || !optional(i.tx_bytes_per_second, 1e15)
        })
    {
        return Err(ApiError::bad("Invalid interface counters"));
    }
    if !optional(value.cpu_percent, 100.0)
        || !optional(value.memory_total_bytes, 1e18)
        || !optional(
            value.memory_used_bytes,
            value.memory_total_bytes.unwrap_or(1e18),
        )
        || !optional(value.network_rx_bytes_per_second, 1e15)
        || !optional(value.network_tx_bytes_per_second, 1e15)
        || !optional(value.disk_read_bytes_per_second, 1e15)
        || !optional(value.disk_write_bytes_per_second, 1e15)
        || !disks(&value.disks)
        || !gpus(&value.gpus)
    {
        return Err(ApiError::bad("Metrics contain invalid values"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
