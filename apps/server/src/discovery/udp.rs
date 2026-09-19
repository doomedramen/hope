//! Targeted UDP discovery primitives.
//!
//! UDP probes are always explicit. Callers provide each destination port and
//! safe protocol payload; this module never expands a target into a full UDP
//! port range. Silence is represented as `open_or_filtered` because UDP has no
//! TCP-like handshake that can prove a port is closed.

// M2 exposes this seam before a future targeted-UDP job orchestrator wires it
// into the worker registry. Keep unused public framework items lint-clean.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use sqlx::PgPool;
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

/// UDP probe result states persisted in `port_observations.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    /// The target returned a UDP datagram.
    Open,
    /// The target returned an ICMP port-unreachable error.
    Closed,
    /// No response arrived, or the transport could not prove either state.
    OpenOrFiltered,
}

/// Alias that makes the UDP-specific state type clear at call sites that also
/// import the TCP discovery module.
pub type UdpPortState = PortState;

/// One targeted UDP probe. One configured payload is used for one destination
/// port. Duplicate ports in one scan are de-duplicated, preserving first use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpProbe {
    port: u16,
    payload: Vec<u8>,
}

impl UdpProbe {
    /// Build a probe for one configured UDP port.
    pub fn new(port: u16, payload: impl Into<Vec<u8>>) -> Result<Self, ScannerError> {
        validate_port(port)?;
        Ok(Self {
            port,
            payload: payload.into(),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// One UDP port observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortObservation {
    pub address: IpAddr,
    pub port: u16,
    pub state: PortState,
    pub latency: Duration,
}

/// Alias for consumers that need to distinguish this from TCP observations.
pub type UdpPortObservation = PortObservation;

/// Errors caused by invalid scanner setup or invalid targeted input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScannerError {
    #[error("UDP probe timeout must be greater than zero")]
    InvalidTimeout,
    #[error("{name} concurrency must be greater than zero")]
    InvalidConcurrency { name: &'static str },
    #[error("UDP port 0 is not a valid scan target")]
    InvalidPort { port: u16 },
    #[error("network key must not be empty")]
    InvalidNetworkKey,
    #[error("scanner concurrency limiter closed")]
    LimiterClosed,
    #[error("scanner returned no observation")]
    NoObservation,
}

/// Limits for one [`UdpScanner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpScannerConfig {
    probe_timeout: Duration,
    global_concurrency: usize,
    network_concurrency: usize,
    per_host_concurrency: usize,
    probe_interval: Duration,
}

impl UdpScannerConfig {
    /// Build a bounded scanner configuration.
    ///
    /// The default interval spaces probes by 10 ms. Use
    /// [`Self::with_probe_interval`] to select another explicit rate policy.
    pub fn new(
        probe_timeout: Duration,
        global_concurrency: usize,
        network_concurrency: usize,
        per_host_concurrency: usize,
    ) -> Result<Self, ScannerError> {
        Self::with_probe_interval(
            probe_timeout,
            global_concurrency,
            network_concurrency,
            per_host_concurrency,
            Duration::from_millis(10),
        )
    }

    /// Build a bounded scanner configuration with an explicit minimum delay
    /// between probes. Zero disables pacing and is intended for tests or a
    /// caller that supplies an equivalent external rate limiter.
    pub fn with_probe_interval(
        probe_timeout: Duration,
        global_concurrency: usize,
        network_concurrency: usize,
        per_host_concurrency: usize,
        probe_interval: Duration,
    ) -> Result<Self, ScannerError> {
        if probe_timeout.is_zero() {
            return Err(ScannerError::InvalidTimeout);
        }
        for (name, value) in [
            ("global", global_concurrency),
            ("network", network_concurrency),
            ("per-host", per_host_concurrency),
        ] {
            if value == 0 {
                return Err(ScannerError::InvalidConcurrency { name });
            }
        }
        Ok(Self {
            probe_timeout,
            global_concurrency,
            network_concurrency,
            per_host_concurrency,
            probe_interval,
        })
    }

    pub fn probe_timeout(self) -> Duration {
        self.probe_timeout
    }

    pub fn global_concurrency(self) -> usize {
        self.global_concurrency
    }

    pub fn network_concurrency(self) -> usize {
        self.network_concurrency
    }

    pub fn per_host_concurrency(self) -> usize {
        self.per_host_concurrency
    }

    pub fn probe_interval(self) -> Duration {
        self.probe_interval
    }
}

impl Default for UdpScannerConfig {
    fn default() -> Self {
        Self {
            probe_timeout: Duration::from_secs(1),
            global_concurrency: 64,
            network_concurrency: 64,
            per_host_concurrency: 2,
            probe_interval: Duration::from_millis(10),
        }
    }
}

/// Injectable UDP transport. Production uses [`TokioUdpTransport`]; tests or
/// future protocol-specific transports can return deterministic outcomes.
#[async_trait]
pub trait UdpTransport: Send + Sync {
    /// Send one configured payload and return one received datagram.
    ///
    /// `ConnectionRefused` or `ConnectionReset` means ICMP port unreachable.
    /// Other errors remain ambiguous and become `open_or_filtered`.
    async fn probe(&self, target: SocketAddr, payload: &[u8]) -> io::Result<Vec<u8>>;
}

/// Native UDP transport. It binds an ephemeral local socket, sends the
/// configured payload, and waits for one response.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioUdpTransport;

#[async_trait]
impl UdpTransport for TokioUdpTransport {
    async fn probe(&self, target: SocketAddr, payload: &[u8]) -> io::Result<Vec<u8>> {
        let local = match target {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(target).await?;
        socket.send(payload).await?;

        let mut response = vec![0_u8; 4096];
        let received = socket.recv(&mut response).await?;
        response.truncate(received);
        Ok(response)
    }
}

/// Scanner for explicitly configured UDP probes.
pub struct UdpScanner {
    config: UdpScannerConfig,
    global_limiter: Arc<Semaphore>,
    network_limiters: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
    host_limiters: Arc<Mutex<HashMap<IpAddr, Weak<Semaphore>>>>,
    next_probe: Arc<Mutex<tokio::time::Instant>>,
    transport: Arc<dyn UdpTransport>,
}

impl UdpScanner {
    pub fn new(config: UdpScannerConfig) -> Result<Self, ScannerError> {
        Self::with_transport(config, Arc::new(TokioUdpTransport))
    }

    /// Construct a scanner with an injected transport.
    pub fn with_transport(
        config: UdpScannerConfig,
        transport: Arc<dyn UdpTransport>,
    ) -> Result<Self, ScannerError> {
        let config = UdpScannerConfig::with_probe_interval(
            config.probe_timeout,
            config.global_concurrency,
            config.network_concurrency,
            config.per_host_concurrency,
            config.probe_interval,
        )?;
        Ok(Self {
            global_limiter: Arc::new(Semaphore::new(config.global_concurrency)),
            network_limiters: Arc::new(Mutex::new(HashMap::new())),
            host_limiters: Arc::new(Mutex::new(HashMap::new())),
            next_probe: Arc::new(Mutex::new(tokio::time::Instant::now())),
            config,
            transport,
        })
    }

    /// Probe one explicit UDP endpoint under the scanner's global limit.
    pub async fn scan(
        &self,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        validate_port(target.port())?;
        let _global = self
            .global_limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ScannerError::LimiterClosed)?;
        self.probe(target, payload).await
    }

    /// Probe one explicit UDP endpoint under global, network, and per-host
    /// bounds.
    pub async fn scan_scoped(
        &self,
        network_key: &str,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        let probe = UdpProbe::new(target.port(), payload.to_vec())?;
        self.scan_probes(network_key.to_owned(), target.ip(), &[probe])
            .await?
            .into_iter()
            .next()
            .ok_or(ScannerError::NoObservation)
    }

    /// Scan one host using only the explicitly supplied configured probes.
    /// Results preserve first-use port order. Duplicate ports are probed once.
    pub async fn scan_probes(
        &self,
        network_key: impl Into<String>,
        address: IpAddr,
        probes: &[UdpProbe],
    ) -> Result<Vec<PortObservation>, ScannerError> {
        let network_key = network_key.into();
        if network_key.trim().is_empty() {
            return Err(ScannerError::InvalidNetworkKey);
        }
        let probes = unique_probes(probes)?;
        if probes.is_empty() {
            return Ok(Vec::new());
        }

        let network_limiter = semaphore_for(
            &self.network_limiters,
            network_key,
            self.config.network_concurrency,
        )
        .await;
        let host_limiter = semaphore_for(
            &self.host_limiters,
            address,
            self.config.per_host_concurrency,
        )
        .await;
        let max_in_flight = self.config.global_concurrency;

        stream::iter(probes.into_iter().map(|probe| {
            let scanner = self.clone();
            let network_limiter = Arc::clone(&network_limiter);
            let host_limiter = Arc::clone(&host_limiter);
            async move {
                // Acquisition order matches TCP: global, network, host.
                let _global = scanner
                    .global_limiter
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| ScannerError::LimiterClosed)?;
                let _network = network_limiter
                    .acquire_owned()
                    .await
                    .map_err(|_| ScannerError::LimiterClosed)?;
                let _host = host_limiter
                    .acquire_owned()
                    .await
                    .map_err(|_| ScannerError::LimiterClosed)?;
                scanner
                    .probe(SocketAddr::new(address, probe.port), probe.payload())
                    .await
            }
        }))
        .buffered(max_in_flight)
        .try_collect()
        .await
    }

    async fn probe(
        &self,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        let started = tokio::time::Instant::now();
        self.wait_for_rate().await;
        let state = match tokio::time::timeout(
            self.config.probe_timeout,
            self.transport.probe(target, payload),
        )
        .await
        {
            Ok(Ok(_response)) => PortState::Open,
            Ok(Err(error)) if is_port_unreachable(&error) => PortState::Closed,
            Ok(Err(_)) | Err(_) => PortState::OpenOrFiltered,
        };
        Ok(PortObservation {
            address: target.ip(),
            port: target.port(),
            state,
            latency: started.elapsed(),
        })
    }

    async fn wait_for_rate(&self) {
        let interval = self.config.probe_interval;
        if interval.is_zero() {
            return;
        }
        let now = tokio::time::Instant::now();
        let mut next_probe = self.next_probe.lock().await;
        let start = (*next_probe).max(now);
        *next_probe = start + interval;
        drop(next_probe);
        if start > now {
            tokio::time::sleep_until(start).await;
        }
    }
}

impl Clone for UdpScanner {
    fn clone(&self) -> Self {
        Self {
            config: self.config,
            global_limiter: Arc::clone(&self.global_limiter),
            network_limiters: Arc::clone(&self.network_limiters),
            host_limiters: Arc::clone(&self.host_limiters),
            next_probe: Arc::clone(&self.next_probe),
            transport: Arc::clone(&self.transport),
        }
    }
}

/// Generic UDP scanner seam used by workers and alternate scanners.
#[async_trait]
pub trait Scanner: Send + Sync {
    async fn scan(
        &self,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError>;

    async fn scan_scoped(
        &self,
        _network_key: &str,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        self.scan(target, payload).await
    }
}

#[async_trait]
impl Scanner for UdpScanner {
    async fn scan(
        &self,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        UdpScanner::scan(self, target, payload).await
    }

