//! Evidence about what a device is, gathered without credentials: open common ports, the title of
//! its web interface, and what it announces over mDNS and SSDP. Everything read from the network is
//! untrusted: sizes, counts and string lengths are bounded and control characters are dropped.
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};
use tokio::{net::UdpSocket, time::Instant};

/// TCP ports probed on every discovered device, with the service name shown in the console.
/// Chosen to identify device types: infrastructure (DNS, Winbox, Proxmox), storage (SMB, AFP, NFS,
/// DSM), printing, cameras (RTSP), media (Cast, AirPlay, Plex), home automation and Apple devices.
pub const COMMON_PORTS: [(u16, &str); 40] = [
    (21, "FTP"),
    (22, "SSH"),
    (23, "Telnet"),
    (25, "SMTP"),
    (53, "DNS"),
    (80, "HTTP"),
    (111, "RPC"),
    (139, "NetBIOS"),
    (143, "IMAP"),
    (443, "HTTPS"),
    (445, "SMB"),
    (515, "LPD printing"),
    (548, "AFP"),
    (554, "RTSP"),
    (631, "IPP printing"),
    (1883, "MQTT"),
    (2049, "NFS"),
    (3000, "HTTP (3000)"),
    (3306, "MySQL"),
    (3389, "RDP"),
    (5000, "HTTP (5000)"),
    (5001, "HTTPS (5001)"),
    (5432, "PostgreSQL"),
    (5900, "VNC"),
    (6443, "Kubernetes API"),
    (7000, "AirPlay"),
    (8006, "Proxmox VE"),
    (8007, "Proxmox Backup Server"),
    (8008, "HTTP (8008)"),
    (8009, "Google Cast"),
    (8080, "HTTP (8080)"),
    (8123, "Home Assistant"),
    (8291, "MikroTik Winbox"),
    (8443, "HTTPS (8443)"),
    (8883, "MQTT over TLS"),
    (9000, "HTTP (9000)"),
    (9090, "HTTP (9090)"),
    (9100, "Raw printing"),
    (32400, "Plex"),
    (62078, "Apple device sync"),
];

pub fn port_name(port: u16) -> Option<&'static str> {
    COMMON_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, name)| *name)
}

/// Untrusted text from the network: control characters removed, whitespace collapsed, bounded.
pub fn clean(value: &str, limit: usize) -> Option<String> {
    let text: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text: String = text.chars().take(limit).collect();
    (!text.is_empty()).then_some(text)
}

// ---------------------------------------------------------------------------------------------
// Web interface title
// ---------------------------------------------------------------------------------------------

/// Web ports tried in order, with their scheme. Proxmox, NAS and router interfaces sit on these.
const WEB_PORTS: [(u16, &str); 11] = [
    (8006, "https"),
    (443, "https"),
    (80, "http"),
    (8443, "https"),
    (8080, "http"),
    (5001, "https"),
    (5000, "http"),
    (8123, "http"),
    (8007, "https"),
    (9000, "http"),
    (3000, "http"),
];

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // Devices on a LAN almost always present self-signed certificates. Only the page title and
        // Server header are read, never credentials or cookies.
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(3))
        .timeout(Duration::from_secs(3))
        .connect_timeout(Duration::from_secs(2))
        .user_agent("H.O.P.E discovery")
        // Devices are on the LAN: never route these requests through a configured proxy.
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// `<title>` and `Server` header of the first web interface that answers among the open ports.
pub async fn http_banner(
    client: &reqwest::Client,
    ip: Ipv4Addr,
    open: &[u16],
) -> (Option<String>, Option<String>) {
    let mut server = None;
    for (port, scheme) in WEB_PORTS.iter().filter(|(p, _)| open.contains(p)) {
        let Ok(mut response) = client.get(format!("{scheme}://{ip}:{port}/")).send().await else {
            continue;
        };
        if server.is_none() {
            server = response
                .headers()
                .get(reqwest::header::SERVER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| clean(v, 120));
        }
        let mut body = Vec::new();
        while body.len() < 65536 {
            match tokio::time::timeout(Duration::from_secs(2), response.chunk()).await {
                Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
                _ => break,
            }
        }
        if let Some(title) = title(&String::from_utf8_lossy(&body)) {
            return (Some(title), server);
        }
    }
    (None, server)
}

