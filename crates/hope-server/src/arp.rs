//! Active, correlated on-link ARP. Cache entries alone are not evidence of current reachability.
#[cfg(any(target_os = "linux", test))]
fn reply(
    packet: &[u8],
    local_mac: [u8; 6],
) -> Option<(std::net::Ipv4Addr, std::net::Ipv4Addr, String)> {
    if packet.len() < 42
        || packet[..6] != local_mac
        || packet[12..22] != [8, 6, 0, 1, 8, 0, 6, 4, 0, 2]
        || packet[32..38] != local_mac
        || packet[6..12] != packet[22..28]
        || packet[22] & 1 != 0
        || packet[22..28] == [0; 6]
    {
        return None;
    }
    let source = std::net::Ipv4Addr::new(packet[28], packet[29], packet[30], packet[31]);
    let target = std::net::Ipv4Addr::new(packet[38], packet[39], packet[40], packet[41]);
    if source.is_unspecified() || source.is_multicast() || source.is_broadcast() {
        return None;
    }
    let mac = packet[22..28]
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<Vec<_>>()
        .join(":");
    Some((source, target, mac))
}

#[cfg(target_os = "linux")]
mod platform {
    use ipnet::Ipv4Net;
    use std::{
        collections::HashMap,
        net::Ipv4Addr,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    use tokio::sync::oneshot;
    type Pending = HashMap<(Ipv4Addr, Ipv4Addr), Vec<oneshot::Sender<String>>>;
    struct Interface {
        sender: Mutex<Box<dyn pnet_datalink::DataLinkSender>>,
        pending: Mutex<Pending>,
        mac: [u8; 6],
        networks: Vec<Ipv4Net>,
        active: AtomicBool,
    }
    #[derive(Clone, Default)]
    pub struct Arp {
        interfaces: Vec<Arc<Interface>>,
    }
    impl Arp {
        pub fn new(enabled: bool) -> Self {
            if !enabled {
                return Self::default();
            }
            let mut interfaces = vec![];
            for interface in pnet_datalink::interfaces()
                .into_iter()
                .filter(|i| i.is_up() && !i.is_loopback())
                .take(16)
            {
                let Some(mac) = interface.mac else {
                    continue;
                };
                let networks: Vec<_> = interface
                    .ips
                    .iter()
                    .filter_map(|ip| match ip.ip() {
                        std::net::IpAddr::V4(address) => Ipv4Net::new(address, ip.prefix()).ok(),
                        _ => None,
                    })
                    .collect();
                if networks.is_empty() {
                    continue;
                }
                let config = pnet_datalink::Config {
                    read_timeout: Some(Duration::from_millis(100)),
                    write_timeout: Some(Duration::from_millis(100)),
                    read_buffer_size: 4096,
                    write_buffer_size: 4096,
                    ..Default::default()
                };
                let (sender, mut receiver) = match pnet_datalink::channel(&interface, config) {
                    Ok(pnet_datalink::Channel::Ethernet(sender, receiver)) => (sender, receiver),
                    Ok(_) => {
                        tracing::warn!(interface = %interface.name, "active ARP unavailable: not an Ethernet channel");
                        continue;
                    }
                    Err(error) => {
                        // Almost always EPERM: the process lacks CAP_NET_RAW (or a sandbox such as a
                        // container profile or systemd's RestrictAddressFamilies blocks AF_PACKET).
                        tracing::warn!(
                            interface = %interface.name,
                            %error,
                            "active ARP unavailable on this interface: a raw packet socket could not be opened (needs CAP_NET_RAW)"
                        );
                        continue;
                    }
                };
                tracing::info!(interface = %interface.name, networks = ?networks, "active ARP enabled");
                let state = Arc::new(Interface {
                    sender: Mutex::new(sender),
                    pending: Mutex::new(HashMap::new()),
                    mac: mac.octets(),
                    networks,
                    active: AtomicBool::new(true),
                });
                let weak = Arc::downgrade(&state);
                std::thread::spawn(move || {
                    loop {
                        let Some(state) = weak.upgrade() else {
                            break;
                        };
                        match receiver.next() {
                            Ok(packet) => {
                                if let Some((source, target, mac)) = super::reply(packet, state.mac)
                                    && let Ok(mut pending) = state.pending.lock()
                                    && let Some(waiters) = pending.remove(&(source, target))
                                {
                                    for waiter in waiters {
                                        let _ = waiter.send(mac.clone());
                                    }
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(error) => {
                                tracing::warn!(%error, "active ARP stopped: the raw packet socket failed");
                                state.active.store(false, Ordering::Relaxed);
                                break;
                            }
                        }
                        if let Ok(mut pending) = state.pending.lock() {
                            pending.retain(|_, waiters| {
                                waiters.retain(|w| !w.is_closed());
                                !waiters.is_empty()
                            });
                        }
                    }
                });
                interfaces.push(state);
            }
            if interfaces.is_empty() {
                tracing::warn!(
                    "active ARP unavailable: no IPv4 Ethernet interface could be opened for raw packets"
                );
            }
            Self { interfaces }
        }
        pub fn available(&self) -> bool {
            self.interfaces
                .iter()
                .any(|i| i.active.load(Ordering::Relaxed))
        }
        pub fn on_link(&self, ip: Ipv4Addr) -> bool {
            self.interfaces.iter().any(|i| {
                i.active.load(Ordering::Relaxed) && i.networks.iter().any(|net| net.contains(&ip))
            })
        }
        pub async fn probe(&self, ip: Ipv4Addr) -> Option<String> {
            let interface = self
                .interfaces
                .iter()
                .find(|i| {
                    i.active.load(Ordering::Relaxed) && i.networks.iter().any(|n| n.contains(&ip))
                })?
                .clone();
            let source = interface.networks.iter().find(|n| n.contains(&ip))?.addr();
            let (sender, receiver) = oneshot::channel();
            {
                let mut pending = interface.pending.lock().ok()?;
                if pending.values().map(Vec::len).sum::<usize>() >= 64 {
                    return None;
                }
                pending.entry((ip, source)).or_default().push(sender);
            }
            let mut packet = [0u8; 42];
            packet[..6].copy_from_slice(&[255; 6]);
            packet[6..12].copy_from_slice(&interface.mac);
            packet[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
            packet[22..28].copy_from_slice(&interface.mac);
            packet[28..32].copy_from_slice(&source.octets());
            packet[38..42].copy_from_slice(&ip.octets());
            let outgoing = interface.clone();
            // The global probe limit bounds these blocking tasks. No raw send blocks a Tokio worker.
            tokio::task::spawn_blocking(move || {
                outgoing.sender.lock().ok()?.send_to(&packet, None)?.ok()
            })
            .await
            .ok()??;
            // ARP is restricted to directly attached networks. Keep its deadline short;
            // routed ICMP/TCP retain their longer deadlines under the same shared limit.
            tokio::time::timeout(Duration::from_millis(250), receiver)
                .await
                .ok()?
                .ok()
        }
    }
}
#[cfg(not(target_os = "linux"))]
mod platform {
    #[derive(Clone, Default)]
    pub struct Arp;
    impl Arp {
        pub fn new(_: bool) -> Self {
            Self
        }
        pub fn available(&self) -> bool {
            false
        }
        pub fn on_link(&self, _: std::net::Ipv4Addr) -> bool {
            false
        }
        pub async fn probe(&self, _: std::net::Ipv4Addr) -> Option<String> {
            None
        }
    }
}
pub use platform::Arp;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_valid_correlated_reply_fields_are_parsed() {
        let local = [2, 0, 0, 0, 0, 1];
        let remote = [2, 0, 0, 0, 0, 2];
        let mut packet = [0u8; 42];
        packet[..6].copy_from_slice(&local);
        packet[6..12].copy_from_slice(&remote);
        packet[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 2]);
        packet[22..28].copy_from_slice(&remote);
        packet[28..32].copy_from_slice(&[192, 168, 30, 2]);
        packet[32..38].copy_from_slice(&local);
        packet[38..42].copy_from_slice(&[192, 168, 30, 1]);
        let parsed = reply(&packet, local).expect("valid");
        assert_eq!(parsed.0.to_string(), "192.168.30.2");
        assert_eq!(parsed.1.to_string(), "192.168.30.1");
        for length in 0..42 {
            assert!(reply(&packet[..length], local).is_none());
        }
        for index in [0, 6, 12, 14, 16, 18, 19, 21, 32] {
            let mut bad = packet;
            bad[index] ^= 1;
            assert!(reply(&bad, local).is_none(), "field {index}");
        }
    }
}
