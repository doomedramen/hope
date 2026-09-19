//! Bounded mDNS/DNS-SD and UPnP/SSDP service observations.
//!
//! This module parses datagrams and defines the only multicast destinations
//! that a future network collector may use. It never follows an advertised
//! URL, sends credentials, or turns a response into a unicast probe.
//
// The parser/target boundary is intentionally exposed before a job
// orchestrator and persistence adapter wire it into discovery. Keep the
// standalone seam lint-clean until that integration lands.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};

pub const MDNS_IPV4_MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_IPV6_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);
pub const MDNS_PORT: u16 = 5353;
pub const SSDP_IPV4_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const SSDP_IPV6_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x000c);
pub const SSDP_PORT: u16 = 1900;

const MAX_DNS_POINTER_HOPS: usize = 32;
const MAX_DNS_LABEL_BYTES: usize = 63;

/// The only multicast protocols supported by this collector boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorProtocol {
    Mdns,
    Ssdp,
}

impl CollectorProtocol {
    pub fn targets(self) -> [SocketAddr; 2] {
        match self {
            Self::Mdns => [
                SocketAddr::new(IpAddr::V4(MDNS_IPV4_MULTICAST), MDNS_PORT),
                SocketAddr::new(IpAddr::V6(MDNS_IPV6_MULTICAST), MDNS_PORT),
            ],
            Self::Ssdp => [
                SocketAddr::new(IpAddr::V4(SSDP_IPV4_MULTICAST), SSDP_PORT),
                SocketAddr::new(IpAddr::V6(SSDP_IPV6_MULTICAST), SSDP_PORT),
            ],
        }
    }

    pub fn allows_target(self, target: SocketAddr) -> bool {
        self.targets().contains(&target)
    }
}

/// Bounds shared by parsers and a future socket collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorConfig {
    pub receive_window: Duration,
    pub max_datagram_bytes: usize,
    pub max_records: usize,
    pub max_headers: usize,
    pub max_txt_entries: usize,
    pub max_text_bytes: usize,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            receive_window: Duration::from_secs(3),
            max_datagram_bytes: 16 * 1024,
            max_records: 128,
            max_headers: 64,
            max_txt_entries: 32,
            max_text_bytes: 1024,
        }
    }
}

