//! Resolved scan policy and deterministic low-impact pacing.
//!
//! Scope rows store operator limits. This module resolves those limits into a
//! run policy without ever increasing them. Low-impact runs keep the complete
//! TCP port plan, but reduce fan-out and add bounded pacing between probes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Existing worker bound for open-port classification in the normal profile.
pub const NORMAL_CLASSIFICATION_CONCURRENCY: usize = 8;
/// Maximum global and per-network TCP fan-out for low-impact runs.
pub const LOW_IMPACT_TCP_CONCURRENCY_CAP: usize = 8;
/// Maximum TCP fan-out to one host for low-impact runs.
pub const LOW_IMPACT_PER_HOST_CONCURRENCY_CAP: usize = 1;
/// Maximum open-port classifier fan-out for low-impact runs.
pub const LOW_IMPACT_CLASSIFICATION_CONCURRENCY: usize = 2;

const LOW_IMPACT_PROBE_INTERVAL: Duration = Duration::from_millis(10);
const LOW_IMPACT_JITTER: Duration = Duration::from_millis(10);
const LOW_IMPACT_BACKOFF_STEP: Duration = Duration::from_millis(5);
const LOW_IMPACT_BACKOFF_MAX: Duration = Duration::from_millis(20);
const LOW_IMPACT_CLASSIFICATION_INTERVAL: Duration = Duration::from_millis(20);
const LOW_IMPACT_CLASSIFICATION_JITTER: Duration = Duration::from_millis(10);
const LOW_IMPACT_CLASSIFICATION_BACKOFF_STEP: Duration = Duration::from_millis(5);
const LOW_IMPACT_CLASSIFICATION_BACKOFF_MAX: Duration = Duration::from_millis(20);

/// Operator-selected scan profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanProfile {
    Normal,
    LowImpact,
}

impl ScanProfile {
    pub fn parse(value: &str) -> Result<Self, PolicyError> {
        match value {
            "normal" => Ok(Self::Normal),
            "low_impact" => Ok(Self::LowImpact),
            _ => Err(PolicyError::InvalidProfile(value.to_string())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::LowImpact => "low_impact",
        }
    }
}

/// Errors from resolving persisted scope policy.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("unknown scan profile `{0}`")]
    InvalidProfile(String),
    #[error("stored {0} must be greater than zero")]
    InvalidLimit(&'static str),
}

/// Pacing applied before one probe starts.
///
/// `interval` spaces starts. `jitter` is a deterministic bounded offset. The
/// backoff sequence is capped, so a full scan remains finite and predictable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacingConfig {
    interval: Duration,
    jitter: Duration,
    backoff_step: Duration,
    backoff_max: Duration,
}

impl PacingConfig {
    pub const fn disabled() -> Self {
        Self {
            interval: Duration::ZERO,
            jitter: Duration::ZERO,
            backoff_step: Duration::ZERO,
            backoff_max: Duration::ZERO,
        }
    }

    const fn low_impact_tcp() -> Self {
        Self {
            interval: LOW_IMPACT_PROBE_INTERVAL,
            jitter: LOW_IMPACT_JITTER,
            backoff_step: LOW_IMPACT_BACKOFF_STEP,
            backoff_max: LOW_IMPACT_BACKOFF_MAX,
        }
    }

    const fn low_impact_classification() -> Self {
        Self {
            interval: LOW_IMPACT_CLASSIFICATION_INTERVAL,
            jitter: LOW_IMPACT_CLASSIFICATION_JITTER,
            backoff_step: LOW_IMPACT_CLASSIFICATION_BACKOFF_STEP,
            backoff_max: LOW_IMPACT_CLASSIFICATION_BACKOFF_MAX,
        }
    }

    pub const fn interval(self) -> Duration {
        self.interval
    }

    pub const fn jitter(self) -> Duration {
        self.jitter
    }

    pub const fn backoff_step(self) -> Duration {
        self.backoff_step
    }

    pub const fn backoff_max(self) -> Duration {
        self.backoff_max
    }

    fn backoff_for(self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }
        let factor = 1_u32 << (attempt - 1).min(3);
        self.backoff_step
            .checked_mul(factor)
            .unwrap_or(self.backoff_max)
            .min(self.backoff_max)
    }

    fn is_disabled(self) -> bool {
        self.interval.is_zero() && self.jitter.is_zero() && self.backoff_max.is_zero()
    }
}

/// Effective policy for one scan run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedScanPolicy {
    profile: ScanProfile,
    tcp_global_concurrency: usize,
    tcp_network_concurrency: usize,
    tcp_per_host_concurrency: usize,
    classification_concurrency: usize,
    tcp_pacing: PacingConfig,
    classification_pacing: PacingConfig,
}

impl ResolvedScanPolicy {
    pub const fn profile(self) -> ScanProfile {
        self.profile
    }

    pub const fn tcp_global_concurrency(self) -> usize {
        self.tcp_global_concurrency
    }

    pub const fn tcp_network_concurrency(self) -> usize {
        self.tcp_network_concurrency
    }

