//! Network hierarchy from layer 3: which router each range sits behind and how routers chain.
//!
//! - Directly attached ranges sit behind the server's default gateway when it lies inside them.
//! - Routed ranges are traced (ICMP echo with increasing TTL); the hop before the destination is
//!   their gateway and earlier hops form the chain up to the server.
//!
//! Switches and access points forward frames without appearing in either, so which port or
//! access point a device hangs off needs the switches' own tables (SNMP/LLDP) and is not inferred.
use crate::state::{State, now};
use hope_protocol::{Host, Network};
use ipnet::Ipv4Net;
use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    time::Duration,
};

/// Routes from `/proc/net/route`: the default gateway and directly attached networks.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Routes {
    pub gateway: Option<Ipv4Addr>,
    pub attached: Vec<Ipv4Net>,
}

/// Parse the kernel's IPv4 routing table (little-endian hex fields).
pub fn parse_routes(table: &str) -> Routes {
    let mut routes = Routes::default();
    let hex = |v: &str| {
        u32::from_str_radix(v, 16)
            .ok()
            .map(|n| Ipv4Addr::from(n.to_le_bytes()))
    };
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let (Some(destination), Some(gateway), Some(mask)) =
            (hex(fields[1]), hex(fields[2]), hex(fields[7]))
        else {
            continue;
        };
        let flags = u16::from_str_radix(fields[3], 16).unwrap_or(0);
        if flags & 1 == 0 {
            continue; // route not up
        }
        if destination.is_unspecified() && mask.is_unspecified() && flags & 2 != 0 {
            routes.gateway.get_or_insert(gateway);
        } else if gateway.is_unspecified()
            && let Ok(prefix) = ipnet::ipv4_mask_to_prefix(mask)
            && let Ok(net) = Ipv4Net::new(destination, prefix)
            && !destination.is_loopback()
        {
            routes.attached.push(net);
        }
    }
    routes
}

pub fn routes() -> Routes {
    std::fs::read_to_string("/proc/net/route")
        .map(|t| parse_routes(&t))
        .unwrap_or_default()
}

/// Routers between the server and `target`, in order, excluding `target` itself. Empty when raw
/// ICMP sockets are unavailable or nothing answers. One probe per hop, at most `max_hops`.
pub async fn trace(target: Ipv4Addr, max_hops: u8) -> Vec<Ipv4Addr> {
    tokio::task::spawn_blocking(move || trace_blocking(target, max_hops))
        .await
        .unwrap_or_default()
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32)
        .sum();
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// For an ICMP message received on a raw socket (with its IP header): who sent it, whether it is
/// our echo reply or a time-exceeded about our probe, and the sequence (the probe's TTL).
fn parse_icmp(packet: &[u8], identifier: u16) -> Option<(Ipv4Addr, bool, u16)> {
    let header = (*packet.first()? as usize & 0x0F) * 4;
    let address = packet.get(12..16)?;
    let source = Ipv4Addr::new(address[0], address[1], address[2], address[3]);
    let icmp = packet.get(header..)?;
    match *icmp.first()? {
        0 => {
            let id = u16::from_be_bytes([*icmp.get(4)?, *icmp.get(5)?]);
            let seq = u16::from_be_bytes([*icmp.get(6)?, *icmp.get(7)?]);
            (id == identifier).then_some((source, true, seq))
        }
        11 => {
            let inner = icmp.get(8..)?;
            let inner_header = (*inner.first()? as usize & 0x0F) * 4;
            let original = inner.get(inner_header..inner_header + 8)?;
            let id = u16::from_be_bytes([original[4], original[5]]);
            let seq = u16::from_be_bytes([original[6], original[7]]);
            (original[0] == 8 && id == identifier).then_some((source, false, seq))
        }
        _ => None,
    }
}