    async fn scan_scoped(
        &self,
        network_key: &str,
        target: SocketAddr,
        payload: &[u8],
    ) -> Result<PortObservation, ScannerError> {
        UdpScanner::scan_scoped(self, network_key, target, payload).await
    }
}

/// Database transport name for UDP observations.
pub const UDP_TRANSPORT: &str = "udp";

/// Map UDP state to the existing `port_observations.state` values.
pub const fn port_state_name(state: PortState) -> &'static str {
    match state {
        PortState::Open => "open",
        PortState::Closed => "closed",
        PortState::OpenOrFiltered => "open_or_filtered",
    }
}

/// Persist UDP observations without changing TCP run application behavior.
///
/// Run progress, cancellation, and completeness remain the caller's policy.
/// This helper only writes rows with `transport = 'udp'`; it never infers
/// closure from an unvisited port.
pub async fn persist_observations(
    pool: &PgPool,
    scan_run_id: Uuid,
    observations: &[PortObservation],
    device_ids: &HashMap<IpAddr, Uuid>,
) -> AnyhowResult<()> {
    let mut transaction = pool.begin().await?;
    for observation in observations {
        let latency_ms = i32::try_from(observation.latency.as_millis()).unwrap_or(i32::MAX);
        sqlx::query(
            "insert into port_observations \
                (scan_run_id, device_id, address, port, transport, state, latency_ms) \
             values ($1, $2, $3::inet, $4, $5, $6, $7) \
             on conflict (scan_run_id, address, port, transport) do nothing",
        )
        .bind(scan_run_id)
        .bind(device_ids.get(&observation.address).copied())
        .bind(observation.address.to_string())
        .bind(i32::from(observation.port))
        .bind(UDP_TRANSPORT)
        .bind(port_state_name(observation.state))
        .bind(latency_ms)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

fn is_port_unreachable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
    )
}

fn validate_port(port: u16) -> Result<(), ScannerError> {
    if port == 0 {
        return Err(ScannerError::InvalidPort { port });
    }
    Ok(())
}

fn unique_probes(probes: &[UdpProbe]) -> Result<Vec<UdpProbe>, ScannerError> {
    let mut seen = HashSet::with_capacity(probes.len());
    let mut unique = Vec::with_capacity(probes.len());
    for probe in probes {
        validate_port(probe.port)?;
        if seen.insert(probe.port) {
            unique.push(probe.clone());
        }
    }
    Ok(unique)
}

async fn semaphore_for<K>(
    registry: &Mutex<HashMap<K, Weak<Semaphore>>>,
    key: K,
    limit: usize,
) -> Arc<Semaphore>
where
    K: Eq + Hash,
{
    let mut registry = registry.lock().await;
    registry.retain(|_, semaphore| semaphore.strong_count() > 0);
    if let Some(semaphore) = registry.get(&key).and_then(Weak::upgrade) {
        return semaphore;
    }
    let semaphore = Arc::new(Semaphore::new(limit));
    registry.insert(key, Arc::downgrade(&semaphore));
    semaphore
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::net::UdpSocket;

    use super::*;

    #[tokio::test]
    async fn scanner_reports_local_udp_reply_as_open() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind UDP test server");
        let target = server.local_addr().expect("read UDP test address");
        let responder = tokio::spawn(async move {
            let mut request = [0_u8; 32];
            let (size, peer) = server
                .recv_from(&mut request)
                .await
                .expect("receive UDP probe");
            assert_eq!(&request[..size], b"safe-probe");
            server
                .send_to(b"reply", peer)
                .await
                .expect("send UDP reply");
        });
        let scanner = UdpScanner::new(test_config()).expect("valid scanner config");

        let observation = scanner
            .scan(target, b"safe-probe")
            .await
            .expect("scan local UDP reply");
        responder.await.expect("UDP responder task");

        assert_eq!(observation.state, PortState::Open);
        assert_eq!(observation.address, target.ip());
        assert_eq!(observation.port, target.port());
    }