    pub const fn tcp_per_host_concurrency(self) -> usize {
        self.tcp_per_host_concurrency
    }

    pub const fn classification_concurrency(self) -> usize {
        self.classification_concurrency
    }

    pub const fn tcp_pacing(self) -> PacingConfig {
        self.tcp_pacing
    }

    pub const fn classification_pacing(self) -> PacingConfig {
        self.classification_pacing
    }
}

/// Resolve persisted operator limits into an explicit run policy.
///
/// Low-impact caps are reductions, never overrides that increase stored
/// limits. The port plan remains unchanged: policy controls pressure only.
pub fn resolve_scan_policy(
    profile: &str,
    stored_tcp_concurrency: usize,
    stored_per_host_concurrency: usize,
) -> Result<ResolvedScanPolicy, PolicyError> {
    if stored_tcp_concurrency == 0 {
        return Err(PolicyError::InvalidLimit("TCP concurrency"));
    }
    if stored_per_host_concurrency == 0 {
        return Err(PolicyError::InvalidLimit("per-host concurrency"));
    }

    let profile = ScanProfile::parse(profile)?;
    let classification_upper_bound = stored_tcp_concurrency.min(stored_per_host_concurrency);
    let policy = match profile {
        ScanProfile::Normal => ResolvedScanPolicy {
            profile,
            tcp_global_concurrency: stored_tcp_concurrency,
            tcp_network_concurrency: stored_tcp_concurrency,
            tcp_per_host_concurrency: stored_per_host_concurrency,
            classification_concurrency: NORMAL_CLASSIFICATION_CONCURRENCY
                .min(classification_upper_bound),
            tcp_pacing: PacingConfig::disabled(),
            classification_pacing: PacingConfig::disabled(),
        },
        ScanProfile::LowImpact => ResolvedScanPolicy {
            profile,
            tcp_global_concurrency: stored_tcp_concurrency.min(LOW_IMPACT_TCP_CONCURRENCY_CAP),
            tcp_network_concurrency: stored_tcp_concurrency.min(LOW_IMPACT_TCP_CONCURRENCY_CAP),
            tcp_per_host_concurrency: stored_per_host_concurrency
                .min(LOW_IMPACT_PER_HOST_CONCURRENCY_CAP),
            classification_concurrency: LOW_IMPACT_CLASSIFICATION_CONCURRENCY
                .min(classification_upper_bound),
            tcp_pacing: PacingConfig::low_impact_tcp(),
            classification_pacing: PacingConfig::low_impact_classification(),
        },
    };

    Ok(policy)
}

/// Clock seam used by [`ScanPacer`].
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Sleep seam used by [`ScanPacer`].
#[async_trait]
pub trait Sleeper: Send + Sync {
    async fn sleep_until(&self, deadline: Instant);
}

/// Randomness seam used to derive bounded jitter.
pub trait JitterSource: Send + Sync {
    fn sample(&self, seed: u64) -> u64;
}

/// Tokio-backed production clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Tokio-backed production sleeper.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep_until(&self, deadline: Instant) {
        tokio::time::sleep_until(deadline).await;
    }
}

/// Deterministic pseudo-random source. Seed includes scan-run identity, so
/// concurrent runs do not share one synchronized jitter sequence.
#[derive(Debug, Clone, Copy)]
pub struct DeterministicJitter {
    seed: u64,
}

impl DeterministicJitter {
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }
}

impl JitterSource for DeterministicJitter {
    fn sample(&self, seed: u64) -> u64 {
        split_mix64(self.seed ^ seed)
    }
}

/// Shared pacing coordinator for one scan phase.
pub struct ScanPacer {
    config: PacingConfig,
    clock: Arc<dyn Clock>,
    jitter: Arc<dyn JitterSource>,
    sleeper: Arc<dyn Sleeper>,
    next_start: Mutex<Option<Instant>>,
}

impl ScanPacer {
    /// Build production pacing with Tokio time and deterministic jitter.
    pub fn production(config: PacingConfig, seed: u64) -> Self {
        Self::with_seams(
            config,
            Arc::new(TokioClock),
            Arc::new(DeterministicJitter::new(seed)),
            Arc::new(TokioSleeper),
        )
    }

