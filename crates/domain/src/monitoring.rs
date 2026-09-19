//! Deterministic monitor health state machine.
//!
//! This module contains no clock or persistence access. Callers provide the
//! timestamp for each observation and can persist the returned state snapshot
//! and structured events in their own layer.

use std::time::Duration;

use jiff::Timestamp;
use serde::de::Error as SerdeError;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Maximum delay accepted for deriving a stale state.
pub const MAX_STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default consecutive failures required to open an incident.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
/// Default consecutive successes required to recover an open incident.
pub const DEFAULT_RECOVERY_THRESHOLD: u32 = 2;
/// Default delay after the last observation before a monitor becomes stale.
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(5 * 60);

/// Health state exposed to monitoring consumers.
///
/// `Stale` is an effective state derived from observation age. The state
/// machine keeps its underlying `Unknown`, `Up`, `Degraded`, or `Down` state
/// unchanged while a monitor is stale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    #[default]
    Unknown,
    Up,
    Degraded,
    Down,
    Stale,
}

impl HealthState {
    /// Return the stable wire spelling used by persistence and API layers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Up => "up",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::Stale => "stale",
        }
    }
}

/// One normalized result from a monitor execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthObservation {
    Success,
    Failure,
}

/// Event classification for a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthEventKind {
    StateChanged,
    IncidentOpened,
    IncidentRecovered,
    StaleStarted,
    StaleCleared,
}

/// Validated thresholds and staleness policy for one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HealthConfig {
    failure_threshold: u32,
    recovery_threshold: u32,
    stale_after: Duration,
}

impl HealthConfig {
    /// Build a policy with nonzero thresholds and a bounded stale delay.
    pub fn new(
        failure_threshold: u32,
        recovery_threshold: u32,
        stale_after: Duration,
    ) -> Result<Self, HealthConfigError> {
        if failure_threshold == 0 {
            return Err(HealthConfigError::ZeroFailureThreshold);
        }
        if recovery_threshold == 0 {
            return Err(HealthConfigError::ZeroRecoveryThreshold);
        }
        if stale_after.is_zero() {
            return Err(HealthConfigError::ZeroStaleAfter);
        }
        if stale_after > MAX_STALE_AFTER {
            return Err(HealthConfigError::StaleAfterTooLong);
        }

        Ok(Self {
            failure_threshold,
            recovery_threshold,
            stale_after,
        })
    }

    pub const fn failure_threshold(self) -> u32 {
        self.failure_threshold
    }

    pub const fn recovery_threshold(self) -> u32 {
        self.recovery_threshold
    }

    pub const fn stale_after(self) -> Duration {
        self.stale_after
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            recovery_threshold: DEFAULT_RECOVERY_THRESHOLD,
            stale_after: DEFAULT_STALE_AFTER,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawHealthConfig {
    failure_threshold: u32,
    recovery_threshold: u32,
    stale_after: Duration,
}

impl<'de> Deserialize<'de> for HealthConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawHealthConfig::deserialize(deserializer)?;
        Self::new(
            raw.failure_threshold,
            raw.recovery_threshold,
            raw.stale_after,
        )
        .map_err(D::Error::custom)
    }
}

/// Invalid monitor health policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HealthConfigError {
    #[error("failure threshold must be greater than zero")]
    ZeroFailureThreshold,
    #[error("recovery threshold must be greater than zero")]
    ZeroRecoveryThreshold,
    #[error("stale delay must be greater than zero")]
    ZeroStaleAfter,
    #[error("stale delay exceeds the maximum allowed duration")]
    StaleAfterTooLong,
}

/// Persistable non-derived health state for one monitor.
///
/// `underlying_state` intentionally excludes [`HealthState::Stale`]. Stale is
/// derived again from `last_observed_at` and the restored [`HealthConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub underlying_state: HealthState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub last_observed_at: Option<Timestamp>,
    pub last_success_at: Option<Timestamp>,
}

/// Error returned when persisted health state cannot be restored safely.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HealthRestoreError {
    #[error("underlying health state `{state:?}` cannot be restored")]
    InvalidUnderlyingState { state: HealthState },
    #[error(
        "invalid counters for underlying state `{state:?}`: failures={consecutive_failures}, successes={consecutive_successes}"
    )]
    InvalidCounters {
        state: HealthState,
        consecutive_failures: u32,
        consecutive_successes: u32,
    },
    #[error("underlying state `{state:?}` requires last_observed_at")]
    MissingObservedAt { state: HealthState },
    #[error("underlying state `{state:?}` requires last_success_at")]
    MissingSuccessAt { state: HealthState },
    #[error("last_success_at cannot be later than last_observed_at")]
    SuccessAfterObservation,
}