/// Text of the first `<title>` element, with the common entities decoded.
pub fn title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let text = html.get(start..end)?;
    let decoded = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    clean(&decoded, 120)
}

// ---------------------------------------------------------------------------------------------
// mDNS (DNS-SD) and SSDP sweeps
// ---------------------------------------------------------------------------------------------

/// What a device announced during a sweep.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Announcement {
    pub mdns_name: Option<String>,
    pub mdns_services: BTreeSet<String>,
    pub mdns_model: Option<String>,
    pub mdns_instances: BTreeSet<String>,
    /// The address sent or was named in an mDNS answer this sweep, so its mDNS fields are current.
    pub mdns_seen: bool,
    pub ssdp_server: Option<String>,
    pub upnp_device_type: Option<String>,
    pub upnp_friendly_name: Option<String>,
    pub upnp_manufacturer: Option<String>,
    pub upnp_model: Option<String>,
    location: Option<String>,
}

const MAX_DEVICES: usize = 1024;

/// Service types queried explicitly, besides the DNS-SD enumeration, because many devices answer
/// only for their own types.
const MDNS_TYPES: [&str; 30] = [
    "_services._dns-sd._udp",
    "_googlecast._tcp",
    "_airplay._tcp",
    "_raop._tcp",
    "_hap._tcp",
    "_ipp._tcp",
    "_ipps._tcp",
    "_printer._tcp",
    "_pdl-datastream._tcp",
    "_smb._tcp",
    "_afpovertcp._tcp",
    "_ssh._tcp",
    "_sftp-ssh._tcp",
    "_workstation._tcp",
    "_device-info._tcp",
    "_companion-link._tcp",
    "_apple-mobdev2._tcp",
    "_sleep-proxy._udp",
    "_home-assistant._tcp",
    "_esphomelib._tcp",
    "_hue._tcp",
    "_sonos._tcp",
    "_spotify-connect._tcp",
    "_miio._udp",
    "_amzn-wplay._tcp",
    "_matter._tcp",
    "_homeconnect._tcp",
    "_octoprint._tcp",
    "_wled._tcp",
    "_http._tcp",
];

fn encode_name(name: &str, out: &mut Vec<u8>) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

/// One mDNS query packet asking (with the unicast-response bit) for every type above.
pub fn mdns_query() -> Vec<u8> {
    let mut packet = vec![0, 0, 0, 0, 0, MDNS_TYPES.len() as u8, 0, 0, 0, 0, 0, 0];
    for kind in MDNS_TYPES {
        encode_name(&format!("{kind}.local"), &mut packet);
        packet.extend_from_slice(&[0, 12, 0x80, 1]); // PTR, IN with the QU bit
    }
    packet
}

/// A DNS name at `offset`, following compression pointers (bounded), and the offset after it.
fn read_name(packet: &[u8], mut offset: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let length = *packet.get(offset)? as usize;
        if length == 0 {
            return Some((labels.join("."), end.unwrap_or(offset + 1)));
        }
        if length & 0xC0 == 0xC0 {
            let pointer = ((length & 0x3F) << 8) | *packet.get(offset + 1)? as usize;
            end.get_or_insert(offset + 2);
            offset = pointer;
            continue;
        }
        let label = packet.get(offset + 1..offset + 1 + length)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        offset += 1 + length;
    }
    None
}

fn strip_local(name: &str) -> &str {
    name.strip_suffix(".local").unwrap_or(name)
}

