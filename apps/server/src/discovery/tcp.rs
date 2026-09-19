//! Network discovery primitives.
//!
//! This module only performs TCP connect probes. It never writes to a socket,
//! authenticates, fingerprints, or decides whether an address is approved;
//! scan jobs own those policies. The bulk API applies global, network, and
//! per-host semaphores in that order, matching ADR-0010 and the M2 design.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};

/// Future returned by the TCP connector seam used by [`ConnectScanner`].
type ConnectFuture = Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>>;

/// Injectable TCP connection primitive. Production uses [`TcpStream::connect`].
trait TcpConnector: Send + Sync {
    fn connect(&self, target: SocketAddr) -> ConnectFuture;
}

struct TokioTcpConnector;

impl TcpConnector for TokioTcpConnector {
    fn connect(&self, target: SocketAddr) -> ConnectFuture {
        Box::pin(TcpStream::connect(target))
    }
}

/// TCP result state supported by a connect scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    /// TCP handshake completed. The stream is closed immediately.
    Open,
    /// Target rejected the handshake with `ECONNREFUSED`.
    Closed,
    /// Connect timed out or failed without enough information to call it closed.
    Filtered,
}

/// One TCP connect observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortObservation {
    pub address: IpAddr,
    pub port: u16,
    pub state: PortState,
    pub latency: Duration,
}

/// Errors caused by invalid scanner setup or invalid scan input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScannerError {
    #[error("connect timeout must be greater than zero")]
    InvalidTimeout,
    #[error("{name} concurrency must be greater than zero")]
    InvalidConcurrency { name: &'static str },
    #[error("TCP port 0 is not a valid scan target")]
    InvalidPort { port: u16 },
    #[error("network key must not be empty")]
    InvalidNetworkKey,
    #[error("scanner concurrency limiter closed")]
    LimiterClosed,
    #[error("scanner returned no observation")]
    NoObservation,
}

/// Limits for one [`ConnectScanner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectScannerConfig {
    connect_timeout: Duration,
    global_concurrency: usize,
    network_concurrency: usize,
    per_host_concurrency: usize,
}

impl ConnectScannerConfig {
    pub fn new(
        connect_timeout: Duration,
        global_concurrency: usize,
        network_concurrency: usize,
        per_host_concurrency: usize,
    ) -> Result<Self, ScannerError> {
        if connect_timeout.is_zero() {
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
            connect_timeout,
            global_concurrency,
            network_concurrency,
            per_host_concurrency,
        })
    }

    pub fn connect_timeout(self) -> Duration {
        self.connect_timeout
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
}

impl Default for ConnectScannerConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(1),
            global_concurrency: 64,
            network_concurrency: 64,
            per_host_concurrency: 2,
        }
    }
}

/// Scanner seam reserved for alternate implementations such as SYN scanning.
#[async_trait]
pub trait Scanner: Send + Sync {
    /// Probe one TCP endpoint. Implementations must not write application data.
    async fn scan(&self, target: SocketAddr) -> Result<PortObservation, ScannerError>;

    /// Probe one TCP endpoint under an approved network's concurrency limits.
    ///
    /// Alternate scanner implementations can use the simpler [`Scanner::scan`]
    /// seam when they do not need network- or host-specific throttling.
    async fn scan_scoped(
        &self,
        _network_key: &str,
        target: SocketAddr,
    ) -> Result<PortObservation, ScannerError> {
        self.scan(target).await
    }
}

/// Native asynchronous TCP connect scanner.
pub struct ConnectScanner {
    config: ConnectScannerConfig,
    global_limiter: Arc<Semaphore>,
    network_limiters: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
    host_limiters: Arc<Mutex<HashMap<IpAddr, Weak<Semaphore>>>>,
    connector: Arc<dyn TcpConnector>,
}

impl ConnectScanner {
    pub fn new(config: ConnectScannerConfig) -> Result<Self, ScannerError> {
        Self::with_connector(config, Arc::new(TokioTcpConnector))
    }

    fn with_connector(
        config: ConnectScannerConfig,
        connector: Arc<dyn TcpConnector>,
    ) -> Result<Self, ScannerError> {
        // Config fields are private, but revalidate here so this constructor
        // remains safe if config construction changes later.
        let config = ConnectScannerConfig::new(
            config.connect_timeout,
            config.global_concurrency,
            config.network_concurrency,
            config.per_host_concurrency,
        )?;
        Ok(Self {
            global_limiter: Arc::new(Semaphore::new(config.global_concurrency)),
            network_limiters: Arc::new(Mutex::new(HashMap::new())),
            host_limiters: Arc::new(Mutex::new(HashMap::new())),
            config,
            connector,
        })
    }