/// One structured state event suitable for persistence.
///
/// `from` and `to` are effective states, so a stale transition is visible to
/// consumers. `underlying_from` and `underlying_to` make stale derivation
/// explicit and preserve incident semantics for downstream persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthEvent {
    pub kind: HealthEventKind,
    pub at: Timestamp,
    pub from: HealthState,
    pub to: HealthState,
    pub underlying_from: HealthState,
    pub underlying_to: HealthState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
}

/// Alias for callers that refer to emitted events as transitions.
pub type HealthTransition = HealthEvent;

/// State snapshot and events emitted after one observation or stale check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthUpdate {
    pub state: HealthState,
    pub underlying_state: HealthState,
    pub incident_open: bool,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub last_observed_at: Option<Timestamp>,
    pub last_success_at: Option<Timestamp>,
    pub events: Vec<HealthEvent>,
}

impl HealthUpdate {
    /// Return the sole transition for this update, when state changed.
    ///
    /// The state machine emits at most one event per update. The event list is
    /// retained so the persistence boundary has a stable append-only shape.
    pub fn transition(&self) -> Option<&HealthTransition> {
        self.events.first()
    }
}

/// Pure monitor health state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthStateMachine {
    config: HealthConfig,
    underlying_state: HealthState,
    consecutive_failures: u32,
    consecutive_successes: u32,
    last_observed_at: Option<Timestamp>,
    last_success_at: Option<Timestamp>,
    last_reported_state: HealthState,
}

impl HealthStateMachine {
    pub fn new(config: HealthConfig) -> Self {
        Self {
            config,
            underlying_state: HealthState::Unknown,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_observed_at: None,
            last_success_at: None,
            last_reported_state: HealthState::Unknown,
        }
    }

    pub fn config(&self) -> HealthConfig {
        self.config
    }

