//! Device type guessing. Every signal adds points to one or more kinds; the kind with the most
//! points wins, its confidence follows the score and the margin over the runner-up, and the
//! signals behind it become the reasons shown in the console. Rules are data, so adding a vendor
//! or a keyword is one line.
use hope_protocol::{DeviceGuess, Host};
use std::collections::{HashMap, HashSet};

/// Every kind with its display name, in the order the console offers them.
pub const KINDS: [(&str, &str); 20] = [
    ("router", "Router"),
    ("switch", "Switch"),
    ("access_point", "Access point"),
    ("hypervisor", "Hypervisor"),
    ("nas", "NAS"),
    ("server", "Server"),
    ("vm", "Virtual machine"),
    ("computer", "Computer"),
    ("phone", "Phone"),
    ("tablet", "Tablet"),
    ("tv", "TV"),
    ("streamer", "Media streamer"),
    ("speaker", "Smart speaker"),
    ("printer", "Printer"),
    ("camera", "Camera"),
    ("vacuum", "Robot vacuum"),
    ("appliance", "Home appliance"),
    ("game_console", "Game console"),
    ("smart_home", "Smart home device"),
    ("unknown", "Unknown device"),
];

pub fn kind_label(kind: &str) -> Option<&'static str> {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, l)| *l)
}

/// Infrastructure whose disappearance is worth a notification.
pub fn is_infrastructure(kind: &str) -> bool {
    matches!(
        kind,
        "router" | "switch" | "access_point" | "hypervisor" | "nas" | "server"
    )
}

