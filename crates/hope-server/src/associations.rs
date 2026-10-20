use hope_protocol::{ChildDeviceReport, Host, Network};
use ipnet::IpNet;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub host_id: String,
    pub host_name: String,
    pub strong: bool,
    pub reason: String,
    pub evidence: Vec<String>,
}

/// Match source interfaces to scan hosts. Only a unique MAC+address match within a known HOPE
/// network is strong enough to confirm automatically; every other overlap is review-only.
pub fn candidates(
    child: &ChildDeviceReport,
    hosts: &[Host],
    networks: &[Network],
) -> Vec<Candidate> {
    let child_ips: HashSet<_> = child
        .interfaces
        .iter()
        .flat_map(|interface| &interface.addresses)
        .filter_map(|address| crate::addresses::usable_ip(address))
        .collect();
    let child_macs: HashSet<_> = child
        .interfaces
        .iter()
        .filter_map(|interface| interface.mac.as_deref())
        .chain(
            child
                .network_configuration
                .iter()
                .filter_map(|network| network.mac.as_deref()),
        )
        .map(str::to_ascii_lowercase)
        .collect();
    if child_ips.is_empty() && child_macs.is_empty() {
        return vec![];
    }

    let networks: Vec<_> = networks
        .iter()
        .filter_map(|network| {
            network
                .cidr
                .parse::<IpNet>()
                .ok()
                .map(|cidr| (network.id.as_str(), cidr))
        })
        .collect();
    let child_networks: HashSet<_> = child_ips
        .iter()
        .filter_map(|ip| {
            networks
                .iter()
                .find_map(|(id, cidr)| cidr.contains(ip).then_some(*id))
        })
        .collect();
    let mac_owners = |mac: &str| {
        hosts
            .iter()
            .filter(|host| {
                host.macs
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(mac))
            })
            .count()
    };
    let ip_owners = |ip: &std::net::IpAddr| {
        hosts
            .iter()
            .filter(|host| {
                host.addresses
                    .iter()
                    .any(|known| crate::addresses::usable_ip(known).as_ref() == Some(ip))
            })
            .count()
    };
    let recent_after = crate::state::now() - 7.0 * 86400.0;
    let mut result = Vec::new();
    for host in hosts {
        let shared_macs: Vec<_> = child_macs
            .iter()
            .filter(|mac| {
                host.macs
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(mac))
            })
            .cloned()
            .collect();
        let shared_ips: Vec<_> = child_ips
            .iter()
            .filter(|ip| {
                host.addresses
                    .iter()
                    .any(|known| crate::addresses::usable_ip(known).as_ref() == Some(ip))
            })
            .collect();
        let hostname_match = child
            .name
            .trim_end_matches('.')
            .eq_ignore_ascii_case(host.name.trim_end_matches('.'));
        if shared_macs.is_empty() && shared_ips.is_empty() && !hostname_match {
            continue;
        }
        let host_networks: HashSet<_> = host
            .addresses
            .iter()
            .filter_map(|address| crate::addresses::usable_ip(address))
            .filter_map(|ip| {
                networks
                    .iter()
                    .find_map(|(id, cidr)| cidr.contains(&ip).then_some(*id))
            })
            .collect();
        let compatible_network = child_networks
            .iter()
            .any(|network| host_networks.contains(network));
        let unique_mac = shared_macs.iter().any(|mac| mac_owners(mac) == 1);
        let unique_ip = shared_ips.iter().any(|ip| ip_owners(ip) == 1);
        // Proxmox can report a configured guest NIC MAC without an in-guest address or
        // interface. A unique MAC then identifies the scanned host directly; when guest IPs
        // do provide managed-network context, still require that context to agree.
        let network_matches = child_networks.is_empty() || compatible_network;
        let strong = !shared_macs.is_empty()
            && network_matches
            && unique_mac
            && (shared_ips.is_empty() || unique_ip)
            && host.last_seen >= recent_after;
        let mut evidence = Vec::new();
        if !shared_macs.is_empty() {
            evidence.push(format!("MAC {}", shared_macs.join(", ")));
        }
        if !shared_ips.is_empty() {
            evidence.push(format!(
                "address {}",
                shared_ips
                    .iter()
                    .map(|ip| ip.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if hostname_match {
            evidence.push(format!("hostname {}", child.name));
        }
        if compatible_network {
            let mut common = child_networks
                .iter()
                .filter(|network| host_networks.contains(*network))
                .copied()
                .collect::<Vec<_>>();
            common.sort_unstable();
            evidence.push(format!("same managed network {}", common.join(", ")));
        } else if !child_networks.is_empty() {
            evidence.push("network context is missing or conflicts".into());
        }
        result.push(Candidate {
            host_id: host.id.clone(),
            host_name: host.name.clone(),
            strong,
            reason: if strong {
                "Unique interface, address, and managed-network match".into()
            } else {
                "Interface evidence needs review".into()
            },
            evidence,
        });
    }
    if result.iter().filter(|candidate| candidate.strong).count() != 1 {
        for candidate in &mut result {
            candidate.strong = false;
            candidate.reason = "Multiple or incomplete interface matches need review".into();
        }
    }
    result.sort_by(|left, right| left.host_name.cmp(&right.host_name));
    result.truncate(16);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::{
        ChildDeviceKind, DeviceLifecycle, GuestNetworkConfiguration, Interface, SourceIdentity,
    };

    fn child() -> ChildDeviceReport {
        ChildDeviceReport {
            identity: SourceIdentity {
                kind: ChildDeviceKind::ProxmoxVm,
                scope: "cluster".into(),
                key: "100".into(),
                incarnation: "first".into(),
            },
            name: "guest".into(),
            lifecycle: DeviceLifecycle::Running,
            status: None,
            health: None,
            restart_count: None,
            parent_key: None,
            node: None,
            ownership_generation: None,
            legacy_incarnation: None,
            tags: vec![],
            interfaces: vec![Interface {
                name: "net0".into(),
                mac: Some("02:00:00:00:00:01".into()),
                addresses: vec!["192.168.1.10".into()],
                gateway: None,
            }],
            cpu_cores: None,
            memory_bytes: None,
            image: None,
            compose_project: None,
            compose_service: None,
            published_ports: vec![],
            guest_agent_status: None,
            guest_os: None,
            guest_filesystems: vec![],
            network_configuration: vec![],
            disk_configuration: vec![],
            metrics: Default::default(),
        }
    }

    fn host(id: &str, address: &str, mac: &str) -> Host {
        let mut host = Host::new(id.into(), id.into(), "scan");
        host.addresses = vec![address.into()];
        host.macs = vec![mac.into()];
        host.last_seen = crate::state::now();
        host
    }

    fn network() -> Network {
        Network {
            id: "lan".into(),
            name: "LAN".into(),
            cidr: "192.168.1.0/24".into(),
            enabled: true,
            interval_minutes: 5,
        }
    }

    #[test]
    fn only_unique_mac_address_and_managed_network_evidence_auto_confirms() {
        let found = candidates(
            &child(),
            &[host("scan", "192.168.1.10", "02:00:00:00:00:01")],
            &[network()],
        );
        assert_eq!(found.len(), 1);
        assert!(found[0].strong);
    }

    #[test]
    fn duplicate_or_ip_only_matches_remain_suggestions() {
        let duplicate = candidates(
            &child(),
            &[
                host("scan-a", "192.168.1.10", "02:00:00:00:00:01"),
                host("scan-b", "192.168.1.11", "02:00:00:00:00:01"),
            ],
            &[network()],
        );
        assert!(duplicate.iter().all(|candidate| !candidate.strong));
        let ip_only = candidates(
            &child(),
            &[host("scan", "192.168.1.10", "02:00:00:00:00:02")],
            &[network()],
        );
        assert_eq!(ip_only.len(), 1);
        assert!(!ip_only[0].strong);
    }

    #[test]
    fn a_unique_mac_with_compatible_network_context_survives_a_dhcp_address_change() {
        let mut changed = child();
        changed.interfaces[0].addresses = vec!["192.168.1.50".into()];
        let found = candidates(
            &changed,
            &[host("scan", "192.168.1.55", "02:00:00:00:00:01")],
            &[network()],
        );
        assert_eq!(found.len(), 1);
        assert!(found[0].strong);
    }

    #[test]
    fn configured_guest_mac_without_guest_ip_can_match_uniquely_but_not_ambiguously() {
        let mut configured = child();
        configured.interfaces.clear();
        configured.network_configuration = vec![GuestNetworkConfiguration {
            name: "net0".into(),
            bridge: Some("vmbr0".into()),
            vlan_tag: None,
            model: Some("virtio".into()),
            mac: Some("02:00:00:00:00:01".into()),
        }];
        let unique = candidates(
            &configured,
            &[host("scan", "192.168.1.10", "02:00:00:00:00:01")],
            &[],
        );
        assert_eq!(unique.len(), 1);
        assert!(unique[0].strong);

        let ambiguous = candidates(
            &configured,
            &[
                host("scan-a", "192.168.1.10", "02:00:00:00:00:01"),
                host("scan-b", "192.168.1.11", "02:00:00:00:00:01"),
            ],
            &[],
        );
        assert_eq!(ambiguous.len(), 2);
        assert!(ambiguous.iter().all(|candidate| !candidate.strong));
    }
}