fn trace_blocking(target: Ipv4Addr, max_hops: u8) -> Vec<Ipv4Addr> {
    use socket2::{Domain, Protocol, Socket, Type};
    let Ok(socket) = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)) else {
        return vec![];
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let identifier: u16 = rand::random();
    let address = std::net::SocketAddr::from((target, 0)).into();
    let mut hops = Vec::new();
    for ttl in 1..=max_hops {
        if socket.set_ttl_v4(ttl as u32).is_err() {
            return vec![];
        }
        let mut packet = [8u8, 0, 0, 0, 0, 0, 0, 0, b'H', b'O', b'P', b'E'];
        packet[4..6].copy_from_slice(&identifier.to_be_bytes());
        packet[6..8].copy_from_slice(&(ttl as u16).to_be_bytes());
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
        if socket.send_to(&packet, &address).is_err() {
            return hops;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 1500];
        let mut answer = None;
        while std::time::Instant::now() < deadline {
            let Ok(size) = socket.recv(&mut buffer) else {
                continue;
            };
            // SAFETY: recv initialised the first `size` bytes.
            let bytes: Vec<u8> = buffer[..size]
                .iter()
                .map(|b| unsafe { b.assume_init() })
                .collect();
            if let Some((from, reached, seq)) = parse_icmp(&bytes, identifier)
                && seq == ttl as u16
            {
                answer = Some((from, reached || from == target));
                break;
            }
        }
        match answer {
            Some((_, true)) => return hops,
            Some((router, false)) => hops.push(router),
            None => {} // a silent hop; keep going
        }
    }
    hops
}

/// The gateway of each range and the router chain above it.
#[derive(Debug, Default, Clone)]
pub struct Layout {
    /// Network id → its gateway address.
    pub gateways: HashMap<String, Ipv4Addr>,
    /// Router address → the router above it.
    pub upstream: HashMap<Ipv4Addr, Ipv4Addr>,
}

impl Layout {
    pub fn addresses(&self) -> HashSet<String> {
        self.gateways
            .values()
            .chain(self.upstream.keys())
            .chain(self.upstream.values())
            .map(|a| a.to_string())
            .collect()
    }
}

/// Work out the layout from the routing table and traces of routed ranges (`traces` holds the
/// hops recorded for each routed network id).
pub fn layout(
    networks: &[Network],
    routes: &Routes,
    traces: &HashMap<String, Vec<Ipv4Addr>>,
) -> Layout {
    let mut layout = Layout::default();
    for network in networks {
        let Ok(net) = network.cidr.parse::<Ipv4Net>() else {
            continue;
        };
        let attached = routes.attached.iter().any(|a| a.contains(&net.network()));
        if attached {
            if let Some(gateway) = routes.gateway.filter(|g| net.contains(g)) {
                layout.gateways.insert(network.id.clone(), gateway);
            }
        } else if let Some(hops) = traces.get(&network.id).filter(|h| !h.is_empty()) {
            layout
                .gateways
                .insert(network.id.clone(), *hops.last().expect("non-empty"));
            for pair in hops.windows(2) {
                layout.upstream.entry(pair[1]).or_insert(pair[0]);
            }
        }
    }
    layout
}

/// Set each host's parent from the layout. Returns the hosts that changed.
pub fn assign(hosts: &mut [Host], layout: &Layout, networks: &[Network]) -> Vec<usize> {
    let by_address: HashMap<String, String> = hosts
        .iter()
        .flat_map(|h| h.addresses.iter().map(|a| (a.clone(), h.id.clone())))
        .collect();
    let names: HashMap<&str, (&str, &str)> = networks
        .iter()
        .map(|n| (n.id.as_str(), (n.name.as_str(), n.cidr.as_str())))
        .collect();
    let mut changed = vec![];
    for (index, host) in hosts.iter_mut().enumerate() {
        let mut parent: Option<(String, String)> = None;
        // A router: its upstream router.
        for address in &host.addresses {
            if let Ok(ip) = address.parse::<Ipv4Addr>()
                && let Some(up) = layout.upstream.get(&ip)
                && let Some(id) = by_address.get(&up.to_string())
            {
                parent = Some((
                    id.clone(),
                    format!("Next router towards the server is {up}"),
                ));
            }
        }
        // Anything else: the gateway of its first range that has one.
        if parent.is_none() {
            for network in &host.network_ids {
                let Some(gateway) = layout.gateways.get(network) else {
                    continue;
                };
                if host.addresses.contains(&gateway.to_string()) {
                    break; // the gateway itself sits at the top of its range
                }
                if let Some(id) = by_address.get(&gateway.to_string()) {
                    let (name, cidr) = names
                        .get(network.as_str())
                        .copied()
                        .unwrap_or(("this range", ""));
                    parent = Some((
                        id.clone(),
                        format!("{gateway} is the gateway of {name} ({cidr})"),
                    ));
                    break;
                }
            }
        }
        let (id, reason) = parent.map_or((None, None), |(i, r)| (Some(i), Some(r)));
        if host.parent_id != id || host.parent_reason != reason {
            host.parent_id = id;
            host.parent_reason = reason;
            changed.push(index);
        }
    }
    changed
}

