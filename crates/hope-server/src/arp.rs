//! Active, correlated on-link ARP. Cache entries alone are not evidence of current reachability.
#[cfg(target_os = "linux")]
mod platform {
    use ipnet::Ipv4Net;
    use std::{
        collections::HashMap,
        net::Ipv4Addr,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::sync::oneshot;
    type Pending = HashMap<Ipv4Addr, Vec<oneshot::Sender<String>>>;
    struct Interface {
        sender: Mutex<Box<dyn pnet_datalink::DataLinkSender>>,
        pending: Mutex<Pending>,
        mac: [u8; 6],
        source: Ipv4Addr,
        networks: Vec<Ipv4Net>,
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
                let networks: Vec<Ipv4Net> = interface
                    .ips
                    .iter()
                    .filter_map(|ip| match ip.ip() {
                        std::net::IpAddr::V4(address) => Ipv4Net::new(address, ip.prefix()).ok(),
                        _ => None,
                    })
                    .collect();
                let Some(network) = networks.first() else {
                    continue;
                };
                let config = pnet_datalink::Config {
                    read_timeout: Some(Duration::from_millis(100)),
                    write_timeout: Some(Duration::from_millis(100)),
                    read_buffer_size: 4096,
                    write_buffer_size: 4096,
                    ..Default::default()
                };
                let Ok(pnet_datalink::Channel::Ethernet(sender, mut receiver)) =
                    pnet_datalink::channel(&interface, config)
                else {
                    continue;
                };
                let state = Arc::new(Interface {
                    sender: Mutex::new(sender),
                    pending: Mutex::new(HashMap::new()),
                    mac: mac.octets(),
                    source: network.addr(),
                    networks,
                });
                let weak = Arc::downgrade(&state);
                std::thread::spawn(move || {
                    loop {
                        let Some(state) = weak.upgrade() else {
                            break;
                        };
                        if let Ok(packet) = receiver.next()
                            && packet.len() >= 42
                            && packet[12..14] == [8, 6]
                            && packet[14..22] == [0, 1, 8, 0, 6, 4, 0, 2]
                            && packet[32..38] == state.mac
                            && packet[38..42] == state.source.octets()
                        {
                            let ip = Ipv4Addr::new(packet[28], packet[29], packet[30], packet[31]);
                            let mac = packet[22..28]
                                .iter()
                                .map(|v| format!("{v:02x}"))
                                .collect::<Vec<_>>()
                                .join(":");
                            if let Ok(mut pending) = state.pending.lock()
                                && let Some(waiters) = pending.remove(&ip)
                            {
                                for waiter in waiters {
                                    let _ = waiter.send(mac.clone());
                                }
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
            Self { interfaces }
        }
        pub fn available(&self) -> bool {
            !self.interfaces.is_empty()
        }
        pub fn on_link(&self, ip: Ipv4Addr) -> bool {
            self.interfaces
                .iter()
                .any(|i| i.networks.iter().any(|net| net.contains(&ip)))
        }
        pub async fn probe(&self, ip: Ipv4Addr) -> Option<String> {
            let interface = self
                .interfaces
                .iter()
                .find(|i| i.networks.iter().any(|n| n.contains(&ip)))?;
            let (sender, receiver) = oneshot::channel();
            interface
                .pending
                .lock()
                .ok()?
                .entry(ip)
                .or_default()
                .push(sender);
            let mut packet = [0u8; 42];
            packet[..6].copy_from_slice(&[255; 6]);
            packet[6..12].copy_from_slice(&interface.mac);
            packet[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
            packet[22..28].copy_from_slice(&interface.mac);
            packet[28..32].copy_from_slice(&interface.source.octets());
            packet[38..42].copy_from_slice(&ip.octets());
            interface.sender.lock().ok()?.send_to(&packet, None)?.ok()?;
            tokio::time::timeout(Duration::from_millis(750), receiver)
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