/// Records of one mDNS response, applied to the announcement of the device they describe.
///
/// Routers that reflect mDNS between VLANs (UniFi, Avahi's reflector) resend other devices'
/// answers from their own address, so the sender is not necessarily the owner. Each service is
/// credited to the address its SRV target resolves to through the A records in the same packet;
/// only when the packet does not say otherwise does it fall back to the sender.
pub fn parse_mdns(packet: &[u8], source: Ipv4Addr, found: &mut HashMap<Ipv4Addr, Announcement>) {
    if packet.len() < 12 || packet[2] & 0x80 == 0 {
        return; // not a response
    }
    let count = |i: usize| u16::from_be_bytes([packet[i], packet[i + 1]]) as usize;
    let (questions, total) = (count(4), count(6) + count(8) + count(10));
    let mut offset = 12;
    for _ in 0..questions.min(64) {
        let Some((_, next)) = read_name(packet, offset) else {
            return;
        };
        offset = next + 4;
    }
    // (owner, type, rdata offset, rdata)
    let mut records: Vec<(String, u16, usize, &[u8])> = Vec::new();
    for _ in 0..total.min(256) {
        let Some((owner, next)) = read_name(packet, offset) else {
            break;
        };
        let Some(header) = packet.get(next..next + 10) else {
            break;
        };
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = u16::from_be_bytes([header[8], header[9]]) as usize;
        let start = next + 10;
        let Some(data) = packet.get(start..start + length) else {
            break;
        };
        offset = start + length;
        records.push((owner, kind, start, data));
    }

    let mut hosts: HashMap<String, Ipv4Addr> = HashMap::new();
    let mut targets: HashMap<String, String> = HashMap::new();
    for (owner, kind, start, data) in &records {
        match kind {
            1 if data.len() == 4 => {
                let address = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                if !address.is_unspecified() {
                    hosts.insert(owner.to_ascii_lowercase(), address);
                }
            }
            33 if data.len() > 6 => {
                if let Some((target, _)) = read_name(packet, start + 6) {
                    targets.insert(owner.to_ascii_lowercase(), target.to_ascii_lowercase());
                }
            }
            _ => {}
        }
    }
    let mut addresses: Vec<Ipv4Addr> = hosts.values().copied().collect();
    addresses.sort();
    addresses.dedup();
    // Without an SRV link: the packet's only address, else the sender unless the packet names
    // other addresses but not the sender's (a reflected answer that cannot be placed).
    let fallback = match addresses.as_slice() {
        [only] => Some(*only),
        [] => Some(source),
        many => many.contains(&source).then_some(source),
    };
    let owner_of = |instance: &str| {
        targets
            .get(&instance.to_ascii_lowercase())
            .and_then(|target| hosts.get(target))
            .copied()
            .or(fallback)
    };
    let admit = |address: Ipv4Addr, found: &mut HashMap<Ipv4Addr, Announcement>| {
        (found.len() < MAX_DEVICES || found.contains_key(&address)).then(|| {
            let entry = found.entry(address).or_default();
            entry.mdns_seen = true;
        })
    };
    // The sender answered this sweep, so whatever it announced before is now known to be stale.
    admit(source, found);

    for (owner, kind, start, data) in &records {
        match kind {
            1 if data.len() == 4 => {
                let address = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                if !address.is_unspecified() && admit(address, found).is_some() {
                    found.entry(address).or_default().mdns_name = clean(strip_local(owner), 80);
                }
            }
            12 => {
                let Some((target, _)) = read_name(packet, *start) else {
                    continue;
                };
                if owner.starts_with("_services._dns-sd._udp") {
                    let Some(device) = fallback.filter(|d| admit(*d, found).is_some()) else {
                        continue;
                    };
                    let entry = found.entry(device).or_default();
                    if entry.mdns_services.len() < 32
                        && let Some(kind) = clean(strip_local(&target), 60)
                    {
                        entry.mdns_services.insert(kind);
                    }
                } else if let Some(kind) = clean(strip_local(owner), 60) {
                    let Some(device) = owner_of(&target).filter(|d| admit(*d, found).is_some())
                    else {
                        continue;
                    };
                    let entry = found.entry(device).or_default();
                    if entry.mdns_services.len() < 32 {
                        entry.mdns_services.insert(kind.clone());
                    }
                    let instance = target
                        .strip_suffix(&format!(".{owner}"))
                        .and_then(|i| clean(i, 80));
                    if let Some(instance) = instance
                        && entry.mdns_instances.len() < 8
                    {
                        entry.mdns_instances.insert(instance);
                    }
                }
            }
            16 => {
                let Some(device) = owner_of(owner).filter(|d| admit(*d, found).is_some()) else {
                    continue;
                };
                let entry = found.entry(device).or_default();
                let mut index = 0;
                while index < data.len() {
                    let size = data[index] as usize;
                    let Some(text) = data.get(index + 1..index + 1 + size) else {
                        break;
                    };
                    index += 1 + size;
                    let text = String::from_utf8_lossy(text);
                    if let Some((key, value)) = text.split_once('=')
                        && matches!(
                            key.to_ascii_lowercase().as_str(),
                            "model" | "md" | "ty" | "mdl" | "am" | "modelid"
                        )
                        && entry.mdns_model.is_none()
                    {
                        entry.mdns_model = clean(value, 80);
                    }
                }
            }
            _ => {}
        }
    }
}

