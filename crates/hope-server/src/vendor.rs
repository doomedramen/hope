//! MAC address vendor lookup from the embedded IEEE registry (`data/oui.tsv`, see `data/OUI-NOTICE`).
use std::{collections::HashMap, sync::OnceLock};

static TABLE: &str = include_str!("../data/oui.tsv");

fn table() -> &'static HashMap<&'static str, &'static str> {
    static PARSED: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    PARSED.get_or_init(|| {
        TABLE
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    })
}

/// Upper-case hex digits of a MAC address in any common notation, or None when it is not 48 bits.
fn digits(mac: &str) -> Option<String> {
    let hex: String = mac
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (hex.len() == 12).then_some(hex)
}

/// True for locally administered addresses, which phones and laptops use as private (randomised)
/// Wi-Fi addresses. These carry no vendor.
pub fn is_private(mac: &str) -> bool {
    digits(mac)
        .and_then(|hex| u8::from_str_radix(&hex[..2], 16).ok())
        .is_some_and(|first| first & 0b10 != 0)
}

/// Registered manufacturer for a MAC address: the longest matching 36-, 28- or 24-bit prefix.
pub fn lookup(mac: &str) -> Option<&'static str> {
    let hex = digits(mac)?;
    if is_private(mac) {
        return None;
    }
    [9, 7, 6]
        .into_iter()
        .find_map(|length| table().get(&hex[..length]).copied())
}

/// Vendor for the first MAC address that has one.
pub fn of(macs: &[String]) -> Option<String> {
    macs.iter().find_map(|mac| lookup(mac)).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn longest_prefix_wins_and_private_addresses_have_no_vendor() {
        assert_eq!(lookup("b8:27:eb:12:34:56"), Some("Raspberry Pi Foundation"));
        assert_eq!(
            lookup("BC-24-11-AA-BB-CC"),
            Some("Proxmox Server Solutions")
        );
        assert_eq!(lookup("00156d000001"), Some("Ubiquiti"));
        assert!(
            table().keys().any(|k| k.len() == 9),
            "36-bit blocks are loaded"
        );
        let (long, vendor) = table()
            .iter()
            .find(|(k, _)| k.len() == 9)
            .expect("MA-S block");
        let mac = format!("{}{}", long, "000");
        assert_eq!(lookup(&mac), Some(*vendor));
        assert!(is_private("da:a1:19:00:00:01"));
        assert_eq!(lookup("da:a1:19:00:00:01"), None);
        assert_eq!(lookup("not a mac"), None);
        assert_eq!(
            of(&["da:a1:19:00:00:01".into(), "3c:61:05:00:00:01".into()]).as_deref(),
            Some("Espressif")
        );
    }
}