impl CollectorConfig {
    fn validate(self) -> Result<Self, CollectorError> {
        if self.receive_window.is_zero() || self.receive_window > Duration::from_secs(60) {
            return Err(CollectorError::InvalidConfig(
                "receive window must be greater than zero and at most 60s",
            ));
        }
        if self.max_datagram_bytes == 0 || self.max_datagram_bytes > 65_535 {
            return Err(CollectorError::InvalidConfig(
                "datagram bound must be between 1 and 65535 bytes",
            ));
        }
        if self.max_records == 0 || self.max_headers == 0 || self.max_txt_entries == 0 {
            return Err(CollectorError::InvalidConfig(
                "record, header, and TXT bounds must be greater than zero",
            ));
        }
        if self.max_text_bytes == 0 {
            return Err(CollectorError::InvalidConfig(
                "text bound must be greater than zero",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CollectorError {
    #[error("invalid collector configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("datagram is larger than the configured bound")]
    DatagramTooLarge,
    #[error("collector limit exceeded: {0}")]
    Limit(&'static str),
    #[error("collector socket error: {0}")]
    Socket(String),
    #[error("malformed service datagram: {0}")]
    Malformed(String),
}

/// One normalized DNS-SD service advertisement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsService {
    pub instance: String,
    pub service_type: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub addresses: Vec<IpAddr>,
    pub txt: BTreeMap<String, String>,
}

/// The message kind retained from an SSDP datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsdpMessageKind {
    Response,
    Notification,
}

/// One normalized UPnP/SSDP advertisement. `location` is retained as an
/// observation only; callers must not fetch it as part of collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpService {
    pub kind: SsdpMessageKind,
    pub usn: String,
    pub search_target: Option<String>,
    pub notification_type: Option<String>,
    pub location: Option<String>,
    pub server: Option<String>,
    pub cache_control: Option<String>,
}

/// One parsed observation received from a multicast collector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectedService {
    Mdns {
        source: SocketAddr,
        service: MdnsService,
    },
    Ssdp {
        source: SocketAddr,
        service: SsdpService,
    },
}

/// Query a single fixed multicast protocol on one explicitly selected IPv4
/// interface. The caller owns network-scope authorization; this function
/// never accepts a destination or follows an advertised URL.
pub async fn collect_multicast(
    protocol: CollectorProtocol,
    interface: Ipv4Addr,
    config: CollectorConfig,
) -> Result<Vec<CollectedService>, CollectorError> {
    let config = config.validate()?;
    let target = match protocol {
        CollectorProtocol::Mdns => SocketAddr::new(IpAddr::V4(MDNS_IPV4_MULTICAST), MDNS_PORT),
        CollectorProtocol::Ssdp => SocketAddr::new(IpAddr::V4(SSDP_IPV4_MULTICAST), SSDP_PORT),
    };
    if !protocol.allows_target(target) {
        return Err(CollectorError::InvalidConfig(
            "collector target is not an allowlisted multicast destination",
        ));
    }
    let multicast = match target {
        SocketAddr::V4(target) => *target.ip(),
        SocketAddr::V6(_) => {
            return Err(CollectorError::InvalidConfig(
                "IPv6 multicast collection is not enabled",
            ));
        }
    };

    let socket = UdpSocket::bind(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        target.port(),
    ))
    .await
    .map_err(|error| CollectorError::Socket(error.to_string()))?;
    socket
        .join_multicast_v4(multicast, interface)
        .map_err(|error| CollectorError::Socket(error.to_string()))?;
    socket
        .set_multicast_loop_v4(false)
        .map_err(|error| CollectorError::Socket(error.to_string()))?;

    let query = match protocol {
        CollectorProtocol::Mdns => mdns_query(),
        CollectorProtocol::Ssdp => ssdp_query(),
    };
    socket
        .send_to(&query, target)
        .await
        .map_err(|error| CollectorError::Socket(error.to_string()))?;

    let deadline = Instant::now() + config.receive_window;
    let mut datagram = vec![0; config.max_datagram_bytes.saturating_add(1)];
    let mut observations = Vec::new();
    loop {
        let received = match timeout_at(deadline, socket.recv_from(&mut datagram)).await {
            Ok(Ok(received)) => received,
            Ok(Err(error)) => return Err(CollectorError::Socket(error.to_string())),
            Err(_) => break,
        };
        let (length, source) = received;
        if length > config.max_datagram_bytes {
            return Err(CollectorError::DatagramTooLarge);
        }
        match protocol {
            CollectorProtocol::Mdns => {
                for service in parse_mdns_packet(&datagram[..length], config)? {
                    observations.push(CollectedService::Mdns { source, service });
                }
            }
            CollectorProtocol::Ssdp => {
                let service = parse_ssdp_datagram(&datagram[..length], config)?;
                observations.push(CollectedService::Ssdp { source, service });
            }
        }
        if observations.len() > config.max_records {
            return Err(CollectorError::Limit("collector observation count"));
        }
    }
    Ok(observations)
}

fn mdns_query() -> Vec<u8> {
    let mut query = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    query.extend_from_slice(&dns_name_bytes("_services._dns-sd._udp.local"));
    query.extend_from_slice(&12u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    query
}

fn dns_name_bytes(name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for label in name.split('.') {
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.push(0);
    bytes
}

fn ssdp_query() -> Vec<u8> {
    b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: ssdp:all\r\n\r\n".to_vec()
}

fn checked_datagram(
    bytes: &[u8],
    config: CollectorConfig,
) -> Result<(&[u8], CollectorConfig), CollectorError> {
    let config = config.validate()?;
    if bytes.len() > config.max_datagram_bytes {
        return Err(CollectorError::DatagramTooLarge);
    }
    Ok((bytes, config))
}

/// Parse a bounded DNS response containing DNS-SD PTR/SRV/TXT/A/AAAA records.
pub fn parse_mdns_packet(
    bytes: &[u8],
    config: CollectorConfig,
) -> Result<Vec<MdnsService>, CollectorError> {
    let (bytes, config) = checked_datagram(bytes, config)?;
    if bytes.len() < 12 {
        return Err(malformed("mDNS header is truncated"));
    }

    let questions = read_u16(bytes, 4)? as usize;
    let answers = read_u16(bytes, 6)? as usize;
    let authorities = read_u16(bytes, 8)? as usize;
    let additionals = read_u16(bytes, 10)? as usize;
    let record_count = answers
        .checked_add(authorities)
        .and_then(|count| count.checked_add(additionals))
        .ok_or(CollectorError::Limit("DNS record count"))?;
    if record_count > config.max_records {
        return Err(CollectorError::Limit("DNS record count"));
    }

    let mut cursor = 12;
    for _ in 0..questions {
        let (_, next) = read_dns_name(bytes, cursor, config.max_text_bytes)?;
        cursor = next
            .checked_add(4)
            .ok_or_else(|| malformed("mDNS question is truncated"))?;
        if cursor > bytes.len() {
            return Err(malformed("mDNS question is truncated"));
        }
    }

    let mut ptr_records = Vec::new();
    let mut srv_records: HashMap<String, (String, u16)> = HashMap::new();
    let mut txt_records: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    let mut addresses: HashMap<String, Vec<IpAddr>> = HashMap::new();

    for _ in 0..record_count {
        let (name, next) = read_dns_name(bytes, cursor, config.max_text_bytes)?;
        cursor = next;
        if cursor.checked_add(10).is_none_or(|end| end > bytes.len()) {
            return Err(malformed("mDNS resource record header is truncated"));
        }
        let record_type = read_u16_at(bytes, cursor)?;
        let record_class = read_u16_at(bytes, cursor + 2)?;
        let data_length = read_u16_at(bytes, cursor + 8)? as usize;
        cursor += 10;
        let data_end = cursor
            .checked_add(data_length)
            .ok_or_else(|| malformed("mDNS resource record length overflows"))?;
        if data_end > bytes.len() {
            return Err(malformed("mDNS resource record data is truncated"));
        }
        let key = name.to_ascii_lowercase();
        match record_type {
            1 if data_length == 4 => {
                let address = IpAddr::V4(Ipv4Addr::new(
                    bytes[cursor],
                    bytes[cursor + 1],
                    bytes[cursor + 2],
                    bytes[cursor + 3],
                ));
                addresses.entry(key).or_default().push(address);
            }
            28 if data_length == 16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&bytes[cursor..data_end]);
                addresses
                    .entry(key)
                    .or_default()
                    .push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            12 => {
                let (target, target_end) = read_dns_name(bytes, cursor, config.max_text_bytes)?;
                if target_end > data_end {
                    return Err(malformed("mDNS PTR name escapes its record"));
                }
                ptr_records.push((name, target));
            }
            33 if data_length >= 6 => {
                let port = read_u16_at(bytes, cursor + 4)?;
                let (target, target_end) = read_dns_name(bytes, cursor + 6, config.max_text_bytes)?;
                if target_end > data_end {
                    return Err(malformed("mDNS SRV name escapes its record"));
                }
                if record_class & 0x7fff != 1 {
                    cursor = data_end;
                    continue;
                }
                srv_records.insert(key, (target, port));
            }
            16 => {
                let txt = parse_txt(bytes, cursor, data_end, config)?;
                txt_records.insert(key, txt);
            }
            _ => {}
        }
        cursor = data_end;
    }

    let mut services = Vec::new();
    let mut seen = HashSet::new();
    for (service_type, instance) in ptr_records {
        if !is_service_type(&service_type) {
            continue;
        }
        let identity = format!(
            "{}\u{0}{}",
            service_type.to_ascii_lowercase(),
            instance.to_ascii_lowercase()
        );
        if !seen.insert(identity) {
            continue;
        }
        let instance_key = instance.to_ascii_lowercase();
        let (hostname, port) = srv_records
            .get(&instance_key)
            .map(|(hostname, port)| (Some(hostname.clone()), Some(*port)))
            .unwrap_or((None, None));
        let service_addresses = hostname
            .as_ref()
            .and_then(|hostname| addresses.get(&hostname.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_default();
        let txt = txt_records.remove(&instance_key).unwrap_or_default();
        services.push(MdnsService {
            instance,
            service_type,
            hostname,
            port,
            addresses: service_addresses,
            txt,
        });
        if services.len() >= config.max_records {
            return Err(CollectorError::Limit("DNS-SD service count"));
        }
    }
    Ok(services)
}

fn parse_txt(
    bytes: &[u8],
    mut cursor: usize,
    end: usize,
    config: CollectorConfig,
) -> Result<BTreeMap<String, String>, CollectorError> {
    let mut entries = BTreeMap::new();
    while cursor < end {
        let length = bytes[cursor] as usize;
        cursor += 1;
        if entries.len() >= config.max_txt_entries {
            return Err(CollectorError::Limit("DNS-SD TXT entry count"));
        }
        let entry_end = cursor
            .checked_add(length)
            .ok_or_else(|| malformed("DNS-SD TXT length overflows"))?;
        if entry_end > end {
            return Err(malformed("DNS-SD TXT entry is truncated"));
        }
        let text = bounded_text(&bytes[cursor..entry_end], config.max_text_bytes)?;
        let (key, value) = text.split_once('=').unwrap_or((text.as_str(), ""));
        if key.is_empty() {
            return Err(malformed("DNS-SD TXT key is empty"));
        }
        entries.insert(key.to_string(), value.to_string());
        cursor = entry_end;
    }
    Ok(entries)
}

/// Parse one bounded UPnP/SSDP response or NOTIFY advertisement.
pub fn parse_ssdp_datagram(
    bytes: &[u8],
    config: CollectorConfig,
) -> Result<SsdpService, CollectorError> {
    let (bytes, config) = checked_datagram(bytes, config)?;
    let text = std::str::from_utf8(bytes).map_err(|_| malformed("SSDP is not UTF-8"))?;
    let mut lines = text.split("\r\n");
    let start_line = lines
        .next()
        .ok_or_else(|| malformed("SSDP start line is missing"))?;
    let kind = if start_line.eq_ignore_ascii_case("HTTP/1.1 200 OK") {
        SsdpMessageKind::Response
    } else if start_line.eq_ignore_ascii_case("NOTIFY * HTTP/1.1") {
        SsdpMessageKind::Notification
    } else {
        return Err(malformed("unsupported SSDP start line"));
    };

    let mut headers = BTreeMap::new();
    let mut header_count = 0;
    for line in lines {
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > config.max_headers {
            return Err(CollectorError::Limit("SSDP header count"));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| malformed("SSDP header is missing a colon"))?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name.is_empty() || value.len() > config.max_text_bytes {
            return Err(malformed("SSDP header is invalid or too long"));
        }
        if matches!(
            name.as_str(),
            "cache-control" | "location" | "server" | "st" | "nt" | "usn"
        ) {
            headers.entry(name).or_insert_with(|| value.to_string());
        }
    }

    let usn = headers
        .remove("usn")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed("SSDP USN header is missing"))?;
    Ok(SsdpService {
        kind,
        usn,
        search_target: headers.remove("st"),
        notification_type: headers.remove("nt"),
        location: headers.remove("location"),
        server: headers.remove("server"),
        cache_control: headers.remove("cache-control"),
    })
}

fn is_service_type(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 3
        && labels[0].starts_with('_')
        && matches!(labels[1], "_tcp" | "_udp")
        && labels
            .last()
            .is_some_and(|label| label.eq_ignore_ascii_case("local"))
}

fn read_dns_name(
    bytes: &[u8],
    start: usize,
    max_text_bytes: usize,
) -> Result<(String, usize), CollectorError> {
    let mut cursor = start;
    let mut next_offset = None;
    let mut labels = Vec::new();
    let mut visited = HashSet::new();
    for _ in 0..=MAX_DNS_POINTER_HOPS {
        if cursor >= bytes.len() || !visited.insert(cursor) {
            return Err(malformed("mDNS name pointer is invalid"));
        }
        let length = bytes[cursor];
        if length & 0xc0 == 0xc0 {
            if cursor + 1 >= bytes.len() {
                return Err(malformed("mDNS name pointer is truncated"));
            }
            let pointer = ((length as usize & 0x3f) << 8) | bytes[cursor + 1] as usize;
            if pointer >= bytes.len() {
                return Err(malformed("mDNS name pointer is out of bounds"));
            }
            next_offset.get_or_insert(cursor + 2);
            cursor = pointer;
            continue;
        }
        if length & 0xc0 != 0 {
            return Err(malformed("mDNS label has an invalid length"));
        }
        let length = length as usize;
        cursor += 1;
        if length == 0 {
            return Ok((labels.join("."), next_offset.unwrap_or(cursor)));
        }
        if length > MAX_DNS_LABEL_BYTES || cursor + length > bytes.len() {
            return Err(malformed("mDNS label is invalid or truncated"));
        }
        let label = bounded_text(&bytes[cursor..cursor + length], max_text_bytes)?;
        labels.push(label);
        cursor += length;
        if labels.join(".").len() > max_text_bytes {
            return Err(CollectorError::Limit("mDNS name length"));
        }
    }
    Err(malformed("mDNS name pointer depth exceeded"))
}

fn bounded_text(bytes: &[u8], max_bytes: usize) -> Result<String, CollectorError> {
    if bytes.len() > max_bytes {
        return Err(CollectorError::Limit("text length"));
    }
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| malformed("service text is not UTF-8"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CollectorError> {
    read_u16_at(bytes, offset)
}

fn read_u16_at(bytes: &[u8], offset: usize) -> Result<u16, CollectorError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| malformed("integer offset overflows"))?;
    let pair = bytes
        .get(offset..end)
        .ok_or_else(|| malformed("integer is truncated"))?;
    Ok(u16::from_be_bytes([pair[0], pair[1]]))
}

fn malformed(message: &str) -> CollectorError {
    CollectorError::Malformed(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_targets_reject_unicast() {
        assert!(
            CollectorProtocol::Mdns
                .allows_target(SocketAddr::new(IpAddr::V4(MDNS_IPV4_MULTICAST), MDNS_PORT,))
        );
        assert!(!CollectorProtocol::Mdns.allows_target(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            MDNS_PORT,
        )));
    }

    #[test]
    fn parses_mdns_service_records() {
        let instance = dns_name("Plex._http._tcp.local");
        let hostname = dns_name("plex.local");
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 1];
        append_dns_record(&mut packet, "_http._tcp.local", 12, &instance);
        let mut srv_data = vec![0, 0, 0, 0, 0x23, 0x45];
        srv_data.extend_from_slice(&hostname);
        append_dns_record(&mut packet, "Plex._http._tcp.local", 33, &srv_data);
        append_dns_record(
            &mut packet,
            "Plex._http._tcp.local",
            16,
            &[6, b'p', b'a', b't', b'h', b'=', b'/'],
        );
        append_dns_record(&mut packet, "plex.local", 1, &[192, 0, 2, 10]);
        append_dns_record(&mut packet, "plex.local", 28, &[0; 16]);

        let parsed = parse_mdns_packet(&packet, CollectorConfig::default()).expect("parse mDNS");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].instance, "Plex._http._tcp.local");
        assert_eq!(parsed[0].service_type, "_http._tcp.local");
        assert_eq!(parsed[0].port, Some(9029));
        assert_eq!(
            parsed[0].addresses,
            vec![
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
                IpAddr::V6(Ipv6Addr::from([0; 16])),
            ]
        );
        assert_eq!(parsed[0].txt.get("path"), Some(&"/".to_string()));
    }