/// UDP socket for mDNS: port 5353 joined to the group when available (so multicast answers are
/// heard too), otherwise an ephemeral port that receives legacy unicast answers.
fn mdns_socket() -> Option<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let group = Ipv4Addr::new(224, 0, 0, 251);
    let shared = (|| -> std::io::Result<Socket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 5353)).into())?;
        socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)?;
        Ok(socket)
    })();
    let socket = match shared {
        Ok(socket) => socket,
        Err(_) => {
            let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
            socket
                .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())
                .ok()?;
            socket
        }
    };
    socket.set_multicast_ttl_v4(255).ok()?;
    socket.set_nonblocking(true).ok()?;
    UdpSocket::from_std(socket.into()).ok()
}

/// Collect what devices on the local link announce over mDNS within `wait`.
pub async fn mdns_sweep(wait: Duration) -> HashMap<Ipv4Addr, Announcement> {
    let mut found = HashMap::new();
    let Some(socket) = mdns_socket() else {
        return found;
    };
    let query = mdns_query();
    let target = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);
    let deadline = Instant::now() + wait;
    let mut buffer = vec![0u8; 9000];
    for attempt in 0..2 {
        let _ = socket.send_to(&query, target).await;
        let until = if attempt == 0 {
            Instant::now() + wait / 3
        } else {
            deadline
        };
        while let Ok(Ok((size, from))) =
            tokio::time::timeout_at(until, socket.recv_from(&mut buffer)).await
        {
            if let IpAddr::V4(source) = from.ip() {
                parse_mdns(&buffer[..size], source, &mut found);
            }
        }
    }
    found
}

/// Headers of an SSDP response as lower-case name to value.
fn ssdp_headers(packet: &str) -> HashMap<String, String> {
    packet
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect()
}

/// First text of `<tag>` in a UPnP description.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    clean(xml.get(start..end)?, 120)
}

