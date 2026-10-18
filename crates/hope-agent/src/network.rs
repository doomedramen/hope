//! Use the same bounded interface selection for counters and inventory.
use hope_protocol::Interface;
use std::net::IpAddr;
use sysinfo::Networks;

const MAX_INTERFACES: usize = 128;
const MAX_ADDRESSES: usize = 64;

pub struct Selection {
    pub names: Vec<String>,
    pub truncated: bool,
}

pub fn select(networks: &Networks, gateway: Option<&str>) -> Selection {
    select_names(networks.keys().map(String::as_str), gateway)
}

fn select_names<'a>(names: impl Iterator<Item = &'a str>, gateway: Option<&str>) -> Selection {
    let mut names: Vec<_> = names
        .filter(|name| !matches!(*name, "lo" | "lo0"))
        .collect();
    // Hash-map iteration must not rotate a busy hypervisor's chosen interfaces.
    // Retain the total's default-route source even when there are many veths.
    names.sort_unstable_by_key(|name| (Some(*name) != gateway, virtual_interface(name), *name));
    let truncated = names.len() > MAX_INTERFACES;
    names.truncate(MAX_INTERFACES);
    Selection {
        names: names.into_iter().map(String::from).collect(),
        truncated,
    }
}

/// Fallback total policy when no default-route interface was found.
pub fn virtual_interface(name: &str) -> bool {
    [
        "veth", "docker", "br-", "bridge", "virbr", "tap", "tun", "utun", "wg", "bond",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn bounded_addresses(addresses: impl Iterator<Item = IpAddr>) -> (Vec<String>, bool) {
    // IPv6 link-local addresses need an interface zone, which this protocol
    // cannot represent. Preserve the previous collector's exclusion policy.
    let mut addresses: Vec<_> = addresses
        .filter(|ip| !matches!(ip, IpAddr::V6(ip) if ip.is_unicast_link_local()))
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    let truncated = addresses.len() > MAX_ADDRESSES;
    addresses.truncate(MAX_ADDRESSES);
    (
        addresses.into_iter().map(|ip| ip.to_string()).collect(),
        truncated,
    )
}

pub fn inventory(
    networks: &Networks,
    selected: &Selection,
    gateway: Option<&(String, String)>,
) -> (Vec<Interface>, bool) {
    let mut truncated = selected.truncated;
    let mut interfaces = vec![];
    for name in &selected.names {
        let Some(data) = networks.get(name) else {
            continue;
        };
        // Networks::refresh already refreshes these addresses on Linux/macOS.
        // Avoid a second getifaddrs enumeration and its separate snapshot.
        let (addresses, limited) = bounded_addresses(data.ip_networks().iter().map(|ip| ip.addr));
        truncated |= limited;
        interfaces.push(Interface {
            name: name.clone(),
            mac: (!data.mac_address().is_unspecified()).then(|| data.mac_address().to_string()),
            addresses,
            gateway: gateway
                .filter(|(interface, _)| interface == name)
                .map(|(_, ip)| ip.clone()),
        });
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    (interfaces, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_hosts_have_a_stable_selection_that_retains_the_default_route() {
        let mut names: Vec<_> = (0..300).map(|i| format!("veth{i:03}")).collect();
        names.extend(["lo".into(), "lo0".into(), "eth0".into(), "zz-route".into()]);
        let selection = select_names(names.iter().map(String::as_str), Some("zz-route"));
        assert!(selection.truncated);
        assert_eq!(selection.names.len(), MAX_INTERFACES);
        assert_eq!(&selection.names[..2], &["zz-route", "eth0"]);
        assert!(
            !selection
                .names
                .iter()
                .any(|name| name == "lo" || name == "lo0")
        );
        names.reverse();
        assert_eq!(
            selection.names,
            select_names(names.iter().map(String::as_str), Some("zz-route")).names
        );
        assert!(!select_names(["eth0", "lo"].into_iter(), None).truncated);
    }

    #[test]
    fn many_addresses_stay_within_the_wire_contract_and_ignore_duplicate_ordering() {
        let mut ips: Vec<IpAddr> = (1..=100)
            .map(|i| format!("2001:db8::{i:x}").parse().unwrap())
            .collect();
        ips.extend(ips.clone());
        let (addresses, truncated) = bounded_addresses(ips.iter().copied());
        assert!(truncated);
        assert_eq!(addresses.len(), MAX_ADDRESSES);
        ips.reverse();
        assert_eq!(addresses, bounded_addresses(ips.into_iter()).0);
        let inventory = hope_protocol::Inventory {
            hostname: "fixture".into(),
            os: "Linux".into(),
            architecture: "test".into(),
            cpu_model: "test".into(),
            cpu_cores: 1,
            memory_total_bytes: 1.0,
            interfaces: vec![Interface {
                name: "eth0".into(),
                mac: None,
                addresses,
                gateway: None,
            }],
            disks: vec![],
            gpus: vec![],
            collectors: vec![],
        };
        assert!(hope_protocol::validation::inventory(&inventory).is_ok());
        let (addresses, truncated) = bounded_addresses(
            ["fe80::1".parse().unwrap(), "192.0.2.1".parse().unwrap()].into_iter(),
        );
        assert_eq!(addresses, ["192.0.2.1"]);
        assert!(!truncated);
    }
}