/// Make sure every router address in the layout is a host, so the hierarchy has a node for it.
pub async fn ensure_router_hosts(state: &State, layout: &Layout) -> anyhow::Result<()> {
    let hosts = state.hosts().await?;
    let known: HashSet<&String> = hosts.iter().flat_map(|h| h.addresses.iter()).collect();
    for address in layout.addresses() {
        if known.contains(&address) {
            continue;
        }
        let mut host = Host::new(
            uuid::Uuid::new_v4().to_string(),
            address.clone(),
            "traceroute",
        );
        host.addresses = vec![address.clone()];
        host.network_ids = crate::api::matching_networks(state, &host.addresses)
            .await
            .map_err(|e| anyhow::anyhow!(e.1))?;
        host.last_seen = now();
        crate::classify::refresh(&mut host, &layout.addresses());
        state.save_host(&host).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
eth0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
eth0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
lo\t0000007F\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n";

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
    fn routes_give_the_default_gateway_and_attached_networks() {
        let routes = parse_routes(TABLE);
        assert_eq!(routes.gateway, Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(
            routes.attached,
            vec!["192.168.1.0/24".parse::<Ipv4Net>().unwrap()]
        );
        assert_eq!(parse_routes("garbage\nmore garbage"), Routes::default());
    }

    #[test]
    fn hosts_sit_behind_their_range_gateway_and_routers_chain() {
        let routes = parse_routes(TABLE);
        let networks = [
            network("lan", "192.168.1.0/24"),
            network("iot", "192.168.30.0/24"),
        ];
        let traces = HashMap::from([(
            "iot".to_string(),
            vec![Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(10, 0, 0, 2)],
        )]);
        let layout = layout(&networks, &routes, &traces);
        assert_eq!(layout.gateways["lan"], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(layout.gateways["iot"], Ipv4Addr::new(10, 0, 0, 2));
        let host = |id: &str, address: &str, network: &str| {
            let mut h = Host::new(id.into(), id.into(), "icmp");
            h.addresses = vec![address.into()];
            h.network_ids = if network.is_empty() {
                vec![]
            } else {
                vec![network.into()]
            };
            h
        };
        let mut hosts = vec![
            host("router", "192.168.1.1", "lan"),
            host("core", "10.0.0.2", ""),
            host("laptop", "192.168.1.20", "lan"),
            host("plug", "192.168.30.7", "iot"),
        ];
        let changed = assign(&mut hosts, &layout, &networks);
        assert_eq!(changed, vec![1, 2, 3]);
        assert_eq!(hosts[0].parent_id, None);
        assert_eq!(hosts[1].parent_id.as_deref(), Some("router"));
        assert_eq!(hosts[2].parent_id.as_deref(), Some("router"));
        assert_eq!(hosts[3].parent_id.as_deref(), Some("core"));
        assert!(
            hosts[3]
                .parent_reason
                .as_deref()
                .unwrap()
                .contains("gateway of iot")
        );
        assert!(assign(&mut hosts, &layout, &networks).is_empty(), "stable");
    }

    #[test]
    fn icmp_replies_and_time_exceeded_are_matched_to_the_probe() {
        let mut reply = vec![0x45u8; 20];
        reply[12..16].copy_from_slice(&[10, 0, 0, 9]);
        reply.extend_from_slice(&[0, 0, 0, 0, 0x12, 0x34, 0, 3]);
        assert_eq!(
            parse_icmp(&reply, 0x1234),
            Some((Ipv4Addr::new(10, 0, 0, 9), true, 3))
        );
        assert_eq!(parse_icmp(&reply, 0x9999), None);
        let mut exceeded = vec![0x45u8; 20];
        exceeded[12..16].copy_from_slice(&[192, 168, 1, 1]);
        exceeded.extend_from_slice(&[11, 0, 0, 0, 0, 0, 0, 0]);
        exceeded.extend_from_slice(&[0x45; 20]);
        exceeded.extend_from_slice(&[8, 0, 0, 0, 0x12, 0x34, 0, 1]);
        assert_eq!(
            parse_icmp(&exceeded, 0x1234),
            Some((Ipv4Addr::new(192, 168, 1, 1), false, 1))
        );
        for length in 0..exceeded.len() {
            let _ = parse_icmp(&exceeded[..length], 0x1234);
        }
        assert_eq!(checksum(&[8, 0, 0, 0, 0x12, 0x34, 0, 1]), !0x1A35u16);
    }
}
