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
            Arc, Mutex, RwLock, Weak,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };
    use tokio::sync::oneshot;
    type Pending = HashMap<(Ipv4Addr, Ipv4Addr), Vec<oneshot::Sender<String>>>;

    const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct InterfaceSpec {
        name: String,
        index: u32,
        mac: [u8; 6],
        networks: Vec<Ipv4Net>,
    }

    struct Interface {
        spec: InterfaceSpec,
        sender: Mutex<Box<dyn pnet_datalink::DataLinkSender>>,
        pending: Mutex<Pending>,
        active: AtomicBool,
    }
    #[derive(Default)]
    struct Shared {
        interfaces: RwLock<Vec<Arc<Interface>>>,
    }
    #[derive(Clone, Default)]
    pub struct Arp {
        shared: Arc<Shared>,
    }
    impl Arp {
        pub fn new(enabled: bool) -> Self {
            if !enabled {
                return Self::default();
            }
            let shared = Arc::new(Shared::default());
            refresh(&shared, true);
            let weak = Arc::downgrade(&shared);
            if let Err(error) = thread::Builder::new()
                .name("hope-arp-watch".into())
                .spawn(move || refresh_loop(weak))
            {
                tracing::error!(%error, "active ARP interface recovery could not start");
            }
            Self { shared }
        }
        pub fn available(&self) -> bool {
            interfaces(&self.shared)
                .iter()
                .any(|i| i.active.load(Ordering::Relaxed))
        }
        pub fn on_link(&self, ip: Ipv4Addr) -> bool {
            interfaces(&self.shared).iter().any(|i| {
                i.active.load(Ordering::Relaxed)
                    && i.spec.networks.iter().any(|net| net.contains(&ip))
            })
        }
        pub async fn probe(&self, ip: Ipv4Addr) -> Option<String> {
            let interface = interfaces(&self.shared)
                .iter()
                .find(|i| {
                    i.active.load(Ordering::Relaxed)
                        && i.spec.networks.iter().any(|n| n.contains(&ip))
                })?
                .clone();
            let source = interface
                .spec
                .networks
                .iter()
                .find(|n| n.contains(&ip))?
                .addr();
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
            packet[6..12].copy_from_slice(&interface.spec.mac);
            packet[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
            packet[22..28].copy_from_slice(&interface.spec.mac);
            packet[28..32].copy_from_slice(&source.octets());
            packet[38..42].copy_from_slice(&ip.octets());
            let outgoing = interface.clone();
            // The global probe limit bounds these blocking tasks. No raw send blocks a Tokio worker.
            let sent = tokio::task::spawn_blocking(move || {
                outgoing
                    .sender
                    .lock()
                    .ok()
                    .and_then(|mut sender| sender.send_to(&packet, None))
                    .is_some_and(|result| result.is_ok())
            })
            .await
            .ok()?;
            if !sent {
                tracing::warn!(interface = %interface.spec.name, "active ARP send failed; reopening the raw packet socket");
                deactivate(&interface);
                return None;
            }
            // ARP is restricted to directly attached networks. Keep its deadline short;
            // routed ICMP/TCP retain their longer deadlines under the same shared limit.
            tokio::time::timeout(Duration::from_millis(250), receiver)
                .await
                .ok()?
                .ok()
        }
    }

    fn interfaces(shared: &Shared) -> Vec<Arc<Interface>> {
        shared
            .interfaces
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn refresh_loop(shared: Weak<Shared>) {
        loop {
            thread::sleep(REFRESH_INTERVAL);
            let Some(shared) = shared.upgrade() else {
                break;
            };
            refresh(&shared, false);
        }
    }

    fn refresh(shared: &Shared, initial: bool) {
        let previous = interfaces(shared);
        let mut current = Vec::new();
        for interface in pnet_datalink::interfaces()
            .into_iter()
            .filter(|interface| interface.is_up() && !interface.is_loopback())
            .take(16)
        {
            let Some(mac) = interface.mac.map(|mac| mac.octets()) else {
                continue;
            };
            let mut networks: Vec<_> = interface
                .ips
                .iter()
                .filter_map(|ip| match ip.ip() {
                    std::net::IpAddr::V4(address) => Ipv4Net::new(address, ip.prefix()).ok(),
                    _ => None,
                })
                .collect();
            networks.sort_unstable();
            if networks.is_empty() {
                continue;
            }
            let spec = InterfaceSpec {
                name: interface.name.clone(),
                index: interface.index,
                mac,
                networks,
            };
            if let Some(existing) = previous.iter().find(|existing| {
                can_reuse(
                    &existing.spec,
                    &spec,
                    existing.active.load(Ordering::Acquire),
                )
            }) {
                current.push(existing.clone());
                continue;
            }
            if let Some(opened) = open_interface(interface, spec, initial) {
                current.push(opened);
            }
        }

        let was_available = previous
            .iter()
            .any(|interface| interface.active.load(Ordering::Acquire));
        for old in &previous {
            if !current.iter().any(|new| Arc::ptr_eq(new, old)) {
                deactivate(old);
            }
        }
        let is_available = current
            .iter()
            .any(|interface| interface.active.load(Ordering::Acquire));
        *shared
            .interfaces
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = current;
        match (was_available, is_available) {
            (true, false) => tracing::warn!("active ARP lost all usable interfaces"),
            (false, true) if !initial => {
                tracing::info!("active ARP recovered on a network interface")
            }
            (false, false) if initial => tracing::warn!(
                "active ARP unavailable: no IPv4 Ethernet interface could be opened for raw packets"
            ),
            _ => {}
        }
    }

    fn open_interface(
        interface: pnet_datalink::NetworkInterface,
        spec: InterfaceSpec,
        initial: bool,
    ) -> Option<Arc<Interface>> {
        let config = pnet_datalink::Config {
            read_timeout: Some(Duration::from_millis(100)),
            write_timeout: Some(Duration::from_millis(100)),
            read_buffer_size: 4096,
            write_buffer_size: 4096,
            ..Default::default()
        };
        let (sender, receiver) = match pnet_datalink::channel(&interface, config) {
            Ok(pnet_datalink::Channel::Ethernet(sender, receiver)) => (sender, receiver),
            Ok(_) => {
                if initial {
                    tracing::warn!(interface = %spec.name, "active ARP unavailable: not an Ethernet channel");
                }
                return None;
            }
            Err(error) => {
                // Almost always EPERM: CAP_NET_RAW is missing, or AF_PACKET is blocked by a sandbox.
                if initial {
                    tracing::warn!(
                        interface = %spec.name,
                        %error,
                        "active ARP unavailable on this interface: a raw packet socket could not be opened (needs CAP_NET_RAW)"
                    );
                } else {
                    tracing::debug!(interface = %spec.name, %error, "active ARP socket retry failed");
                }
                return None;
            }
        };
        tracing::info!(interface = %spec.name, networks = ?spec.networks, "active ARP enabled");
        let state = Arc::new(Interface {
            spec,
            sender: Mutex::new(sender),
            pending: Mutex::new(HashMap::new()),
            active: AtomicBool::new(true),
        });
        let weak = Arc::downgrade(&state);
        let name = state.spec.name.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("hope-arp-{}", name))
            .spawn(move || receive_loop(receiver, weak))
        {
            tracing::warn!(interface = %name, %error, "active ARP receiver could not start");
            deactivate(&state);
            return None;
        }
        Some(state)
    }

    fn receive_loop(mut receiver: Box<dyn pnet_datalink::DataLinkReceiver>, weak: Weak<Interface>) {
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            if !state.active.load(Ordering::Acquire) {
                break;
            }
            match receiver.next() {
                Ok(packet) => {
                    if let Some((source, target, mac)) = super::reply(packet, state.spec.mac)
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
                    tracing::warn!(interface = %state.spec.name, %error, "active ARP stopped: the raw packet socket failed");
                    deactivate(&state);
                    break;
                }
            }
            if let Ok(mut pending) = state.pending.lock() {
                pending.retain(|_, waiters| {
                    waiters.retain(|waiter| !waiter.is_closed());
                    !waiters.is_empty()
                });
            }
        }
    }

    fn deactivate(interface: &Interface) {
        interface.active.store(false, Ordering::Release);
        if let Ok(mut pending) = interface.pending.lock() {
            pending.clear();
        }
    }

    fn can_reuse(existing: &InterfaceSpec, current: &InterfaceSpec, active: bool) -> bool {
        active && existing == current
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn spec(name: &str, index: u32, mac: [u8; 6], network: &str) -> InterfaceSpec {
            InterfaceSpec {
                name: name.into(),
                index,
                mac,
                networks: vec![network.parse().expect("network")],
            }
        }

        #[test]
        fn socket_is_reused_only_while_interface_identity_and_addresses_match() {
            let original = spec("eth0", 2, [2, 0, 0, 0, 0, 1], "192.168.1.0/24");
            assert!(can_reuse(&original, &original, true));
            assert!(!can_reuse(
                &original,
                &spec("eth0", 2, [2, 0, 0, 0, 0, 1], "192.168.2.0/24"),
                true
            ));
            assert!(!can_reuse(
                &original,
                &spec("eth1", 2, [2, 0, 0, 0, 0, 1], "192.168.1.0/24"),
                true
            ));
            assert!(!can_reuse(
                &original,
                &spec("eth0", 2, [2, 0, 0, 0, 0, 2], "192.168.1.0/24"),
                true
            ));
            assert!(!can_reuse(
                &original,
                &spec("eth0", 3, [2, 0, 0, 0, 0, 1], "192.168.1.0/24"),
                true
            ));
            assert!(!can_reuse(&original, &original, false));
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