type Rule = (&'static str, &'static str, i32, Option<&'static str>);

/// Registered MAC vendor (substring, case-insensitive) → kind, points, specific label.
const VENDORS: &[Rule] = &[
    (
        "proxmox server solutions",
        "vm",
        7,
        Some("Proxmox virtual machine"),
    ),
    ("vmware", "vm", 7, None),
    ("xensource", "vm", 7, None),
    ("raspberry pi", "server", 5, Some("Raspberry Pi")),
    ("synology", "nas", 8, Some("Synology NAS")),
    ("qnap", "nas", 8, Some("QNAP NAS")),
    ("western digital", "nas", 3, None),
    ("super micro", "server", 5, None),
    ("ubiquiti", "access_point", 3, Some("UniFi")),
    ("ubiquiti", "switch", 2, Some("UniFi")),
    ("ubiquiti", "router", 2, Some("UniFi")),
    ("mikrotik", "router", 6, Some("MikroTik router")),
    ("routerboard", "router", 6, Some("MikroTik router")),
    ("netgear", "router", 2, None),
    ("netgear", "switch", 2, None),
    ("tp-link", "router", 1, None),
    ("tp-link", "smart_home", 1, None),
    ("avm", "router", 5, Some("FRITZ!Box")),
    ("cisco", "switch", 3, None),
    ("cisco", "router", 2, None),
    ("juniper", "switch", 3, None),
    ("aruba", "access_point", 5, None),
    ("ruckus", "access_point", 5, None),
    ("cambium", "access_point", 4, None),
    ("fortinet", "router", 6, Some("Fortinet firewall")),
    ("protectli", "router", 5, None),
    ("zyxel", "router", 2, None),
    ("zyxel", "switch", 2, None),
    ("espressif", "smart_home", 5, None),
    ("tuya", "smart_home", 6, None),
    ("shelly", "smart_home", 7, Some("Shelly")),
    ("allterco", "smart_home", 7, Some("Shelly")),
    ("signify", "smart_home", 6, Some("Philips Hue")),
    ("philips lighting", "smart_home", 6, Some("Philips Hue")),
    ("lifx", "smart_home", 7, None),
    ("nest labs", "smart_home", 5, None),
    ("ecobee", "smart_home", 7, Some("Thermostat")),
    ("sonos", "speaker", 9, Some("Sonos speaker")),
    ("bose", "speaker", 6, None),
    ("roku", "streamer", 8, Some("Roku")),
    ("google", "streamer", 2, None),
    ("google", "speaker", 2, None),
    ("amazon technologies", "speaker", 2, None),
    ("amazon technologies", "streamer", 2, None),
    ("apple", "phone", 1, None),
    ("apple", "computer", 1, None),
    ("samsung", "phone", 1, None),
    ("samsung", "tv", 1, None),
    ("lg electronics", "tv", 2, None),
    ("lg innotek", "appliance", 2, None),
    ("sony interactive", "game_console", 9, Some("PlayStation")),
    ("nintendo", "game_console", 9, Some("Nintendo console")),
    ("valve", "game_console", 6, Some("Steam device")),
    ("vizio", "tv", 7, None),
    ("tcl", "tv", 4, None),
    ("hisense", "tv", 5, None),
    ("hikvision", "camera", 8, None),
    ("dahua", "camera", 8, None),
    ("reolink", "camera", 9, None),
    ("axis communications", "camera", 8, None),
    ("amcrest", "camera", 8, None),
    ("wyze", "camera", 6, None),
    ("ring llc", "camera", 6, Some("Ring doorbell or camera")),
    ("brother", "printer", 8, None),
    ("seiko epson", "printer", 8, None),
    ("canon", "printer", 6, None),
    ("hp inc", "printer", 5, None),
    ("xerox", "printer", 8, None),
    ("lexmark", "printer", 8, None),
    ("kyocera", "printer", 8, None),
    ("ricoh", "printer", 7, None),
    ("irobot", "vacuum", 10, Some("Roomba")),
    ("roborock", "vacuum", 10, None),
    ("ecovacs", "vacuum", 10, None),
    ("dreame", "vacuum", 10, None),
    ("neato", "vacuum", 10, None),
    ("bsh hausger", "appliance", 9, None),
    ("miele", "appliance", 9, None),
    ("electrolux", "appliance", 7, None),
    ("whirlpool", "appliance", 7, None),
    ("haier", "appliance", 5, None),
    ("intel corporate", "computer", 2, None),
    ("intel corporate", "server", 1, None),
    ("micro-star", "computer", 2, None),
    ("gigabyte", "computer", 2, None),
    ("asrock", "computer", 2, None),
    ("dell", "computer", 2, None),
    ("dell", "server", 1, None),
    ("lenovo", "computer", 2, None),
    ("asustek", "computer", 1, None),
    ("asustek", "router", 1, None),
    ("liteon", "computer", 2, None),
    ("azurewave", "computer", 2, None),
];

/// Lower-case keywords in announced or resolved names → kind, points, label.
const NAMES: &[Rule] = &[
    ("iphone", "phone", 9, Some("iPhone")),
    ("ipad", "tablet", 9, Some("iPad")),
    ("android", "phone", 6, None),
    ("galaxy", "phone", 5, None),
    ("pixel", "phone", 5, None),
    ("macbook", "computer", 9, Some("Mac")),
    ("imac", "computer", 9, Some("Mac")),
    ("mac-mini", "computer", 9, Some("Mac")),
    ("macmini", "computer", 9, Some("Mac")),
    ("desktop", "computer", 5, None),
    ("laptop", "computer", 6, None),
    ("appletv", "streamer", 9, Some("Apple TV")),
    ("apple-tv", "streamer", 9, Some("Apple TV")),
    ("chromecast", "streamer", 9, Some("Chromecast")),
    ("roku", "streamer", 8, Some("Roku")),
    ("firetv", "streamer", 8, Some("Fire TV")),
    ("fire-tv", "streamer", 8, Some("Fire TV")),
    ("shield", "streamer", 5, Some("NVIDIA Shield")),
    ("homepod", "speaker", 9, Some("HomePod")),
    ("sonos", "speaker", 8, Some("Sonos speaker")),
    ("echo", "speaker", 5, Some("Amazon Echo")),
    ("nest-hub", "speaker", 7, Some("Nest Hub")),
    ("bravia", "tv", 8, None),
    ("webos", "tv", 8, Some("LG TV")),
    ("tizen", "tv", 7, Some("Samsung TV")),
    ("smart-tv", "tv", 8, None),
    ("tv", "tv", 7, None),
    ("printer", "printer", 7, None),
    ("laserjet", "printer", 9, Some("HP LaserJet")),
    ("officejet", "printer", 9, Some("HP OfficeJet")),
    ("deskjet", "printer", 9, Some("HP DeskJet")),
    ("epson", "printer", 6, None),
    ("octoprint", "printer", 9, Some("3D printer (OctoPrint)")),
    ("camera", "camera", 7, None),
    ("doorbell", "camera", 8, Some("Doorbell")),
    ("reolink", "camera", 8, None),
    ("roomba", "vacuum", 10, Some("Roomba")),
    ("roborock", "vacuum", 10, None),
    ("rockrobo", "vacuum", 10, None),
    ("vacuum", "vacuum", 10, None),
    ("deebot", "vacuum", 10, None),
    ("dreame", "vacuum", 9, None),
    ("dishwasher", "appliance", 10, Some("Dishwasher")),
    ("washing", "appliance", 9, Some("Washing machine")),
    ("washer", "appliance", 8, Some("Washing machine")),
    ("dryer", "appliance", 8, Some("Tumble dryer")),
    ("oven", "appliance", 7, Some("Oven")),
    ("fridge", "appliance", 8, Some("Fridge")),
    ("refrigerator", "appliance", 8, Some("Fridge")),
    ("coffee", "appliance", 7, Some("Coffee machine")),
    ("diskstation", "nas", 9, Some("Synology NAS")),
    ("truenas", "nas", 9, Some("TrueNAS")),
    ("unraid", "nas", 9, Some("Unraid")),
    ("nas", "nas", 5, None),
    ("proxmox", "hypervisor", 7, Some("Proxmox VE")),
    ("pve", "hypervisor", 5, Some("Proxmox VE")),
    ("esxi", "hypervisor", 7, Some("VMware ESXi")),
    ("opnsense", "router", 9, Some("OPNsense")),
    ("pfsense", "router", 9, Some("pfSense")),
    ("openwrt", "router", 8, Some("OpenWrt router")),
    ("fritz", "router", 8, Some("FRITZ!Box")),
    ("router", "router", 7, None),
    ("gateway", "router", 5, None),
    ("firewall", "router", 6, None),
    ("udm", "router", 6, Some("UniFi Dream Machine")),
    ("ucg", "router", 6, Some("UniFi Cloud Gateway")),
    ("usg", "router", 5, Some("UniFi Security Gateway")),
    ("usw", "switch", 7, Some("UniFi switch")),
    ("switch", "switch", 4, None),
    ("uap", "access_point", 7, Some("UniFi access point")),
    ("u6-", "access_point", 7, Some("UniFi access point")),
    ("u7-", "access_point", 7, Some("UniFi access point")),
    ("access-point", "access_point", 7, None),
    ("homeassistant", "smart_home", 9, Some("Home Assistant")),
    ("home-assistant", "smart_home", 9, Some("Home Assistant")),
    ("hue-bridge", "smart_home", 8, Some("Hue Bridge")),
    ("philips-hue", "smart_home", 8, Some("Hue Bridge")),
    ("esphome", "smart_home", 8, Some("ESPHome device")),
    ("shelly", "smart_home", 8, Some("Shelly")),
    ("tasmota", "smart_home", 8, Some("Tasmota device")),
    ("wled", "smart_home", 8, Some("WLED light")),
    ("thermostat", "smart_home", 8, Some("Thermostat")),
    ("xbox", "game_console", 9, Some("Xbox")),
    ("playstation", "game_console", 9, Some("PlayStation")),
    ("ps5", "game_console", 8, Some("PlayStation")),
    ("ps4", "game_console", 8, Some("PlayStation")),
    ("nintendo", "game_console", 9, Some("Nintendo console")),
    ("steamdeck", "game_console", 8, Some("Steam Deck")),
    ("raspberrypi", "server", 6, Some("Raspberry Pi")),
    ("pihole", "server", 7, Some("Pi-hole")),
    ("pi-hole", "server", 7, Some("Pi-hole")),
    ("server", "server", 4, None),
    ("srv", "server", 3, None),
];

/// mDNS service types → kind, points, label.
const SERVICES: &[Rule] = &[
    ("_googlecast._tcp", "streamer", 6, Some("Chromecast")),
    ("_airplay._tcp", "streamer", 3, None),
    ("_raop._tcp", "speaker", 2, None),
    ("_hap._tcp", "smart_home", 5, Some("HomeKit accessory")),
    ("_ipp._tcp", "printer", 8, None),
    ("_ipps._tcp", "printer", 8, None),
    ("_printer._tcp", "printer", 8, None),
    ("_pdl-datastream._tcp", "printer", 8, None),
    ("_smb._tcp", "nas", 2, None),
    ("_smb._tcp", "computer", 1, None),
    ("_afpovertcp._tcp", "nas", 3, None),
    ("_workstation._tcp", "server", 3, None),
    ("_ssh._tcp", "server", 2, None),
    ("_sftp-ssh._tcp", "server", 2, None),
    ("_companion-link._tcp", "computer", 2, None),
    ("_companion-link._tcp", "phone", 2, None),
    ("_apple-mobdev2._tcp", "phone", 6, None),
    ("_sleep-proxy._udp", "streamer", 4, None),
    (
        "_home-assistant._tcp",
        "smart_home",
        10,
        Some("Home Assistant"),
    ),
    ("_esphomelib._tcp", "smart_home", 9, Some("ESPHome device")),
    ("_hue._tcp", "smart_home", 9, Some("Hue Bridge")),
    ("_sonos._tcp", "speaker", 10, Some("Sonos speaker")),
    ("_spotify-connect._tcp", "speaker", 3, None),
    ("_miio._udp", "smart_home", 3, None),
    ("_amzn-wplay._tcp", "streamer", 7, Some("Fire TV")),
    ("_matter._tcp", "smart_home", 4, Some("Matter device")),
    ("_homeconnect._tcp", "appliance", 10, None),
    (
        "_octoprint._tcp",
        "printer",
        10,
        Some("3D printer (OctoPrint)"),
    ),
    ("_wled._tcp", "smart_home", 9, Some("WLED light")),
];

/// Open TCP ports → kind, points, label. Applied per port.
const PORTS: &[(u16, &str, i32, Option<&str>)] = &[
    (8006, "hypervisor", 10, Some("Proxmox VE")),
    (8007, "server", 8, Some("Proxmox Backup Server")),
    (8291, "router", 8, Some("MikroTik router")),
    (9100, "printer", 4, None),
    (631, "printer", 5, None),
    (515, "printer", 5, None),
    (554, "camera", 5, None),
    (62078, "phone", 6, None),
    (8009, "streamer", 5, Some("Chromecast")),
    (7000, "streamer", 2, None),
    (32400, "server", 5, Some("Plex server")),
    (8123, "smart_home", 7, Some("Home Assistant")),
    (1883, "smart_home", 2, None),
    (3389, "computer", 5, Some("Windows PC")),
    (2049, "nas", 3, None),
    (548, "nas", 2, None),
    (5001, "nas", 3, None),
    (53, "router", 3, None),
    (6443, "server", 5, Some("Kubernetes node")),
    (3306, "server", 3, None),
    (5432, "server", 3, None),
    (22, "server", 2, None),
];

/// Web interface title or Server header keywords (lower case) → kind, points, label.
const WEB: &[Rule] = &[
    (
        "proxmox virtual environment",
        "hypervisor",
        12,
        Some("Proxmox VE"),
    ),
    (
        "proxmox backup server",
        "server",
        12,
        Some("Proxmox Backup Server"),
    ),
    ("opnsense", "router", 12, Some("OPNsense")),
    ("pfsense", "router", 12, Some("pfSense")),
    ("openwrt", "router", 10, Some("OpenWrt router")),
    ("luci", "router", 6, Some("OpenWrt router")),
    ("fritz!box", "router", 12, Some("FRITZ!Box")),
    ("unifi network", "router", 6, Some("UniFi")),
    ("unifi os", "router", 8, Some("UniFi console")),
    ("routeros", "router", 10, Some("MikroTik router")),
    ("mikrotik", "router", 10, Some("MikroTik router")),
    ("synology", "nas", 12, Some("Synology NAS")),
    ("diskstation", "nas", 12, Some("Synology NAS")),
    ("truenas", "nas", 12, Some("TrueNAS")),
    ("freenas", "nas", 12, Some("TrueNAS")),
    ("unraid", "nas", 12, Some("Unraid")),
    ("qnap", "nas", 10, Some("QNAP NAS")),
    ("openmediavault", "nas", 12, Some("openmediavault")),
    ("home assistant", "smart_home", 12, Some("Home Assistant")),
    ("octoprint", "printer", 12, Some("3D printer (OctoPrint)")),
    ("pi-hole", "server", 9, Some("Pi-hole")),
    ("adguard home", "server", 9, Some("AdGuard Home")),
    ("plex", "server", 6, Some("Plex server")),
    ("jellyfin", "server", 8, Some("Jellyfin server")),
    ("portainer", "server", 6, None),
    ("grafana", "server", 6, None),
    ("cockpit", "server", 6, None),
    ("vmware esxi", "hypervisor", 12, Some("VMware ESXi")),
    ("laserjet", "printer", 10, Some("HP LaserJet")),
    ("officejet", "printer", 10, Some("HP OfficeJet")),
    ("epson", "printer", 8, None),
    ("brother", "printer", 6, None),
    ("printer", "printer", 7, None),
    ("ip camera", "camera", 9, None),
    ("hikvision", "camera", 10, None),
    ("reolink", "camera", 10, None),
    (
        "network video recorder",
        "camera",
        8,
        Some("Video recorder"),
    ),
    ("shelly", "smart_home", 10, Some("Shelly")),
    ("tasmota", "smart_home", 10, Some("Tasmota device")),
    ("esphome", "smart_home", 10, Some("ESPHome device")),
    ("wled", "smart_home", 10, Some("WLED light")),
    ("router", "router", 5, None),
    ("switch", "switch", 4, None),
    ("access point", "access_point", 6, None),
];

/// BSH (Bosch, Siemens, Neff, Gaggenau) model codes start with the appliance family.
const BSH_MODELS: &[(&str, &str)] = &[
    ("SM", "Dishwasher"),
    ("SN", "Dishwasher"),
    ("SB", "Dishwasher"),
    ("SX", "Dishwasher"),
    ("SR", "Dishwasher"),
    ("SP", "Dishwasher"),
    ("SG", "Dishwasher"),
    ("WA", "Washing machine"),
    ("WG", "Washing machine"),
    ("WM", "Washing machine"),
    ("WU", "Washing machine"),
    ("WI", "Washing machine"),
    ("WN", "Washer dryer"),
    ("WD", "Washer dryer"),
    ("WT", "Tumble dryer"),
    ("WQ", "Tumble dryer"),
    ("HB", "Oven"),
    ("HM", "Oven"),
    ("HS", "Oven"),
    ("HR", "Oven"),
    ("CM", "Oven"),
    ("CS", "Oven"),
    ("KG", "Fridge"),
    ("KI", "Fridge"),
    ("KA", "Fridge"),
    ("KF", "Fridge"),
    ("KS", "Fridge"),
    ("CT", "Coffee machine"),
    ("TI", "Coffee machine"),
    ("TQ", "Coffee machine"),
    ("PX", "Hob"),
    ("PI", "Hob"),
    ("DW", "Cooker hood"),
];

/// Apple model identifiers from mDNS `_device-info` TXT records.
const APPLE_MODELS: &[Rule] = &[
    ("iphone", "phone", 10, Some("iPhone")),
    ("ipad", "tablet", 10, Some("iPad")),
    ("macbook", "computer", 10, Some("Mac")),
    ("imac", "computer", 10, Some("Mac")),
    ("macmini", "computer", 10, Some("Mac")),
    ("macpro", "computer", 10, Some("Mac")),
    ("mac1", "computer", 9, Some("Mac")),
    ("appletv", "streamer", 10, Some("Apple TV")),
    ("audioaccessory", "speaker", 10, Some("HomePod")),
];

#[derive(Default)]
struct Score {
    points: i32,
    reasons: Vec<(i32, String)>,
    label: Option<(i32, &'static str)>,
}

#[derive(Default)]
struct Tally(HashMap<&'static str, Score>);

impl Tally {
    fn add(
        &mut self,
        kind: &'static str,
        points: i32,
        reason: String,
        label: Option<&'static str>,
    ) {
        let score = self.0.entry(kind).or_default();
        score.points += points;
        if !score.reasons.iter().any(|(_, r)| *r == reason) {
            score.reasons.push((points, reason));
        }
        if let Some(label) = label
            && score.label.is_none_or(|(p, _)| points > p)
        {
            score.label = Some((points, label));
        }
    }
    fn rules(&mut self, text: &str, rules: &[Rule], reason: impl Fn(&str) -> String) {
        let lower = text.to_ascii_lowercase();
        let mut seen = HashSet::new();
        for (needle, kind, points, label) in rules {
            if contains(&lower, needle) && seen.insert((*kind, *needle)) {
                self.add(kind, *points, reason(needle), *label);
            }
        }
    }
}

/// Whether `needle` occurs in `haystack`; keywords of four characters or fewer must stand alone
/// (so "nas" matches "nas-01" but not "jonas-iphone").
fn contains(haystack: &str, needle: &str) -> bool {
    if needle.len() > 4 || !needle.chars().all(|c| c.is_ascii_alphanumeric()) {
        return haystack.contains(needle);
    }
    haystack.match_indices(needle).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// The appliance a BSH (Bosch, Siemens, Neff, Gaggenau) model code describes, from a model such
/// as "SMV68TX06E" or a name such as "BOSCH-SMV68TX06E-68A40E…". Without a brand prefix the text
/// is read as a model code only when other evidence already points at BSH (`bsh`).
fn bsh_appliance(text: &str, bsh: bool) -> Option<&'static str> {
    let upper = text.to_ascii_uppercase();
    let branded = ["BOSCH-", "SIEMENS-", "NEFF-", "GAGGENAU-"]
        .iter()
        .find_map(|brand| upper.split_once(brand).map(|(_, rest)| rest.to_string()));
    if branded.is_none() && !bsh {
        return None;
    }
    let rest = branded.unwrap_or(upper);
    let code = rest.split(|c: char| !c.is_ascii_alphanumeric()).next()?;
    if code.len() < 6
        || !code.chars().any(|c| c.is_ascii_digit())
        || !code.chars().take(2).all(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    BSH_MODELS
        .iter()
        .find(|(prefix, _)| code.starts_with(prefix))
        .map(|(_, label)| *label)
}

fn is_address(name: &str) -> bool {
    name.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Names the device is known by, best first, excluding bare addresses.
fn names(host: &Host) -> Vec<String> {
    let e = &host.evidence;
    let mut names: Vec<String> = [
        e.dns_name.clone(),
        e.mdns_name.clone(),
        e.upnp_friendly_name.clone(),
    ]
    .into_iter()
    .flatten()
    .chain(e.mdns_instances.iter().cloned())
    .collect();
    if !is_address(&host.name) && !host.name.is_empty() {
        names.insert(0, host.name.clone());
    }
    names.dedup();
    names
}

/// A friendlier name for a host still named after its address.
pub fn display_name(host: &Host) -> Option<String> {
    if (host.agent_id.is_some() && !host.agent_uninstalled)
        || !(host.name.is_empty() || is_address(&host.name))
    {
        return None;
    }
    let e = &host.evidence;
    e.dns_name
        .clone()
        .or_else(|| e.mdns_name.clone())
        .or_else(|| e.upnp_friendly_name.clone())
        .or_else(|| e.mdns_instances.first().cloned())
}

/// Score every signal and return the most likely kind. `gateways` holds the addresses known to
/// route traffic (the server's default gateway and traceroute hops).
pub fn guess(host: &Host, gateways: &HashSet<String>) -> DeviceGuess {
    let mut tally = Tally::default();
    let e = &host.evidence;

    if let Some(gateway) = host.addresses.iter().find(|a| gateways.contains(*a)) {
        tally.add(
            "router",
            12,
            format!("Routes traffic: {gateway} is a gateway"),
            None,
        );
    }
    if let Some(vendor) = &host.vendor {
        tally.rules(vendor, VENDORS, |_| {
            format!("Network card made by {vendor}")
        });
    }
    let virtual_nic = host.macs.iter().find_map(|m| {
        let hex: String = m
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect::<String>()
            .to_ascii_lowercase();
        if hex.starts_with("525400") {
            Some("QEMU/KVM")
        } else if hex.starts_with("0242") {
            Some("Docker")
        } else {
            None
        }
    });
    if let Some(kind) = virtual_nic {
        tally.add("vm", 7, format!("{kind} virtual network card"), None);
    } else if host.vendor.is_none() && host.macs.iter().any(|m| crate::vendor::is_private(m)) {
        let reason = "Private (randomised) MAC address, as phones and laptops use on Wi-Fi";
        tally.add("phone", 3, reason.into(), None);
        tally.add("computer", 1, reason.into(), None);
    }
    let bsh = host
        .vendor
        .as_deref()
        .is_some_and(|v| v.to_ascii_lowercase().contains("bsh"))
        || e.mdns_services.iter().any(|s| s == "_homeconnect._tcp");
    for name in names(host) {
        tally.rules(&name, NAMES, |_| format!("Named \"{name}\""));
        if let Some(label) = bsh_appliance(&name, bsh) {
            tally.add(
                "appliance",
                6,
                format!("Model code in \"{name}\""),
                Some(label),
            );
        }
    }
    for service in &e.mdns_services {
        tally.rules(service, SERVICES, |_| {
            format!("Announces {service} over mDNS")
        });
    }
    if let Some(model) = &e.mdns_model {
        tally.rules(model, APPLE_MODELS, |_| format!("Reports model {model}"));
        tally.rules(model, NAMES, |_| format!("Reports model {model}"));
        if let Some(label) = bsh_appliance(model, bsh) {
            tally.add(
                "appliance",
                8,
                format!("Reports model {model}"),
                Some(label),
            );
        }
    }
    let open: Vec<u16> = host.services.iter().map(|s| s.port).collect();
    for (port, kind, points, label) in PORTS {
        if open.contains(port) {
            let name = crate::fingerprint::port_name(*port).unwrap_or("service");
            tally.add(kind, *points, format!("Port {port} open ({name})"), *label);
        }
    }
    if open.contains(&445) && open.contains(&139) && !open.contains(&22) {
        tally.add("computer", 2, "Windows file sharing open".into(), None);
    }
    if e.upnp_device_type.as_deref() == Some("InternetGatewayDevice") {
        tally.add(
            "router",
            9,
            "Advertises a UPnP internet gateway".into(),
            None,
        );
    }
    if let Some(kind) = &e.upnp_device_type {
        let lower = kind.to_ascii_lowercase();
        if lower.contains("mediarenderer") {
            tally.add("tv", 2, format!("UPnP {kind}"), None);
            tally.add("speaker", 2, format!("UPnP {kind}"), None);
        } else if lower.contains("mediaserver") {
            tally.add("nas", 2, format!("UPnP {kind}"), None);
        }
    }
    for text in [&e.upnp_manufacturer, &e.upnp_model, &e.ssdp_server]
        .into_iter()
        .flatten()
    {
        tally.rules(text, VENDORS, |_| format!("UPnP reports \"{text}\""));
        tally.rules(text, NAMES, |_| format!("UPnP reports \"{text}\""));
    }
    for text in [&e.http_title, &e.http_server].into_iter().flatten() {
        tally.rules(text, WEB, |_| format!("Web interface \"{text}\""));
    }
    if let Some(inventory) = &host.inventory {
        let os = inventory.os.to_ascii_lowercase();
        if os.contains("mac") || os.contains("windows") {
            tally.add(
                "computer",
                6,
                format!("Agent reports {}", inventory.os),
                None,
            );
        } else {
            tally.add("server", 5, format!("Agent reports {}", inventory.os), None);
        }
    }

    let mut ranked: Vec<(&'static str, Score)> = tally.0.into_iter().collect();
    ranked.sort_by(|a, b| b.1.points.cmp(&a.1.points).then(a.0.cmp(b.0)));
    let runner_up = ranked.get(1).map_or(0, |(_, s)| s.points);
    let Some((kind, mut score)) = ranked.into_iter().next().filter(|(_, s)| s.points >= 2) else {
        return DeviceGuess {
            kind: "unknown".into(),
            label: "Unknown device".into(),
            confidence: 0,
            reasons: vec![],
            manual: false,
        };
    };
    let mut confidence: i32 = match score.points {
        p if p >= 12 => 95,
        p if p >= 9 => 85,
        p if p >= 6 => 70,
        p if p >= 4 => 55,
        _ => 35,
    };
    if score.points - runner_up <= 1 {
        confidence -= 20;
    }
    score.reasons.sort_by_key(|r| std::cmp::Reverse(r.0));
    DeviceGuess {
        kind: kind.into(),
        label: score
            .label
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| kind_label(kind).unwrap_or("Unknown device").into()),
        confidence: confidence.clamp(10, 99) as u8,
        reasons: score.reasons.into_iter().take(4).map(|(_, r)| r).collect(),
        manual: false,
    }
}

/// Recompute the vendor, guess and display name of a host. Returns true when anything changed.
/// A type the administrator set is kept.
pub fn refresh(host: &mut Host, gateways: &HashSet<String>) -> bool {
    let before = (host.vendor.clone(), host.device.clone(), host.name.clone());
    host.vendor = crate::vendor::of(&host.macs);
    if !host.device.as_ref().is_some_and(|d| d.manual) {
        host.device = Some(guess(host, gateways));
    }
    if let Some(name) = display_name(host) {
        host.name = name;
    }
    before != (host.vendor.clone(), host.device.clone(), host.name.clone())
}

/// The guess for a type chosen by the administrator.
pub fn manual(kind: &str) -> Option<DeviceGuess> {
    Some(DeviceGuess {
        kind: kind.into(),
        label: kind_label(kind)?.into(),
        confidence: 100,
        reasons: vec!["Set by the administrator".into()],
        manual: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::Service;

    fn host(f: impl FnOnce(&mut Host)) -> Host {
        let mut host = Host::new("h".into(), "192.168.1.50".into(), "icmp");
        host.addresses = vec!["192.168.1.50".into()];
        f(&mut host);
        host.vendor = crate::vendor::of(&host.macs);
        host
    }
    fn ports(list: &[u16]) -> Vec<Service> {
        list.iter()
            .map(|p| Service {
                port: *p,
                name: String::new(),
                observed_at: 0.0,
            })
            .collect()
    }
    fn kind(h: &Host) -> (String, String) {
        let g = guess(h, &HashSet::new());
        (g.kind, g.label)
    }

    #[test]
    fn proxmox_is_recognised_from_its_port_and_title() {
        let h = host(|h| {
            h.services = ports(&[22, 8006]);
            h.evidence.http_title = Some("pve1 - Proxmox Virtual Environment".into());
        });
        let g = guess(&h, &HashSet::new());
        assert_eq!(
            (g.kind.as_str(), g.label.as_str()),
            ("hypervisor", "Proxmox VE")
        );
        assert!(g.confidence >= 90, "{g:?}");
        assert!(g.reasons[0].contains("Proxmox Virtual Environment"));
    }

    #[test]
    fn appliances_and_vacuums_are_named_specifically() {
        let dishwasher = host(|h| {
            h.macs = vec!["68:a4:0e:12:34:56".into()];
            h.evidence.mdns_services = vec!["_homeconnect._tcp".into()];
            h.evidence.mdns_instances = vec!["BOSCH-SMV68TX06E-68A40E123456".into()];
        });
        assert_eq!(kind(&dishwasher), ("appliance".into(), "Dishwasher".into()));
        let washer = host(|h| {
            h.evidence.mdns_services = vec!["_homeconnect._tcp".into()];
            h.evidence.mdns_model = Some("WAX32M41GB".into());
        });
        assert_eq!(kind(&washer).1, "Washing machine");
        let vacuum = host(|h| {
            h.evidence.mdns_services = vec!["_miio._udp".into()];
            h.evidence.mdns_name = Some("roborock-vacuum-a15".into());
        });
        assert_eq!(kind(&vacuum), ("vacuum".into(), "Robot vacuum".into()));
        let roomba = host(|h| h.macs = vec!["50:14:79:00:00:01".into()]);
        assert_eq!(kind(&roomba).0, "vacuum", "iRobot OUI");
    }

    #[test]
    fn gateways_phones_printers_and_vms() {
        let gateways: HashSet<String> = ["192.168.1.50".to_string()].into();
        let router = host(|h| h.services = ports(&[53, 80, 443]));
        let g = guess(&router, &gateways);
        assert_eq!(g.kind, "router");
        assert!(g.confidence >= 90);
        let phone = host(|h| {
            h.macs = vec!["da:a1:19:00:00:01".into()];
            h.services = ports(&[62078]);
        });
        assert_eq!(kind(&phone).0, "phone");
        let printer = host(|h| {
            h.services = ports(&[80, 631, 9100]);
            h.evidence.mdns_services = vec!["_ipp._tcp".into()];
        });
        assert_eq!(kind(&printer).0, "printer");
        let vm = host(|h| h.macs = vec!["bc:24:11:aa:bb:cc".into()]);
        assert_eq!(kind(&vm), ("vm".into(), "Proxmox virtual machine".into()));
        let mac = host(|h| h.evidence.mdns_model = Some("MacBookPro18,3".into()));
        assert_eq!(kind(&mac), ("computer".into(), "Mac".into()));
    }

    #[test]
    fn no_evidence_is_unknown_and_manual_types_stick() {
        let silent = host(|_| {});
        let g = guess(&silent, &HashSet::new());
        assert_eq!((g.kind.as_str(), g.confidence), ("unknown", 0));
        let mut h = host(|h| h.services = ports(&[8006]));
        h.device = manual("nas");
        refresh(&mut h, &HashSet::new());
        assert_eq!(
            h.device.as_ref().map(|d| (d.kind.as_str(), d.manual)),
            Some(("nas", true))
        );
        let jonas = host(|h| h.evidence.mdns_name = Some("jonas-iphone".into()));
        assert_eq!(kind(&jonas), ("phone".into(), "iPhone".into()));
        assert!(contains("nas-01", "nas") && !contains("jonas", "nas"));
        assert!(manual("toaster").is_none());
    }

    #[test]
    fn hosts_named_after_their_address_take_an_announced_name() {
        let mut h = host(|h| h.evidence.mdns_instances = vec!["Living Room TV".into()]);
        refresh(&mut h, &HashSet::new());
        assert_eq!(h.name, "Living Room TV");
        let mut named = host(|h| {
            h.name = "nas.lan".into();
            h.evidence.mdns_name = Some("other".into());
        });
        refresh(&mut named, &HashSet::new());
        assert_eq!(named.name, "nas.lan");
    }
}
