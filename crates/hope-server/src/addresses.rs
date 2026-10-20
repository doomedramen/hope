use hope_protocol::Network;
use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr};

/// Parse an address only when it is suitable as a device address.
///
/// Inventory can include loopback and link-local interface addresses. Keep those observations
/// available where useful, but never let them become a preferred network address.
pub fn usable_ip(address: &str) -> Option<IpAddr> {
    let ip = address.parse::<IpAddr>().ok()?;
    let link_local = match ip {
        IpAddr::V4(address) => address.is_link_local(),
        IpAddr::V6(address) => address.segments()[0] & 0xffc0 == 0xfe80,
    };
    let usable = !ip.is_loopback()
        && !ip.is_unspecified()
        && !link_local
        && !ip.is_multicast()
        && ip != IpAddr::V4(Ipv4Addr::BROADCAST);
    usable.then_some(ip)
}

/// Return the first usable observed address that belongs to a configured HOPE network.
///
/// The address ordering is intentional: callers can retain the collector's preferred interface
/// order, while invalid and unmanaged addresses can never become an implicit SSH target.
pub fn primary_ip(addresses: &[String], networks: &[Network]) -> Option<String> {
    let cidrs: Vec<IpNet> = networks
        .iter()
        .filter(|network| network.enabled)
        .filter_map(|network| network.cidr.parse().ok())
        .collect();
    addresses.iter().find_map(|address| {
        let ip = usable_ip(address)?;
        cidrs
            .iter()
            .any(|cidr| cidr.contains(&ip))
            .then(|| address.clone())
    })
}

/// Whether a valid address belongs to at least one configured network.
pub fn is_managed_ip(address: &str, networks: &[Network]) -> bool {
    let Some(ip) = usable_ip(address) else {
        return false;
    };
    networks
        .iter()
        .filter(|network| network.enabled)
        .any(|network| {
            network
                .cidr
                .parse::<IpNet>()
                .is_ok_and(|cidr| cidr.contains(&ip))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(id: &str, cidr: &str) -> Network {
        Network {
            id: id.into(),
            name: id.into(),
            cidr: cidr.into(),
            enabled: true,
            interval_minutes: 5,
        }
    }

    #[test]
    fn chooses_first_usable_address_in_any_managed_cidr() {
        let networks = vec![
            network("office", "192.168.40.0/24"),
            network("lab", "10.20.0.0/16"),
        ];
        let addresses = vec![
            "127.0.0.1".into(),
            "169.254.1.9".into(),
            "192.168.41.10".into(),
            "10.20.3.8".into(),
        ];
        assert_eq!(
            primary_ip(&addresses, &networks).as_deref(),
            Some("10.20.3.8")
        );
    }

    #[test]
    fn does_not_choose_loopback_or_unmanaged_addresses() {
        let networks = vec![network("office", "192.168.40.0/24")];
        let addresses = vec![
            "127.0.0.1".into(),
            "169.254.1.9".into(),
            "192.168.41.10".into(),
        ];
        assert_eq!(primary_ip(&addresses, &networks), None);
        assert!(!is_managed_ip("127.0.0.1", &networks));
        assert!(!is_managed_ip("169.254.1.9", &networks));
    }

    #[test]
    fn ignores_disabled_networks_and_ipv6_link_local_addresses() {
        let mut disabled = network("old", "2001:db8:1::/64");
        disabled.enabled = false;
        let enabled = network("v6", "2001:db8:2::/64");
        let addresses = vec![
            "fe80::1".into(),
            "2001:db8:1::4".into(),
            "2001:db8:2::5".into(),
        ];
        assert_eq!(
            primary_ip(&addresses, &[disabled, enabled]).as_deref(),
            Some("2001:db8:2::5")
        );
    }
}