/// Collect SSDP announcements within `wait`, then read each device's UPnP description (only from
/// the address that answered, plain HTTP, 64 KiB at most).
pub async fn ssdp_sweep(
    client: &reqwest::Client,
    wait: Duration,
) -> HashMap<Ipv4Addr, Announcement> {
    let mut found: HashMap<Ipv4Addr, Announcement> = HashMap::new();
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return found;
    };
    let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\nUSER-AGENT: H.O.P.E/1 UPnP/1.1 discovery/1\r\n\r\n";
    let target = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
    let deadline = Instant::now() + wait;
    let mut buffer = vec![0u8; 4096];
    for attempt in 0..2 {
        let _ = socket.send_to(search.as_bytes(), target).await;
        let until = if attempt == 0 {
            Instant::now() + wait / 3
        } else {
            deadline
        };
        while let Ok(Ok((size, from))) =
            tokio::time::timeout_at(until, socket.recv_from(&mut buffer)).await
        {
            let IpAddr::V4(source) = from.ip() else {
                continue;
            };
            if found.len() >= MAX_DEVICES && !found.contains_key(&source) {
                continue;
            }
            let headers = ssdp_headers(&String::from_utf8_lossy(&buffer[..size]));
            let entry = found.entry(source).or_default();
            if entry.ssdp_server.is_none() {
                entry.ssdp_server = headers.get("server").and_then(|v| clean(v, 120));
            }
            let st = headers.get("st").map(String::as_str).unwrap_or_default();
            if st.contains("InternetGatewayDevice") {
                entry.upnp_device_type = Some("InternetGatewayDevice".into());
            }
            if let Some(location) = headers.get("location")
                && url::Url::parse(location).is_ok_and(|u| {
                    u.scheme() == "http" && u.host_str() == Some(source.to_string().as_str())
                })
                && (entry.location.is_none() || st == "upnp:rootdevice")
            {
                entry.location = Some(location.clone());
            }
        }
    }
    let fetches = found
        .iter()
        .filter_map(|(ip, a)| a.location.clone().map(|l| (*ip, l)))
        .take(64)
        .map(|(ip, location)| async move {
            let response = client.get(&location).send().await.ok()?;
            let mut response = response;
            let mut body = Vec::new();
            while body.len() < 65536 {
                match tokio::time::timeout(Duration::from_secs(2), response.chunk()).await {
                    Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
                    _ => break,
                }
            }
            Some((ip, String::from_utf8_lossy(&body).into_owned()))
        });
    for (ip, xml) in futures::future::join_all(fetches)
        .await
        .into_iter()
        .flatten()
    {
        let entry = found.entry(ip).or_default();
        let device_type =
            xml_text(&xml, "deviceType").map(|t| t.split(':').nth(3).unwrap_or(&t).to_string());
        if entry.upnp_device_type.is_none() {
            entry.upnp_device_type = device_type;
        }
        entry.upnp_friendly_name = xml_text(&xml, "friendlyName");
        entry.upnp_manufacturer = xml_text(&xml, "manufacturer");
        entry.upnp_model = xml_text(&xml, "modelName");
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(owner: &str, kind: u16, data: &[u8], out: &mut Vec<u8>) {
        encode_name(owner, out);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&[0x80, 1, 0, 0, 0, 120]);
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
    }

    #[test]
    fn mdns_response_yields_name_services_instance_and_model() {
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0];
        let mut instance = Vec::new();
        encode_name(
            "BOSCH-SMV68TX06E-68A40E1234._homeconnect._tcp.local",
            &mut instance,
        );
        record("_homeconnect._tcp.local", 12, &instance, &mut packet);
        let mut kind = Vec::new();
        encode_name("_homeconnect._tcp.local", &mut kind);
        record("_services._dns-sd._udp.local", 12, &kind, &mut packet);
        let txt = b"\x0btype=Washer\x10model=SMV68TX06E";
        record(
            "BOSCH-SMV68TX06E-68A40E1234._homeconnect._tcp.local",
            16,
            txt,
            &mut packet,
        );
        record("bosch-dishwasher.local", 1, &[192, 168, 1, 40], &mut packet);
        let mut found = HashMap::new();
        let source = Ipv4Addr::new(192, 168, 1, 40);
        parse_mdns(&packet, source, &mut found);
        let device = &found[&source];
        assert_eq!(device.mdns_name.as_deref(), Some("bosch-dishwasher"));
        assert!(device.mdns_services.contains("_homeconnect._tcp"));
        assert!(
            device
                .mdns_instances
                .contains("BOSCH-SMV68TX06E-68A40E1234")
        );
        assert_eq!(device.mdns_model.as_deref(), Some("SMV68TX06E"));
    }

    #[test]
    fn reflected_mdns_is_credited_to_the_device_not_the_router() {
        // A UniFi gateway at .1 reflecting a camera's answer from another VLAN.
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0];
        let mut instance = Vec::new();
        encode_name("IndoorCam 2K-1711._hap._tcp.local", &mut instance);
        record("_hap._tcp.local", 12, &instance, &mut packet);
        let mut srv = vec![0, 0, 0, 0, 0x1F, 0x90];
        encode_name("indoorcam.local", &mut srv);
        record("IndoorCam 2K-1711._hap._tcp.local", 33, &srv, &mut packet);
        record(
            "IndoorCam 2K-1711._hap._tcp.local",
            16,
            b"\x08md=T8400",
            &mut packet,
        );
        record("indoorcam.local", 1, &[192, 168, 20, 7], &mut packet);
        let mut found = HashMap::new();
        let router = Ipv4Addr::new(192, 168, 1, 1);
        let camera = Ipv4Addr::new(192, 168, 20, 7);
        parse_mdns(&packet, router, &mut found);
        assert!(found[&router].mdns_seen);
        assert!(found[&router].mdns_instances.is_empty());
        assert!(found[&router].mdns_services.is_empty());
        assert_eq!(found[&router].mdns_model, None);
        assert!(found[&camera].mdns_instances.contains("IndoorCam 2K-1711"));
        assert!(found[&camera].mdns_services.contains("_hap._tcp"));
        assert_eq!(found[&camera].mdns_model.as_deref(), Some("T8400"));

        // A reflected answer naming two devices without SRV links is credited to neither.
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 0];
        record("_hap._tcp.local", 12, &instance, &mut packet);
        record("a.local", 1, &[192, 168, 20, 8], &mut packet);
        record("b.local", 1, &[192, 168, 20, 9], &mut packet);
        let mut found = HashMap::new();
        parse_mdns(&packet, router, &mut found);
        assert!(found.values().all(|a| a.mdns_instances.is_empty()));
    }

    #[test]
    fn malformed_mdns_never_panics_and_compression_loops_stop() {
        let mut found = HashMap::new();
        let source = Ipv4Addr::new(10, 0, 0, 2);
        // A pointer to itself, truncated records, and random bytes.
        let looping = [0u8, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xC0, 12];
        parse_mdns(&looping, source, &mut found);
        for length in 0..40 {
            parse_mdns(
                &mdns_query()[..length.min(mdns_query().len())],
                source,
                &mut found,
            );
        }
        let noise: Vec<u8> = (0..600u32).map(|i| (i * 7919 % 251) as u8).collect();
        for start in 0..50 {
            let mut packet = vec![0, 0, 0x84, 0, 0, 1, 0, 9, 0, 9, 0, 9];
            packet.extend_from_slice(&noise[start..]);
            parse_mdns(&packet, source, &mut found);
        }
    }

    #[test]
    fn titles_and_ssdp_headers_are_extracted_and_bounded() {
        assert_eq!(
            title("<html><head><TITLE>Proxmox Virtual Environment</TITLE>").as_deref(),
            Some("Proxmox Virtual Environment")
        );
        assert_eq!(
            title("<title lang=en>A &amp; B\n</title>").as_deref(),
            Some("A & B")
        );
        assert_eq!(title("no title here"), None);
        assert_eq!(
            title(&format!("<title>{}</title>", "x".repeat(500))).map(|t| t.len()),
            Some(120)
        );
        let headers = ssdp_headers(
            "HTTP/1.1 200 OK\r\nSERVER: Linux UPnP/1.0 Sonos/70.3\r\nST: upnp:rootdevice\r\nLOCATION: http://192.168.1.9:1400/xml\r\n\r\n",
        );
        assert_eq!(headers["server"], "Linux UPnP/1.0 Sonos/70.3");
        assert_eq!(headers["location"], "http://192.168.1.9:1400/xml");
        assert_eq!(
            xml_text("<root><device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType><friendlyName>Router</friendlyName>", "friendlyName").as_deref(),
            Some("Router")
        );
        assert_eq!(clean("a\u{0}b\tc", 10).as_deref(), Some("a b c"));
        assert_eq!(port_name(8006), Some("Proxmox VE"));
    }
}
