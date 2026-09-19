//! Safe network discovery scope validation.
//!
//! M2 requires an operator to approve a concrete CIDR before a worker may
//! scan it. This module owns that policy so route handlers and workers do
//! not each grow their own version of the public-range, size, and exclusion
//! checks.

use std::net::Ipv4Addr;
use std::str::FromStr;

use ipnet::{IpNet, Ipv4Net};
use thiserror::Error;

/// Largest private IPv4 scope accepted without an explicit future override.
/// A /16 is already far above the v1 homelab target (spec §5.3).
pub const MAX_TARGETS: u64 = 65_534;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScopeError {
    #[error("invalid CIDR `{value}`")]
    InvalidCidr { value: String },
    #[error("scope `{value}` must use IPv4")]
    NonIpv4 { value: String },
    #[error("scope `{value}` must be entirely private RFC1918 address space")]
    Public { value: String },
    #[error("scope has {target_count} targets; maximum is {MAX_TARGETS}")]
    TooLarge { target_count: u64 },
    #[error("exclusion `{exclusion}` is outside scope `{scope}`")]
    ExclusionOutsideScope { exclusion: String, scope: String },
}

/// A validated operator-approved scope and its scanable IPv4 target count.
/// Network and broadcast addresses are never TCP scan targets for prefixes
/// through /30. Excluded CIDRs are unioned before calculating the count, so
/// overlapping exclusions cannot subtract targets twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedScope {
    cidr: Ipv4Net,
    exclusions: Vec<Ipv4Net>,
    target_count: u64,
}

impl ApprovedScope {
    pub fn parse(cidr: &str, exclusions: &[String]) -> Result<Self, ScopeError> {
        let cidr = parse_ipv4_net(cidr)?;
        if !is_entirely_private(&cidr) {
            return Err(ScopeError::Public {
                value: cidr.to_string(),
            });
        }

        let scope_range = network_range(&cidr);
        let mut parsed_exclusions = Vec::with_capacity(exclusions.len());
        for exclusion in exclusions {
            let exclusion_net = parse_ipv4_net(exclusion)?;
            let exclusion_range = network_range(&exclusion_net);
            if exclusion_range.0 < scope_range.0 || exclusion_range.1 > scope_range.1 {
                return Err(ScopeError::ExclusionOutsideScope {
                    exclusion: exclusion_net.to_string(),
                    scope: cidr.to_string(),
                });
            }
            parsed_exclusions.push(exclusion_net);
        }

        let target_count = scanable_target_count(&cidr, &parsed_exclusions);
        if target_count > MAX_TARGETS {
            return Err(ScopeError::TooLarge { target_count });
        }

        Ok(Self {
            cidr,
            exclusions: parsed_exclusions,
            target_count,
        })
    }

    pub fn cidr(&self) -> Ipv4Net {
        self.cidr
    }

    pub fn exclusions(&self) -> &[Ipv4Net] {
        &self.exclusions
    }

    pub fn target_count(&self) -> u64 {
        self.target_count
    }
}

fn parse_ipv4_net(value: &str) -> Result<Ipv4Net, ScopeError> {
    let net = IpNet::from_str(value).map_err(|_| ScopeError::InvalidCidr {
        value: value.to_string(),
    })?;
    match net {
        IpNet::V4(net) => Ok(net),
        IpNet::V6(_) => Err(ScopeError::NonIpv4 {
            value: value.to_string(),
        }),
    }
}

fn is_entirely_private(net: &Ipv4Net) -> bool {
    let (first, last) = network_range(net);
    Ipv4Addr::from(first as u32).is_private() && Ipv4Addr::from(last as u32).is_private()
}

fn network_range(net: &Ipv4Net) -> (u64, u64) {
    let start = u32::from(net.network()) as u64;
    let addresses = 1_u64 << (32 - net.prefix_len());
    (start, start + addresses - 1)
}

fn scanable_range(net: &Ipv4Net) -> Option<(u64, u64)> {
    let (start, end) = network_range(net);
    if net.prefix_len() <= 30 {
        Some((start + 1, end - 1))
    } else {
        Some((start, end))
    }
}

fn scanable_target_count(cidr: &Ipv4Net, exclusions: &[Ipv4Net]) -> u64 {
    let Some((scan_start, scan_end)) = scanable_range(cidr) else {
        return 0;
    };
    let mut excluded: Vec<(u64, u64)> = exclusions
        .iter()
        .filter_map(|net| {
            let (start, end) = network_range(net);
            let start = start.max(scan_start);
            let end = end.min(scan_end);
            (start <= end).then_some((start, end))
        })
        .collect();
    excluded.sort_unstable();

    let mut removed = 0;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in excluded {
        match current {
            Some((current_start, current_end)) if start <= current_end.saturating_add(1) => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                removed += current_end - current_start + 1;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        removed += end - start + 1;
    }

    scan_end - scan_start + 1 - removed
}

#[cfg(test)]
mod tests {
    use super::ApprovedScope;

    #[test]
    fn private_scope_counts_hosts_after_exclusions() {
        let scope = ApprovedScope::parse(
            "192.168.10.0/24",
            &[
                "192.168.10.10/32".to_string(),
                "192.168.10.128/25".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(scope.target_count(), 126);
    }

    #[test]
    fn public_scope_is_rejected() {
        let error = ApprovedScope::parse("8.8.8.0/24", &[]).unwrap_err();

        assert!(error.to_string().contains("private"));
    }

    #[test]
    fn oversized_scope_is_rejected_before_scan() {
        let error = ApprovedScope::parse("10.0.0.0/8", &[]).unwrap_err();

        assert!(error.to_string().contains("maximum"));
    }

    #[test]
    fn exclusion_must_be_inside_scope() {
        let error =
            ApprovedScope::parse("192.168.10.0/24", &["192.168.11.0/24".to_string()]).unwrap_err();

        assert!(error.to_string().contains("outside"));
    }
}