    /// Return the non-derived state needed to reconstruct this machine.
    pub fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            underlying_state: self.underlying_state,
            consecutive_failures: self.consecutive_failures,
            consecutive_successes: self.consecutive_successes,
            last_observed_at: self.last_observed_at,
            last_success_at: self.last_success_at,
        }
    }

    /// Restore a machine from persisted non-derived state.
    pub fn from_snapshot(
        config: HealthConfig,
        snapshot: HealthSnapshot,
    ) -> Result<Self, HealthRestoreError> {
        validate_snapshot(config, &snapshot)?;

        Ok(Self {
            config,
            underlying_state: snapshot.underlying_state,
            consecutive_failures: snapshot.consecutive_failures,
            consecutive_successes: snapshot.consecutive_successes,
            last_observed_at: snapshot.last_observed_at,
            last_success_at: snapshot.last_success_at,
            last_reported_state: snapshot.underlying_state,
        })
    }

    /// Alias for callers that describe reconstruction as restore.
    pub fn restore(
        config: HealthConfig,
        snapshot: HealthSnapshot,
    ) -> Result<Self, HealthRestoreError> {
        Self::from_snapshot(config, snapshot)
    }

    /// Return the non-stale state retained by the state machine.
    pub fn state(&self) -> HealthState {
        self.underlying_state
    }

    pub fn underlying_state(&self) -> HealthState {
        self.underlying_state
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn consecutive_successes(&self) -> u32 {
        self.consecutive_successes
    }

    pub fn last_observed_at(&self) -> Option<Timestamp> {
        self.last_observed_at
    }

    pub fn last_success_at(&self) -> Option<Timestamp> {
        self.last_success_at
    }

    pub fn incident_open(&self) -> bool {
        self.underlying_state == HealthState::Down
    }

    /// Derive effective state without changing the underlying state machine.
    pub fn state_at(&self, at: Timestamp) -> HealthState {
        self.effective_state_at(at)
    }

    /// Record one monitor result and return its state snapshot and event.
    pub fn observe(&mut self, at: Timestamp, observation: HealthObservation) -> HealthUpdate {
        let previous_state = self.effective_state_at(at);
        let previous_underlying = self.underlying_state;

        self.last_observed_at = Some(latest_timestamp(self.last_observed_at, at));
        match observation {
            HealthObservation::Success => self.record_success(at),
            HealthObservation::Failure => self.record_failure(),
        }

        self.update_after_transition(at, previous_state, previous_underlying)
    }

    /// Re-evaluate staleness at `at` without recording a new observation.
    pub fn evaluate(&mut self, at: Timestamp) -> HealthUpdate {
        let state = self.effective_state_at(at);
        let previous_state = self.last_reported_state;
        let events = if previous_state == state {
            Vec::new()
        } else {
            vec![self.event(at, previous_state, state, self.underlying_state)]
        };
        self.last_reported_state = state;
        self.update_snapshot(events, state)
    }

    fn effective_state_at(&self, at: Timestamp) -> HealthState {
        if self.underlying_state == HealthState::Unknown {
            return HealthState::Unknown;
        }

        let Some(last_observed_at) = self.last_observed_at else {
            return self.underlying_state;
        };

        if at >= last_observed_at
            && at.duration_since(last_observed_at).unsigned_abs() >= self.config.stale_after
        {
            HealthState::Stale
        } else {
            self.underlying_state
        }
    }

    fn record_success(&mut self, at: Timestamp) {
        self.last_success_at = Some(latest_timestamp(self.last_success_at, at));
        self.consecutive_failures = 0;
        self.consecutive_successes = self.consecutive_successes.saturating_add(1);

        match self.underlying_state {
            HealthState::Unknown | HealthState::Degraded => {
                self.underlying_state = HealthState::Up;
            }
            HealthState::Down if self.consecutive_successes >= self.config.recovery_threshold => {
                self.underlying_state = HealthState::Up;
            }
            HealthState::Up | HealthState::Down | HealthState::Stale => {}
        }
    }

    fn record_failure(&mut self) {
        self.consecutive_successes = 0;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);

        match self.underlying_state {
            HealthState::Unknown | HealthState::Up | HealthState::Degraded
                if self.consecutive_failures >= self.config.failure_threshold =>
            {
                self.underlying_state = HealthState::Down;
            }
            HealthState::Unknown | HealthState::Up | HealthState::Degraded => {
                self.underlying_state = HealthState::Degraded;
            }
            HealthState::Down | HealthState::Stale => {}
        }
    }

    fn update_after_transition(
        &mut self,
        at: Timestamp,
        previous_state: HealthState,
        previous_underlying: HealthState,
    ) -> HealthUpdate {
        let state = self.effective_state_at(at);
        let events = if previous_state == state {
            Vec::new()
        } else {
            vec![self.event(at, previous_state, state, previous_underlying)]
        };
        self.last_reported_state = state;
        self.update_snapshot(events, state)
    }

    fn event(
        &self,
        at: Timestamp,
        from: HealthState,
        to: HealthState,
        underlying_from: HealthState,
    ) -> HealthEvent {
        let underlying_to = self.underlying_state;
        let kind = if underlying_from != HealthState::Down && underlying_to == HealthState::Down {
            HealthEventKind::IncidentOpened
        } else if underlying_from == HealthState::Down && underlying_to == HealthState::Up {
            HealthEventKind::IncidentRecovered
        } else if from != HealthState::Stale && to == HealthState::Stale {
            HealthEventKind::StaleStarted
        } else if from == HealthState::Stale && to != HealthState::Stale {
            HealthEventKind::StaleCleared
        } else {
            HealthEventKind::StateChanged
        };

        HealthEvent {
            kind,
            at,
            from,
            to,
            underlying_from,
            underlying_to,
            consecutive_failures: self.consecutive_failures,
            consecutive_successes: self.consecutive_successes,
        }
    }

    fn update_snapshot(&self, events: Vec<HealthEvent>, state: HealthState) -> HealthUpdate {
        HealthUpdate {
            state,
            underlying_state: self.underlying_state,
            incident_open: self.incident_open(),
            consecutive_failures: self.consecutive_failures,
            consecutive_successes: self.consecutive_successes,
            last_observed_at: self.last_observed_at,
            last_success_at: self.last_success_at,
            events,
        }
    }
}

impl Default for HealthStateMachine {
    fn default() -> Self {
        Self::new(HealthConfig::default())
    }
}

fn latest_timestamp(current: Option<Timestamp>, candidate: Timestamp) -> Timestamp {
    current.map_or(candidate, |current| current.max(candidate))
}