    #[tokio::test]
    async fn injected_transport_preserves_closed_and_ambiguous_states() {
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, 5353));
        for (outcome, expected) in [
            (FakeOutcome::Closed, PortState::Closed),
            (FakeOutcome::Ambiguous, PortState::OpenOrFiltered),
        ] {
            let transport = Arc::new(FakeTransport::new(outcome));
            let scanner = UdpScanner::with_transport(
                test_config(),
                Arc::clone(&transport) as Arc<dyn UdpTransport>,
            )
            .expect("valid scanner config");

            let observation = scanner
                .scan(target, b"safe-probe")
                .await
                .expect("scan injected UDP result");

            assert_eq!(observation.state, expected);
        }
    }

    #[tokio::test]
    async fn timeout_is_ambiguous_and_bounded() {
        let transport = Arc::new(FakeTransport::new(FakeOutcome::Delay(
            Duration::from_millis(50),
        )));
        let config = UdpScannerConfig::with_probe_interval(
            Duration::from_millis(5),
            2,
            2,
            2,
            Duration::ZERO,
        )
        .expect("valid scanner config");
        let scanner = UdpScanner::with_transport(config, transport).expect("valid scanner");

        let started = tokio::time::Instant::now();
        let observation = scanner
            .scan(SocketAddr::from((Ipv4Addr::LOCALHOST, 5353)), b"safe-probe")
            .await
            .expect("scan timed-out UDP probe");

        assert_eq!(observation.state, PortState::OpenOrFiltered);
        assert!(started.elapsed() < Duration::from_millis(30));
    }

    #[tokio::test]
    async fn explicit_probe_list_is_deduplicated_and_concurrency_is_bounded() {
        let transport = Arc::new(FakeTransport::new(FakeOutcome::Open));
        let config =
            UdpScannerConfig::with_probe_interval(Duration::from_secs(1), 2, 2, 2, Duration::ZERO)
                .expect("valid scanner config");
        let scanner =
            UdpScanner::with_transport(config, Arc::clone(&transport) as Arc<dyn UdpTransport>)
                .expect("valid scanner");
        let probes = vec![
            UdpProbe::new(53, b"dns".to_vec()).expect("valid probe"),
            UdpProbe::new(53, b"duplicate".to_vec()).expect("valid probe"),
            UdpProbe::new(123, b"ntp".to_vec()).expect("valid probe"),
            UdpProbe::new(161, b"snmp".to_vec()).expect("valid probe"),
        ];

        let observations = scanner
            .scan_probes("test-network", IpAddr::V4(Ipv4Addr::LOCALHOST), &probes)
            .await
            .expect("scan explicit UDP probes");
        let no_default_observations = scanner
            .scan_probes("test-network", IpAddr::V4(Ipv4Addr::LOCALHOST), &[])
            .await
            .expect("scan with no configured UDP probes");

        assert_eq!(observations.len(), 3);
        assert!(no_default_observations.is_empty());
        assert_eq!(transport.calls(), 3);
        assert!(transport.maximum.load(Ordering::Relaxed) <= 2);
    }

    #[tokio::test]
    async fn probe_interval_spaces_explicit_probes() {
        let transport = Arc::new(FakeTransport::new(FakeOutcome::Open));
        let config = UdpScannerConfig::with_probe_interval(
            Duration::from_secs(1),
            3,
            3,
            3,
            Duration::from_millis(5),
        )
        .expect("valid scanner config");
        let scanner = UdpScanner::with_transport(config, transport).expect("valid scanner");
        let probes = [
            UdpProbe::new(53, b"dns".to_vec()).expect("valid probe"),
            UdpProbe::new(123, b"ntp".to_vec()).expect("valid probe"),
            UdpProbe::new(161, b"snmp".to_vec()).expect("valid probe"),
        ];

        let started = tokio::time::Instant::now();
        scanner
            .scan_probes("test-network", IpAddr::V4(Ipv4Addr::LOCALHOST), &probes)
            .await
            .expect("scan rate-limited UDP probes");

        assert!(started.elapsed() >= Duration::from_millis(10));
    }

    #[test]
    fn storage_state_names_match_existing_schema() {
        assert_eq!(UDP_TRANSPORT, "udp");
        assert_eq!(port_state_name(PortState::Open), "open");
        assert_eq!(port_state_name(PortState::Closed), "closed");
        assert_eq!(
            port_state_name(PortState::OpenOrFiltered),
            "open_or_filtered"
        );
    }

    #[tokio::test]
    async fn db_persistence_writes_udp_transport_and_all_states() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("apply migrations");
        let network_id: Uuid = sqlx::query_scalar(
            "insert into networks (cidr, name) values ('192.168.40.10/32', $1) returning id",
        )
        .bind(format!("udp-test-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create test network");
        let scan_run_id: Uuid = sqlx::query_scalar(
            "insert into scan_runs \
                (network_id, kind, scope_version, targets_planned, ports_planned) \
             values ($1, 'full_tcp', 1, 1, 3) returning id",
        )
        .bind(network_id)
        .fetch_one(&pool)
        .await
        .expect("create test scan run");
        let address = IpAddr::V4(Ipv4Addr::new(192, 168, 40, 10));
        let observations = vec![
            PortObservation {
                address,
                port: 53,
                state: PortState::Open,
                latency: Duration::from_millis(1),
            },
            PortObservation {
                address,
                port: 123,
                state: PortState::Closed,
                latency: Duration::from_millis(2),
            },
            PortObservation {
                address,
                port: 161,
                state: PortState::OpenOrFiltered,
                latency: Duration::from_millis(3),
            },
        ];

        persist_observations(&pool, scan_run_id, &observations, &HashMap::new())
            .await
            .expect("persist UDP observations");
        let rows: Vec<(i32, String, String)> = sqlx::query_as(
            "select port, transport, state from port_observations \
             where scan_run_id = $1 order by port",
        )
        .bind(scan_run_id)
        .fetch_all(&pool)
        .await
        .expect("read UDP observations");

        assert_eq!(
            rows,
            vec![
                (53, "udp".to_string(), "open".to_string()),
                (123, "udp".to_string(), "closed".to_string()),
                (161, "udp".to_string(), "open_or_filtered".to_string()),
            ]
        );
    }

    #[test]
    fn rejects_port_zero() {
        assert_eq!(
            UdpProbe::new(0, Vec::new()).unwrap_err(),
            ScannerError::InvalidPort { port: 0 }
        );
    }

    fn test_config() -> UdpScannerConfig {
        UdpScannerConfig::with_probe_interval(Duration::from_millis(20), 4, 4, 4, Duration::ZERO)
            .expect("valid scanner config")
    }

    async fn pool_or_skip() -> Option<sqlx::PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        Some(
            sqlx::PgPool::connect(&url)
                .await
                .expect("connect to database"),
        )
    }

    #[derive(Clone, Copy)]
    enum FakeOutcome {
        Open,
        Closed,
        Ambiguous,
        Delay(Duration),
    }

    struct FakeTransport {
        outcome: FakeOutcome,
        active: AtomicUsize,
        maximum: AtomicUsize,
        calls: AtomicUsize,
    }

    impl FakeTransport {
        fn new(outcome: FakeOutcome) -> Self {
            Self {
                outcome,
                active: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl UdpTransport for FakeTransport {
        async fn probe(&self, _target: SocketAddr, payload: &[u8]) -> io::Result<Vec<u8>> {
            assert!(!payload.is_empty());
            self.calls.fetch_add(1, Ordering::Relaxed);
            let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
            self.maximum.fetch_max(active, Ordering::Relaxed);
            match self.outcome {
                FakeOutcome::Delay(delay) => tokio::time::sleep(delay).await,
                FakeOutcome::Open | FakeOutcome::Closed | FakeOutcome::Ambiguous => {}
            }
            self.active.fetch_sub(1, Ordering::Relaxed);
            match self.outcome {
                FakeOutcome::Open => Ok(b"reply".to_vec()),
                FakeOutcome::Closed => Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "ICMP port unreachable",
                )),
                FakeOutcome::Ambiguous | FakeOutcome::Delay(_) => {
                    Err(io::Error::new(io::ErrorKind::TimedOut, "no UDP response"))
                }
            }
        }
    }
}
