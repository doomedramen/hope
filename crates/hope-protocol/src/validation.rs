//! Shared validation for agent telemetry and server ingestion.
use crate::{
    ChildDeviceKind, ChildReportBatch, ChildSourceKind, CollectorStatus, DeviceLifecycle, Disk,
    Gpu, Inventory, Metrics,
};

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

pub fn child_batch(value: &ChildReportBatch) -> Result<(), &'static str> {
    if value.schema_version != crate::SCHEMA_VERSION
        || uuid::Uuid::parse_str(&value.agent_id).is_err()
        || uuid::Uuid::parse_str(&value.batch_id).is_err()
        || value.source_scope.is_empty()
        || !text(&value.source_scope, 128)
        || value.source_token.as_ref().is_some_and(|token| {
            token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || value
            .legacy_source_scope
            .as_ref()
            .is_some_and(|scope| scope.is_empty() || !text(scope, 128))
        || value.legacy_identities.len() > 500
        || (value.legacy_source_scope.is_none() && !value.legacy_identities.is_empty())
        || value
            .proxmox_guest_presence
            .as_ref()
            .is_some_and(|members| {
                value.source != ChildSourceKind::Proxmox
                    || value.source_token.is_none()
                    || members.len() > 500
                    || {
                        let mut keys = std::collections::HashSet::new();
                        members.iter().any(|member| {
                            !matches!(
                                member.kind,
                                ChildDeviceKind::ProxmoxVm | ChildDeviceKind::ProxmoxLxc
                            ) || member.key.parse::<u64>().is_err()
                                || member.key.is_empty()
                                || !text(&member.key, 32)
                                || member.incarnation.as_ref().is_some_and(|incarnation| {
                                    incarnation.is_empty() || !text(incarnation, 128)
                                })
                                || !keys.insert((member.kind, member.key.as_str()))
                        })
                    }
            })
        || value.sequence == 0
        || value.sequence > i64::MAX as u64
        || value.inventory_generation == 0
        || value.inventory_generation > i64::MAX as u64
        || value.chunk_count == 0
        || value.chunk_count > 512
        || value.chunk_index >= value.chunk_count
        || value
            .inventory_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        || (value.inventory_id.is_none() && (value.chunk_index != 0 || value.chunk_count != 1))
        || !number(value.observed_at, 4_102_444_800.0)
        || value.devices.len() > 512
    {
        return Err("Invalid child inventory batch");
    }
    let mut identities = std::collections::HashSet::new();
    let mut source_keys = std::collections::HashSet::new();
    let mut legacy_keys = std::collections::HashSet::new();
    for legacy in &value.legacy_identities {
        if !matches!(
            legacy.kind,
            ChildDeviceKind::ProxmoxVm | ChildDeviceKind::ProxmoxLxc
        ) || legacy.key.is_empty()
            || !text(&legacy.key, 256)
            || legacy.incarnation.is_empty()
            || !text(&legacy.incarnation, 128)
            || legacy.canonical_incarnation.is_empty()
            || !text(&legacy.canonical_incarnation, 128)
            || !legacy_keys.insert((
                legacy.kind,
                legacy.key.as_str(),
                legacy.incarnation.as_str(),
            ))
        {
            return Err("Invalid legacy child identity mapping");
        }
    }
    for device in &value.devices {
        let identity = &device.identity;
        let kind_matches_source = matches!(
            (value.source, identity.kind),
            (ChildSourceKind::Docker, ChildDeviceKind::DockerContainer)
                | (ChildSourceKind::Proxmox, ChildDeviceKind::ProxmoxVm)
                | (ChildSourceKind::Proxmox, ChildDeviceKind::ProxmoxLxc)
                | (ChildSourceKind::Proxmox, ChildDeviceKind::DockerContainer)
        );
        let key = (
            &identity.kind,
            &identity.scope,
            &identity.key,
            &identity.incarnation,
        );
        if !kind_matches_source
            || identity.scope != value.source_scope
            || identity.key.is_empty()
            || !text(&identity.key, 256)
            || identity.incarnation.is_empty()
            || !text(&identity.incarnation, 128)
            || device
                .ownership_generation
                .is_some_and(|generation| generation > i64::MAX as u64)
            || device
                .legacy_incarnation
                .as_ref()
                .is_some_and(|incarnation| incarnation.is_empty() || !text(incarnation, 128))
            || (device.legacy_incarnation.is_some() && value.legacy_source_scope.is_none())
            || !identities.insert(key)
            || !source_keys.insert((identity.kind, identity.key.as_str()))
            || device.name.is_empty()
            || !text(&device.name, 256)
            || !matches!(
                device.lifecycle,
                DeviceLifecycle::Running | DeviceLifecycle::Stopped | DeviceLifecycle::Stale
            )
            || device
                .status
                .as_ref()
                .is_some_and(|value| !text(value, 256))
            || device.health.as_ref().is_some_and(|value| !text(value, 32))
            || device
                .restart_count
                .is_some_and(|value| value > 1_000_000_000)
            || device
                .parent_key
                .as_ref()
                .is_some_and(|text_value| text_value.is_empty() || !text(text_value, 256))
            || device
                .node
                .as_ref()
                .is_some_and(|text_value| !text(text_value, 128))
            || device.tags.len() > 64
            || device.tags.iter().any(|tag| !text(tag, 128))
            || device.interfaces.len() > 32
            || device.interfaces.iter().any(|interface| {
                !text(&interface.name, 128)
                    || interface.addresses.len() > 32
                    || interface
                        .addresses
                        .iter()
                        .any(|address| address.parse::<std::net::IpAddr>().is_err())
                    || interface
                        .gateway
                        .as_ref()
                        .is_some_and(|address| address.parse::<std::net::IpAddr>().is_err())
                    || interface.mac.as_ref().is_some_and(|mac| {
                        mac.len() != 17
                            || mac.split(':').count() != 6
                            || mac.split(':').any(|part| {
                                part.len() != 2
                                    || !part.bytes().all(|byte| byte.is_ascii_hexdigit())
                            })
                    })
            })
            || device.guest_os.as_ref().is_some_and(|os| {
                [
                    os.hostname.as_deref(),
                    os.name.as_deref(),
                    os.kernel_release.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|value| !text(value, 255))
                    || !number(os.observed_at, 4_102_444_800.0)
            })
            || device.guest_filesystems.len() > 128
            || device.guest_filesystems.iter().any(|filesystem| {
                !text(&filesystem.mountpoint, 4096)
                    || filesystem.mountpoint.is_empty()
                    || filesystem
                        .filesystem_type
                        .as_deref()
                        .is_some_and(|value| !text(value, 128))
                    || !number(filesystem.total_bytes, 1e18)
                    || !number(filesystem.used_bytes, filesystem.total_bytes)
                    || !number(filesystem.observed_at, 4_102_444_800.0)
            })
            || device.network_configuration.len() > 32
            || device.network_configuration.iter().any(|network| {
                !text(&network.name, 128)
                    || network.name.is_empty()
                    || network
                        .bridge
                        .as_deref()
                        .is_some_and(|value| !text(value, 128))
                    || network
                        .model
                        .as_deref()
                        .is_some_and(|value| !text(value, 64))
                    || network.mac.as_deref().is_some_and(|mac| {
                        mac.len() != 17
                            || mac.split(':').count() != 6
                            || mac.split(':').any(|part| {
                                part.len() != 2
                                    || !part.bytes().all(|byte| byte.is_ascii_hexdigit())
                            })
                    })
            })
            || device.disk_configuration.len() > 64
            || device.disk_configuration.iter().any(|disk| {
                !text(&disk.name, 128)
                    || disk.name.is_empty()
                    || disk
                        .storage
                        .as_deref()
                        .is_some_and(|value| !text(value, 128))
                    || disk
                        .volume
                        .as_deref()
                        .is_some_and(|value| !text(value, 255))
                    || disk.size_bytes.is_some_and(|value| !number(value, 1e18))
            })
            || !optional(device.cpu_cores, 65_536.0)
            || !optional(device.memory_bytes, 1e18)
            || device
                .image
                .as_ref()
                .is_some_and(|text_value| !text(text_value, 512))
            || device
                .compose_project
                .as_ref()
                .is_some_and(|text_value| !text(text_value, 128))
            || device
                .compose_service
                .as_ref()
                .is_some_and(|text_value| !text(text_value, 128))
            || device.published_ports.len() > 128
            || device.published_ports.iter().any(|port| {
                !matches!(port.protocol.as_str(), "tcp" | "udp" | "sctp")
                    || port
                        .host_ip
                        .as_ref()
                        .is_some_and(|address| address.parse::<std::net::IpAddr>().is_err())
            })
            || !optional(device.metrics.cpu_percent, 100_000.0)
            || !optional(device.metrics.cpu_capacity_cores, 65_536.0)
            || !optional(device.metrics.memory_used_bytes, 1e18)
            || !optional(device.metrics.memory_total_bytes, 1e18)
            || !optional(device.metrics.disk_used_bytes, 1e18)
            || !optional(device.metrics.disk_total_bytes, 1e18)
            || !optional(device.metrics.disk_read_bytes, 1e20)
            || !optional(device.metrics.disk_write_bytes, 1e20)
            || !optional(device.metrics.disk_read_bytes_per_second, 1e15)
            || !optional(device.metrics.disk_write_bytes_per_second, 1e15)
            || !optional(device.metrics.uptime_seconds, 1e12)
            || !optional(device.metrics.network_rx_bytes_per_second, 1e15)
            || !optional(device.metrics.network_tx_bytes_per_second, 1e15)
            || !optional(device.metrics.network_rx_bytes, 1e20)
            || !optional(device.metrics.network_tx_bytes, 1e20)
        {
            return Err("Invalid child device inventory");
        }
    }
    Ok(())
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
                || container.created_at.is_some_and(|value| {
                    !value.is_finite() || !(0.0..=4_102_444_800.0).contains(&value)
                })
                || container
                    .health
                    .as_ref()
                    .is_some_and(|value| !text(value, 32))
                || container
                    .compose_project
                    .as_ref()
                    .is_some_and(|value| !text(value, 128))
                || container
                    .compose_service
                    .as_ref()
                    .is_some_and(|value| !text(value, 128))
                || container.networks.len() > 16
                || container.networks.iter().any(|network| {
                    network.name.is_empty()
                        || !text(&network.name, 128)
                        || network
                            .network_id
                            .as_ref()
                            .is_some_and(|value| !text(value, 128))
                        || network.ipv4.as_ref().is_some_and(|value| !text(value, 64))
                        || network.ipv6.as_ref().is_some_and(|value| !text(value, 128))
                        || network.mac.as_ref().is_some_and(|value| !text(value, 32))
                        || network
                            .gateway
                            .as_ref()
                            .is_some_and(|value| value.parse::<std::net::IpAddr>().is_err())
                })
                || container.published_ports.len() > 64
                || container.published_ports.iter().any(|port| {
                    port.protocol.is_empty()
                        || !text(&port.protocol, 8)
                        || port.host_ip.as_ref().is_some_and(|value| !text(value, 64))
                })
                || container
                    .restart_count
                    .is_some_and(|value| value > 1_000_000_000)
                || !optional(container.cpu_percent, 1e6)
                || !optional(container.cpu_capacity_cores, 65_536.0)
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
