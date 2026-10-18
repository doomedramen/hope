//! Shared validation for agent telemetry and server ingestion.
use crate::{CollectorStatus, Disk, Gpu, Inventory, Metrics};

fn text(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.chars().any(char::is_control)
}
fn number(value: f64, maximum: f64) -> bool {
    value.is_finite() && value >= 0.0 && value <= maximum
}
fn optional(value: Option<f64>, maximum: f64) -> bool {
    value.is_none_or(|value| number(value, maximum))
}
pub fn disks(values: &[Disk]) -> bool {
    values.len() <= 128
        && values.iter().all(|disk| {
            text(&disk.name, 255)
                && text(&disk.mount, 4096)
                && text(&disk.filesystem, 128)
                && number(disk.total_bytes, 1e18)
                && number(disk.available_bytes, disk.total_bytes)
        })
}
pub fn gpus(values: &[Gpu]) -> bool {
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
pub fn collectors(values: &[CollectorStatus]) -> Result<(), &'static str> {
    if values.len() > 64
        || values
            .iter()
            .any(|c| !text(&c.name, 128) || c.detail.as_ref().is_some_and(|d| !text(d, 1024)))
    {
        return Err("Invalid collector availability");
    }
    Ok(())
}
pub fn inventory(value: &Inventory) -> Result<(), &'static str> {
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
        return Err("Inventory exceeds supported limits or contains invalid values");
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
    if value.containers.len() > 128
        || !unique(
            value
                .containers
                .iter()
                .map(|container| container.id.as_str()),
        )
        || value.containers.iter().any(|container| {
            container.id.is_empty()
                || !text(&container.id, 128)
                || container.name.is_empty()
                || !text(&container.name, 256)
                || container.image.is_empty()
                || !text(&container.image, 512)
                || !text(&container.state, 32)
                || !text(&container.status, 256)
                || !text(&container.ports, 512)
                || !optional(container.cpu_percent, 1e6)
                || !optional(container.memory_used_bytes, 1e18)
                || !optional(container.memory_limit_bytes, 1e18)
                || !optional(container.memory_percent, 1e6)
                || !optional(container.network_rx_bytes, 1e18)
                || !optional(container.network_tx_bytes, 1e18)
                || !optional(container.block_read_bytes, 1e18)
                || !optional(container.block_write_bytes, 1e18)
        })
    {
        return Err("Invalid containers");
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
pub fn metrics(value: &Metrics) -> Result<(), &'static str> {
    if let Some(uptime) = &value.system_uptime
        && (!number(uptime.seconds, 1e12)
            || !optional(uptime.boot_time, 1e12)
            || uptime
                .boot_id
                .as_ref()
                .is_some_and(|id| id.len() != 36 || uuid::Uuid::parse_str(id).is_err()))
    {
        return Err("Invalid system uptime");
    }
    extended(value)?;
    if value.network_interfaces.len() > 128
        || value.network_interfaces.iter().any(|i| {
            !text(&i.name, 128)
                || !number(i.received_bytes, 1e20)
                || !number(i.transmitted_bytes, 1e20)
                || !optional(i.rx_bytes_per_second, 1e15)
                || !optional(i.tx_bytes_per_second, 1e15)
        })
    {
        return Err("Invalid interface counters");
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
        return Err("Metrics contain invalid values");
    }
    Ok(())
}