    #[test]
    fn rejects_oversized_mdns_datagram() {
        let config = CollectorConfig {
            max_datagram_bytes: 4,
            ..CollectorConfig::default()
        };
        assert_eq!(
            parse_mdns_packet(&[0; 5], config),
            Err(CollectorError::DatagramTooLarge)
        );
    }

    #[test]
    fn rejects_invalid_collector_bounds() {
        let config = CollectorConfig {
            receive_window: Duration::ZERO,
            ..CollectorConfig::default()
        };
        assert!(matches!(
            parse_mdns_packet(&[], config),
            Err(CollectorError::InvalidConfig(_))
        ));
    }

    #[test]
    fn builds_fixed_multicast_queries() {
        let query = mdns_query();
        assert_eq!(&query[..12], &[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert!(
            query
                .windows(b"_services".len())
                .any(|window| window == b"_services")
        );

        let query = ssdp_query();
        assert!(query.starts_with(b"M-SEARCH * HTTP/1.1\r\n"));
        assert!(
            query
                .windows(b"HOST: 239.255.255.250:1900".len())
                .any(|window| { window == b"HOST: 239.255.255.250:1900" })
        );
        assert!(query.ends_with(b"\r\n\r\n"));
    }

    #[test]
    fn parses_ssdp_response_without_following_location() {
        let packet = b"HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\nUSN: uuid:test::urn:schemas-upnp-org:device:MediaServer:1\r\nLOCATION: http://192.0.2.20:8200/device.xml\r\nSERVER: Hope/1.0\r\nCACHE-CONTROL: max-age=1800\r\n\r\n";
        let parsed = parse_ssdp_datagram(packet, CollectorConfig::default()).expect("parse SSDP");
        assert_eq!(parsed.kind, SsdpMessageKind::Response);
        assert_eq!(
            parsed.search_target.as_deref(),
            Some("urn:schemas-upnp-org:device:MediaServer:1")
        );
        assert_eq!(
            parsed.usn,
            "uuid:test::urn:schemas-upnp-org:device:MediaServer:1"
        );
        assert_eq!(
            parsed.location.as_deref(),
            Some("http://192.0.2.20:8200/device.xml")
        );
    }

    #[test]
    fn rejects_ssdp_without_usn() {
        let packet = b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\n\r\n";
        let error =
            parse_ssdp_datagram(packet, CollectorConfig::default()).expect_err("missing USN");
        assert!(matches!(error, CollectorError::Malformed(message) if message.contains("USN")));
    }

    fn dns_name(name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for label in name.split('.') {
            bytes.push(label.len() as u8);
            bytes.extend_from_slice(label.as_bytes());
        }
        bytes.push(0);
        bytes
    }

    fn append_dns_record(packet: &mut Vec<u8>, name: &str, record_type: u16, data: &[u8]) {
        let name = dns_name(name);
        packet.extend_from_slice(&name);
        packet.extend_from_slice(&record_type.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&120u32.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(data);
    }
}
