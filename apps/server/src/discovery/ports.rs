//! Fixed TCP plan for routine discovery. Keep this ordered for stable retries.

/// Common infrastructure, remote access, database, and homelab service ports.
/// Full TCP scans remain available through the `full_tcp` run kind.
pub const STANDARD_TCP_PORTS: &[u16] = &[
    22, 25, 53, 80, 111, 139, 161, 389, 443, 445, 554, 587, 631, 636, 993, 995, 1883, 2049, 3000,
    3306, 3389, 5000, 5001, 5432, 5900, 6379, 6443, 8006, 8080, 8123, 8443, 8888, 9000, 9090, 9200,
    9443, 10000, 27017, 32400,
];

pub const STANDARD_PORT_COUNT: i64 = STANDARD_TCP_PORTS.len() as i64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_ports_are_sorted_unique_and_valid() {
        assert_eq!(STANDARD_TCP_PORTS.len(), 39);
        assert!(STANDARD_TCP_PORTS[0] > 0);
        assert!(STANDARD_TCP_PORTS.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
