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
pub fn metrics(value: &Metrics) -> ApiResult<()> {
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
}
