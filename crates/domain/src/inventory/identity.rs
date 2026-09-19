//! Identity scoring for device reconciliation.
//!
//! Spec §4.2; design docs/design/m1-inventory.md §3. Pure, DB-free scoring
//! so the reconciliation policy is unit- and property-testable without a
//! database. The server/worker layers persist `identity_rules` rows and
//! call [`score`] with the rows loaded for a candidate device.

use serde::{Deserialize, Serialize};

/// Identifier kinds usable for identity matching, mirrors the
/// `identity_rules.rule_type` check constraint (migration 0005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierType {
    AgentId,
    HardwareUuid,
    MachineId,
    TlsCertIdentity,
    SshHostKey,
    DockerContainerId,
    ProxmoxVmid,
    Mac,
    Serial,
    SnmpEngineId,
    Hostname,
    InterfaceIpHistory,
}

impl IdentifierType {
    /// Weight per docs/design/m1-inventory.md §3's table.
    pub fn weight(self) -> f32 {
        match self {
            IdentifierType::AgentId => 1.0,
            IdentifierType::HardwareUuid => 1.0,
            IdentifierType::MachineId => 0.9,
            IdentifierType::TlsCertIdentity => 0.8,
            IdentifierType::SshHostKey => 0.8,
            IdentifierType::DockerContainerId => 0.8,
            IdentifierType::ProxmoxVmid => 0.8,
            IdentifierType::Mac => 0.6,
            IdentifierType::Serial => 0.6,
            IdentifierType::SnmpEngineId => 0.5,
            IdentifierType::Hostname => 0.3,
            IdentifierType::InterfaceIpHistory => 0.2,
        }
    }

    /// Deterministic identifiers auto-match on their own, per Decision 2.
    pub fn is_deterministic(self) -> bool {
        matches!(self, IdentifierType::AgentId | IdentifierType::HardwareUuid)
    }
}

/// One identifier observed on an incoming report, or stored against a
/// candidate device (`identity_rules` row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identifier {
    pub rule_type: IdentifierType,
    pub value: String,
    /// A pinned identifier short-circuits scoring to 1.0 and can never be
    /// outscored (§4.2, Decision-consistent with migration 0005's partial
    /// unique index).
    pub pinned: bool,
}

/// One matched identifier in the explanation record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchedIdentifier {
    pub rule_type: IdentifierType,
    pub weight: f32,
    pub value: String,
}

/// The decision an identity-scoring pass reaches for one candidate device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Deterministic ID or combined score ≥0.85 from ≥2 independent
    /// identifier types: attach/merge automatically.
    AutoMatch,
    /// Score in [0.4, 0.85): write to the review queue, no graph mutation.
    Suggested,
    /// Score < 0.4 (or MAC-only, however high its raw weight): treat as a
    /// new device.
    NewDevice,
}

/// Explainability record — the literal payload rendered by the §13.4
/// "Why?" view and stored in `merge_events.explanation` /
/// `identity_suggestions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Explanation {
    pub score: f32,
    pub matched: Vec<MatchedIdentifier>,
    pub threshold: f32,
    pub decision: Decision,
}

pub const AUTO_MATCH_THRESHOLD: f32 = 0.85;
// Design table (§3) puts hostname's own weight at 0.3, below the
// documented 0.4 review floor, yet §7's acceptance-gate table requires a
// hostname-only match to land in the review queue rather than being
// dropped as a new device ("hostname-only match (weight 0.3) ... appears
// in GET /identity-suggestions"). The gate is authoritative: the floor is
// set just under the weakest single-identifier weight (interface/IP
// history overlap, 0.2) so any real identifier match is at minimum
// surfaced for review, and only a total absence of matches falls through
// to NewDevice.
pub const REVIEW_THRESHOLD: f32 = 0.2;