    /// Build pacing with injected time, jitter, and sleep seams.
    pub fn with_seams(
        config: PacingConfig,
        clock: Arc<dyn Clock>,
        jitter: Arc<dyn JitterSource>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Self {
        Self {
            config,
            clock,
            jitter,
            sleeper,
            next_start: Mutex::new(None),
        }
    }

    /// Wait for one bounded, ordered probe slot.
    ///
    /// `attempt` is capped by the pacing config. Callers can use batch or
    /// retry number; it never causes unbounded exponential delay.
    pub async fn wait(&self, seed: u64, attempt: u32) {
        if self.config.is_disabled() {
            return;
        }

        let now = self.clock.now();
        let mut next_start = self.next_start.lock().await;
        let scheduled = next_start.map_or(now, |next| next.max(now));
        let jitter = bounded_duration(self.jitter.sample(seed), self.config.jitter);
        let deadline = scheduled + jitter + self.config.backoff_for(attempt);
        *next_start = Some(deadline + self.config.interval);
        drop(next_start);

        if deadline > now {
            self.sleeper.sleep_until(deadline).await;
        }
    }
}

/// Stable per-probe seed. Different run identities produce different jitter
/// schedules while retrying one run preserves its schedule.
pub fn probe_seed(run_seed: u64, target_index: usize, batch_index: usize, port: u16) -> u64 {
    let value = run_seed
        .wrapping_add((target_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add((batch_index as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(u64::from(port));
    split_mix64(value)
}

fn bounded_duration(sample: u64, maximum: Duration) -> Duration {
    let maximum_nanos = maximum.as_nanos().min(u128::from(u64::MAX)) as u64;
    if maximum_nanos == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(sample % maximum_nanos.saturating_add(1))
}

fn split_mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    #[test]
    fn low_impact_only_reduces_operator_limits() {
        let policy = resolve_scan_policy("low_impact", 64, 2).expect("valid policy");
        assert_eq!(policy.tcp_global_concurrency(), 8);
        assert_eq!(policy.tcp_network_concurrency(), 8);
        assert_eq!(policy.tcp_per_host_concurrency(), 1);
        assert_eq!(policy.classification_concurrency(), 2);

        let already_low = resolve_scan_policy("low_impact", 1, 1).expect("valid policy");
        assert_eq!(already_low.tcp_global_concurrency(), 1);
        assert_eq!(already_low.tcp_network_concurrency(), 1);
        assert_eq!(already_low.tcp_per_host_concurrency(), 1);
        assert_eq!(already_low.classification_concurrency(), 1);
    }

    #[test]
    fn normal_profile_preserves_stored_tcp_limits() {
        let policy = resolve_scan_policy("normal", 37, 8).expect("valid policy");
        assert_eq!(policy.tcp_global_concurrency(), 37);
        assert_eq!(policy.tcp_network_concurrency(), 37);
        assert_eq!(policy.tcp_per_host_concurrency(), 8);
        assert_eq!(
            policy.classification_concurrency(),
            NORMAL_CLASSIFICATION_CONCURRENCY
        );
        assert_eq!(policy.tcp_pacing(), PacingConfig::disabled());

        let constrained = resolve_scan_policy("normal", 1, 1).expect("valid policy");
        assert_eq!(constrained.classification_concurrency(), 1);
    }

    #[test]
    fn invalid_profile_or_limit_fails_closed() {
        assert!(matches!(
            resolve_scan_policy("quiet", 1, 1),
            Err(PolicyError::InvalidProfile(_))
        ));
        assert_eq!(
            resolve_scan_policy("normal", 0, 1),
            Err(PolicyError::InvalidLimit("TCP concurrency"))
        );
    }

    struct FixedClock {
        now: Instant,
    }

    impl Clock for FixedClock {
        fn now(&self) -> Instant {
            self.now
        }
    }

    struct FixedJitter(u64);

    impl JitterSource for FixedJitter {
        fn sample(&self, _seed: u64) -> u64 {
            self.0
        }
    }

    struct RecordingSleeper {
        deadlines: StdMutex<Vec<Instant>>,
    }

    #[async_trait]
    impl Sleeper for RecordingSleeper {
        async fn sleep_until(&self, deadline: Instant) {
            self.deadlines.lock().expect("deadline lock").push(deadline);
        }
    }

    #[tokio::test]
    async fn pacing_is_deterministic_and_bounded_with_injected_seams() {
        let now = Instant::now();
        let sleeper = Arc::new(RecordingSleeper {
            deadlines: StdMutex::new(Vec::new()),
        });
        let config = PacingConfig {
            interval: Duration::from_millis(10),
            jitter: Duration::from_millis(10),
            backoff_step: Duration::from_millis(5),
            backoff_max: Duration::from_millis(20),
        };
        let pacer = ScanPacer::with_seams(
            config,
            Arc::new(FixedClock { now }),
            Arc::new(FixedJitter(u64::MAX)),
            Arc::clone(&sleeper) as Arc<dyn Sleeper>,
        );

        pacer.wait(1, 0).await;
        pacer.wait(2, 1).await;
        pacer.wait(3, 99).await;

        let deadlines = sleeper.deadlines.lock().expect("deadline lock");
        assert_eq!(deadlines.len(), 3);
        assert!(deadlines[0].duration_since(now) <= Duration::from_millis(10));
        assert!(deadlines[1] > deadlines[0]);
        assert!(deadlines[2] > deadlines[1]);
        assert!(deadlines[2].duration_since(deadlines[1]) <= Duration::from_millis(40));
    }

    #[test]
    fn probe_seeds_change_by_run_and_port() {
        assert_ne!(probe_seed(1, 0, 0, 80), probe_seed(2, 0, 0, 80));
        assert_ne!(probe_seed(1, 0, 0, 80), probe_seed(1, 0, 0, 443));
        assert_eq!(probe_seed(1, 0, 0, 80), probe_seed(1, 0, 0, 80));
    }
}
