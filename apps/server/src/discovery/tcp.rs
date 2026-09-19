//! TCP connect scanner (ADR-0010).
//!
//! The scanner performs one TCP handshake per probe and sends no protocol
//! bytes. Its semaphore is shared by clones, so callers can create one per
//! worker and retain a hard connection-attempt ceiling across scan runs.

// The scan-run coordinator lands immediately after this scanner slice. Keep
// this narrow allowance here, rather than widening lint policy for the server
// crate, while no production caller exists yet.
#![allow(dead_code)]

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Open,
    Closed,
    Filtered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub address: SocketAddr,
    pub state: PortState,
    pub latency: Duration,
    pub error: Option<String>,
}

/// The M2 seam between scan coordination and a concrete probe strategy.
/// ADR-0010 supplies [`ConnectScanner`] today; a privileged SYN adapter can
/// implement this interface later without changing workers or persistence.
#[async_trait]
pub trait Scanner: Send + Sync {
    async fn probe(&self, address: SocketAddr) -> ProbeResult;
}

#[derive(Clone)]
pub struct ConnectScanner {
    timeout: Duration,
    permits: Arc<Semaphore>,
}

impl ConnectScanner {
    pub fn new(timeout: Duration, max_concurrency: usize) -> Self {
        Self {
            timeout,
            permits: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }
}

#[async_trait]
impl Scanner for ConnectScanner {
    async fn probe(&self, address: SocketAddr) -> ProbeResult {
        let started = Instant::now();
        let permit = self.permits.acquire().await;
        if permit.is_err() {
            return ProbeResult {
                address,
                state: PortState::Filtered,
                latency: started.elapsed(),
                error: Some("scanner shut down".to_string()),
            };
        }
        let result = tokio::time::timeout(self.timeout, TcpStream::connect(address)).await;
        drop(permit);

        match result {
            Ok(Ok(stream)) => {
                drop(stream);
                ProbeResult {
                    address,
                    state: PortState::Open,
                    latency: started.elapsed(),
                    error: None,
                }
            }
            Ok(Err(error)) if error.kind() == ErrorKind::ConnectionRefused => ProbeResult {
                address,
                state: PortState::Closed,
                latency: started.elapsed(),
                error: None,
            },
            Ok(Err(error)) => ProbeResult {
                address,
                state: PortState::Filtered,
                latency: started.elapsed(),
                error: Some(error.to_string()),
            },
            Err(_) => ProbeResult {
                address,
                state: PortState::Filtered,
                latency: started.elapsed(),
                error: Some("connection timed out".to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;

    use super::{ConnectScanner, PortState, Scanner};

    #[tokio::test]
    async fn reports_a_listening_port_as_open() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let scanner = ConnectScanner::new(Duration::from_secs(1), 4);

        let result = scanner.probe(listener.local_addr().unwrap()).await;

        assert_eq!(result.state, PortState::Open);
        drop(listener);
    }

    #[tokio::test]
    async fn reports_a_refused_local_port_as_closed() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let scanner = ConnectScanner::new(Duration::from_secs(1), 4);

        let result = scanner
            .probe(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .await;

        assert_eq!(result.state, PortState::Closed);
    }
}