fn validate_snapshot(
    config: HealthConfig,
    snapshot: &HealthSnapshot,
) -> Result<(), HealthRestoreError> {
    let state = snapshot.underlying_state;
    if state == HealthState::Stale {
        return Err(HealthRestoreError::InvalidUnderlyingState { state });
    }

    if let (Some(last_observed_at), Some(last_success_at)) =
        (snapshot.last_observed_at, snapshot.last_success_at)
        && last_success_at > last_observed_at
    {
        return Err(HealthRestoreError::SuccessAfterObservation);
    }

    if state == HealthState::Unknown {
        let has_progress = snapshot.consecutive_failures != 0
            || snapshot.consecutive_successes != 0
            || snapshot.last_observed_at.is_some()
            || snapshot.last_success_at.is_some();
        if has_progress {
            return Err(HealthRestoreError::InvalidCounters {
                state,
                consecutive_failures: snapshot.consecutive_failures,
                consecutive_successes: snapshot.consecutive_successes,
            });
        }
        return Ok(());
    }

    if snapshot.last_observed_at.is_none() {
        return Err(HealthRestoreError::MissingObservedAt { state });
    }

    let counters_valid = match state {
        HealthState::Up => snapshot.consecutive_failures == 0 && snapshot.consecutive_successes > 0,
        HealthState::Degraded => {
            snapshot.consecutive_failures > 0
                && snapshot.consecutive_failures < config.failure_threshold
                && snapshot.consecutive_successes == 0
        }
        HealthState::Down => {
            (snapshot.consecutive_failures >= config.failure_threshold
                && snapshot.consecutive_successes == 0)
                || (snapshot.consecutive_failures == 0
                    && snapshot.consecutive_successes > 0
                    && snapshot.consecutive_successes < config.recovery_threshold)
        }
        HealthState::Unknown | HealthState::Stale => false,
    };
    if !counters_valid {
        return Err(HealthRestoreError::InvalidCounters {
            state,
            consecutive_failures: snapshot.consecutive_failures,
            consecutive_successes: snapshot.consecutive_successes,
        });
    }

    if state == HealthState::Up && snapshot.last_success_at.is_none() {
        return Err(HealthRestoreError::MissingSuccessAt { state });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("test timestamp is valid")
    }

    fn config() -> HealthConfig {
        HealthConfig::new(3, 2, Duration::from_secs(60)).expect("test config is valid")
    }

    #[test]
    fn initial_success_moves_unknown_to_up() {
        let mut machine = HealthStateMachine::new(config());

        let update = machine.observe(timestamp(0), HealthObservation::Success);

        assert_eq!(update.state, HealthState::Up);
        assert_eq!(update.underlying_state, HealthState::Up);
        assert_eq!(update.events[0].kind, HealthEventKind::StateChanged);
        assert_eq!(update.events[0].from, HealthState::Unknown);
        assert_eq!(update.events[0].to, HealthState::Up);
    }

    #[test]
    fn transient_failure_stays_below_incident_threshold() {
        let mut machine = HealthStateMachine::new(config());
        machine.observe(timestamp(0), HealthObservation::Success);

        let update = machine.observe(timestamp(1), HealthObservation::Failure);

        assert_eq!(update.state, HealthState::Degraded);
        assert!(!update.incident_open);
        assert_eq!(update.events[0].kind, HealthEventKind::StateChanged);
        assert!(
            !update
                .events
                .iter()
                .any(|event| event.kind == HealthEventKind::IncidentOpened)
        );
    }

    #[test]
    fn outage_opens_once_at_failure_threshold() {
        let mut machine = HealthStateMachine::new(config());
        machine.observe(timestamp(0), HealthObservation::Success);
        machine.observe(timestamp(1), HealthObservation::Failure);
        machine.observe(timestamp(2), HealthObservation::Failure);

        let opened = machine.observe(timestamp(3), HealthObservation::Failure);
        let repeated = machine.observe(timestamp(4), HealthObservation::Failure);

        assert_eq!(opened.state, HealthState::Down);
        assert!(opened.incident_open);
        assert_eq!(opened.events[0].kind, HealthEventKind::IncidentOpened);
        assert!(repeated.events.is_empty());
        assert!(machine.incident_open());
    }

    #[test]
    fn recovery_waits_for_success_threshold_and_emits_once() {
        let mut machine = HealthStateMachine::new(config());
        machine.observe(timestamp(0), HealthObservation::Success);
        machine.observe(timestamp(1), HealthObservation::Failure);
        machine.observe(timestamp(2), HealthObservation::Failure);
        machine.observe(timestamp(3), HealthObservation::Failure);

        let first_success = machine.observe(timestamp(4), HealthObservation::Success);
        let recovered = machine.observe(timestamp(5), HealthObservation::Success);
        let repeated = machine.observe(timestamp(6), HealthObservation::Success);

        assert_eq!(first_success.state, HealthState::Down);
        assert!(first_success.events.is_empty());
        assert_eq!(recovered.state, HealthState::Up);
        assert!(!recovered.incident_open);
        assert_eq!(recovered.events[0].kind, HealthEventKind::IncidentRecovered);
        assert!(repeated.events.is_empty());
    }

    #[test]
    fn stale_is_derived_without_destroying_underlying_state() {
        let mut machine = HealthStateMachine::new(config());
        machine.observe(timestamp(0), HealthObservation::Success);

        let stale = machine.evaluate(timestamp(60));
        let repeated = machine.evaluate(timestamp(61));
        let fresh = machine.observe(timestamp(62), HealthObservation::Success);

        assert_eq!(stale.state, HealthState::Stale);
        assert_eq!(stale.underlying_state, HealthState::Up);
        assert_eq!(stale.events[0].kind, HealthEventKind::StaleStarted);
        assert!(repeated.events.is_empty());
        assert_eq!(machine.state(), HealthState::Up);
        assert_eq!(fresh.state, HealthState::Up);
        assert_eq!(fresh.events[0].kind, HealthEventKind::StaleCleared);
    }

    #[test]
    fn snapshot_round_trip_preserves_recovery_progress_and_future_events() {
        let mut original = HealthStateMachine::new(config());
        original.observe(timestamp(0), HealthObservation::Success);
        original.observe(timestamp(1), HealthObservation::Failure);
        original.observe(timestamp(2), HealthObservation::Failure);
        original.observe(timestamp(3), HealthObservation::Failure);
        original.observe(timestamp(4), HealthObservation::Success);

        let snapshot = original.snapshot();
        let mut restored = HealthStateMachine::restore(config(), snapshot.clone())
            .expect("snapshot from state machine is valid");

        assert_eq!(restored.snapshot(), snapshot);
        assert_eq!(restored.state(), HealthState::Down);

        let original_update = original.observe(timestamp(5), HealthObservation::Success);
        let restored_update = restored.observe(timestamp(5), HealthObservation::Success);

        assert_eq!(restored_update, original_update);
        assert_eq!(
            restored_update.events[0].kind,
            HealthEventKind::IncidentRecovered
        );
    }

    #[test]
    fn restore_rejects_stale_state_and_invalid_counters() {
        let stale = HealthSnapshot {
            underlying_state: HealthState::Stale,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_observed_at: Some(timestamp(0)),
            last_success_at: None,
        };
        let stale_error =
            HealthStateMachine::restore(config(), stale).expect_err("stale is derived");
        assert_eq!(
            stale_error,
            HealthRestoreError::InvalidUnderlyingState {
                state: HealthState::Stale
            }
        );

        let invalid_counters = HealthSnapshot {
            underlying_state: HealthState::Up,
            consecutive_failures: 1,
            consecutive_successes: 0,
            last_observed_at: Some(timestamp(0)),
            last_success_at: Some(timestamp(0)),
        };
        let counter_error =
            HealthStateMachine::restore(config(), invalid_counters).expect_err("up cannot fail");
        assert!(matches!(
            counter_error,
            HealthRestoreError::InvalidCounters {
                state: HealthState::Up,
                ..
            }
        ));
    }

    #[test]
    fn invalid_config_is_rejected() {
        assert_eq!(
            HealthConfig::new(0, 1, Duration::from_secs(1)),
            Err(HealthConfigError::ZeroFailureThreshold)
        );
        assert_eq!(
            HealthConfig::new(1, 0, Duration::from_secs(1)),
            Err(HealthConfigError::ZeroRecoveryThreshold)
        );
        assert_eq!(
            HealthConfig::new(1, 1, Duration::ZERO),
            Err(HealthConfigError::ZeroStaleAfter)
        );
        assert_eq!(
            HealthConfig::new(1, 1, MAX_STALE_AFTER + Duration::from_secs(1)),
            Err(HealthConfigError::StaleAfterTooLong)
        );
    }
}