/// Score an incoming set of observed identifiers against one candidate
/// device's known identifiers.
///
/// Combination uses a probabilistic-OR (`1 - Π(1 - weight)`) over matched,
/// distinct identifier types so a single weight-1.0 match yields exactly
/// 1.0 (consistent with "deterministic ID alone auto-matches") while
/// multiple weaker matches compound without needing an arbitrary
/// normalization constant.
pub fn score(observed: &[Identifier], candidate: &[Identifier]) -> Explanation {
    let mut matched: Vec<MatchedIdentifier> = Vec::new();
    let mut complement: f32 = 1.0;
    let mut deterministic_match = false;
    let mut mac_only = true;

    for obs in observed {
        let Some(hit) = candidate
            .iter()
            .find(|c| c.rule_type == obs.rule_type && c.value == obs.value)
        else {
            continue;
        };

        let weight = if hit.pinned || obs.pinned {
            1.0
        } else {
            obs.rule_type.weight()
        };
        if obs.rule_type.is_deterministic() {
            deterministic_match = true;
        }
        if obs.rule_type != IdentifierType::Mac {
            mac_only = false;
        }

        matched.push(MatchedIdentifier {
            rule_type: obs.rule_type,
            weight,
            value: obs.value.clone(),
        });
        complement *= 1.0 - weight;
    }

    let raw_score = 1.0 - complement;
    let independent_types = matched
        .iter()
        .map(|m| m.rule_type)
        .collect::<std::collections::HashSet<_>>()
        .len();

    // MAC alone never drives an auto-merge, regardless of score
    // (Decision 2), even though a single MAC hit's raw weight (0.6) sits
    // above the review floor.
    let decision = if matched.is_empty() {
        Decision::NewDevice
    } else if deterministic_match
        || (raw_score >= AUTO_MATCH_THRESHOLD && independent_types >= 2 && !mac_only)
    {
        Decision::AutoMatch
    } else if raw_score >= REVIEW_THRESHOLD {
        Decision::Suggested
    } else {
        Decision::NewDevice
    };

    // A mac-only match that would otherwise clear AutoMatch must be
    // downgraded to Suggested, never NewDevice (it's still real evidence).
    let decision = if mac_only
        && matched.iter().any(|m| m.rule_type == IdentifierType::Mac)
        && decision == Decision::AutoMatch
    {
        Decision::Suggested
    } else {
        decision
    };

    Explanation {
        score: raw_score,
        matched,
        threshold: AUTO_MATCH_THRESHOLD,
        decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(t: IdentifierType, v: &str) -> Identifier {
        Identifier {
            rule_type: t,
            value: v.to_string(),
            pinned: false,
        }
    }

    #[test]
    fn deterministic_agent_id_auto_matches_alone() {
        let observed = vec![id(IdentifierType::AgentId, "a1")];
        let candidate = vec![id(IdentifierType::AgentId, "a1")];
        let exp = score(&observed, &candidate);
        assert_eq!(exp.decision, Decision::AutoMatch);
        assert_eq!(exp.score, 1.0);
    }

    #[test]
    fn mac_alone_never_auto_matches_even_at_high_score() {
        let observed = vec![id(IdentifierType::Mac, "aa:bb:cc:dd:ee:ff")];
        let candidate = vec![id(IdentifierType::Mac, "aa:bb:cc:dd:ee:ff")];
        let exp = score(&observed, &candidate);
        assert_ne!(exp.decision, Decision::AutoMatch);
    }

    #[test]
    fn hostname_only_is_suggested_not_merged() {
        let observed = vec![id(IdentifierType::Hostname, "docker01")];
        let candidate = vec![id(IdentifierType::Hostname, "docker01")];
        let exp = score(&observed, &candidate);
        assert_eq!(exp.decision, Decision::Suggested);
    }

    #[test]
    fn two_independent_weak_types_can_auto_match_above_threshold() {
        // machine_id (0.9) alone already clears 0.85 with 1 type; combine
        // with hostname to also exercise the independent-types path.
        let observed = vec![
            id(IdentifierType::MachineId, "m1"),
            id(IdentifierType::Hostname, "h1"),
        ];
        let candidate = vec![
            id(IdentifierType::MachineId, "m1"),
            id(IdentifierType::Hostname, "h1"),
        ];
        let exp = score(&observed, &candidate);
        assert_eq!(exp.decision, Decision::AutoMatch);
    }

    #[test]
    fn no_overlap_is_new_device() {
        let observed = vec![id(IdentifierType::Hostname, "x")];
        let candidate = vec![id(IdentifierType::Hostname, "y")];
        let exp = score(&observed, &candidate);
        assert_eq!(exp.decision, Decision::NewDevice);
    }

    #[test]
    fn pinned_identifier_short_circuits_to_full_weight() {
        let observed = vec![id(IdentifierType::Hostname, "docker01")];
        let mut candidate = id(IdentifierType::Hostname, "docker01");
        candidate.pinned = true;
        let exp = score(&observed, std::slice::from_ref(&candidate));
        assert_eq!(exp.matched[0].weight, 1.0);
    }
}