    /// Probe one TCP endpoint under the scanner's global limit.
    pub async fn scan(&self, target: SocketAddr) -> Result<PortObservation, ScannerError> {
        if target.port() == 0 {
            return Err(ScannerError::InvalidPort { port: 0 });
        }
        let _global = self
            .global_limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ScannerError::LimiterClosed)?;
        self.probe(target).await
    }

    /// Probe one endpoint under global, network, and per-host bounds.
    pub async fn scan_scoped(
        &self,
        network_key: &str,
        target: SocketAddr,
    ) -> Result<PortObservation, ScannerError> {
        self.scan_ports(network_key.to_owned(), target.ip(), &[target.port()])
            .await?
            .into_iter()
            .next()
            .ok_or(ScannerError::NoObservation)
    }

    /// Scan ports for one host with global, network, and per-host bounds.
    ///
    /// Results preserve input order. Duplicate ports are probed once. The
    /// caller must pass a key identifying approved network scope; scope
    /// validation remains outside this transport primitive.
    pub async fn scan_ports(
        &self,
        network_key: impl Into<String>,
        address: IpAddr,
        ports: &[u16],
    ) -> Result<Vec<PortObservation>, ScannerError> {
        let network_key = network_key.into();
        if network_key.trim().is_empty() {
            return Err(ScannerError::InvalidNetworkKey);
        }
        let ports = unique_ports(ports)?;
        if ports.is_empty() {
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

        stream::iter(ports.into_iter().map(|port| {
            let scanner = self.clone();
            let network_limiter = Arc::clone(&network_limiter);
            let host_limiter = Arc::clone(&host_limiter);
            async move {
                // Acquisition order is intentional: global, network, host.
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
                scanner.probe(SocketAddr::new(address, port)).await
            }
        }))
        .buffered(max_in_flight)
        .try_collect()
        .await
    }

    async fn probe(&self, target: SocketAddr) -> Result<PortObservation, ScannerError> {
        let started = tokio::time::Instant::now();
        let state =
            match tokio::time::timeout(self.config.connect_timeout, self.connector.connect(target))
                .await
            {
                Ok(Ok(stream)) => {
                    drop(stream);
                    PortState::Open
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    PortState::Closed
                }
                Ok(Err(_)) | Err(_) => PortState::Filtered,
            };
        Ok(PortObservation {
            address: target.ip(),
            port: target.port(),
            state,
            latency: started.elapsed(),
        })
    }
}

impl Clone for ConnectScanner {
    fn clone(&self) -> Self {
        Self {
            config: self.config,
            global_limiter: Arc::clone(&self.global_limiter),
            network_limiters: Arc::clone(&self.network_limiters),
            host_limiters: Arc::clone(&self.host_limiters),
            connector: Arc::clone(&self.connector),
        }
    }
}

#[async_trait]
impl Scanner for ConnectScanner {
    async fn scan(&self, target: SocketAddr) -> Result<PortObservation, ScannerError> {
        ConnectScanner::scan(self, target).await
    }

    async fn scan_scoped(
        &self,
        network_key: &str,
        target: SocketAddr,
    ) -> Result<PortObservation, ScannerError> {
        ConnectScanner::scan_scoped(self, network_key, target).await
    }
}

fn unique_ports(ports: &[u16]) -> Result<Vec<u16>, ScannerError> {
    let mut seen = std::collections::HashSet::with_capacity(ports.len());
    let mut unique = Vec::with_capacity(ports.len());
    for &port in ports {
        if port == 0 {
            return Err(ScannerError::InvalidPort { port });
        }
        if seen.insert(port) {
            unique.push(port);
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
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn connect_scanner_reports_local_open_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local test listener");
        let target = listener.local_addr().expect("read local test address");
        let scanner = ConnectScanner::new(test_config()).expect("valid scanner config");

        let observation = scanner.scan(target).await.expect("scan local open port");

        assert_eq!(observation.address, target.ip());
        assert_eq!(observation.port, target.port());
        assert_eq!(observation.state, PortState::Open);
    }

    #[tokio::test]
    async fn connect_scanner_reports_local_closed_port() {
        let target = unused_local_address().await;
        let scanner = ConnectScanner::new(test_config()).expect("valid scanner config");

        let observation = scanner.scan(target).await.expect("scan local closed port");

        assert_eq!(observation.state, PortState::Closed);
    }

    #[tokio::test]
    async fn connect_scanner_reports_timeout_as_filtered() {
        let connector = Arc::new(SlowConnector {
            delay: Duration::from_millis(50),
        });
        let scanner =
            ConnectScanner::with_connector(test_config(), connector).expect("valid scanner config");
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, 12345));

        let started = tokio::time::Instant::now();
        let observation = scanner.scan(target).await.expect("scan timed-out port");

        assert_eq!(observation.state, PortState::Filtered);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn connect_scanner_bounds_concurrent_connections() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(CountingConnector {
            active: Arc::clone(&active),
            maximum: Arc::clone(&maximum),
        });
        let config = ConnectScannerConfig::new(Duration::from_secs(1), 2, 2, 2)
            .expect("valid scanner config");
        let scanner =
            ConnectScanner::with_connector(config, connector).expect("valid scanner config");

        let ports: Vec<u16> = (1..=8).collect();
        let observations = scanner
            .scan_ports("test-network", IpAddr::V4(Ipv4Addr::LOCALHOST), &ports)
            .await
            .expect("scan test ports");

        assert_eq!(observations.len(), ports.len());
        assert!(maximum.load(Ordering::Relaxed) <= 2);
    }

    fn test_config() -> ConnectScannerConfig {
        ConnectScannerConfig::new(Duration::from_millis(20), 4, 4, 4).expect("test config is valid")
    }

    async fn unused_local_address() -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local test listener");
        listener.local_addr().expect("read local test address")
    }

    struct SlowConnector {
        delay: Duration,
    }

    impl TcpConnector for SlowConnector {
        fn connect(&self, _target: SocketAddr) -> ConnectFuture {
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Err(io::Error::new(io::ErrorKind::TimedOut, "test timeout"))
            })
        }
    }

    struct CountingConnector {
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    impl TcpConnector for CountingConnector {
        fn connect(&self, _target: SocketAddr) -> ConnectFuture {
            let active = Arc::clone(&self.active);
            let maximum = Arc::clone(&self.maximum);
            Box::pin(async move {
                let current = active.fetch_add(1, Ordering::Relaxed) + 1;
                maximum.fetch_max(current, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(10)).await;
                active.fetch_sub(1, Ordering::Relaxed);
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "test closed",
                ))
            })
        }
    }
}
