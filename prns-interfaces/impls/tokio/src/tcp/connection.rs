#[cfg(feature = "tcp")]
use crate::reconnect::ReconnectPolicy;
#[cfg(feature = "tcp")]
use std::io;
#[cfg(feature = "tcp")]
use std::net::SocketAddr;
use std::time::Duration;

#[cfg(feature = "tcp")]
use futures_util::{stream::FuturesUnordered, StreamExt};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

#[cfg(feature = "tcp")]
use crate::byte_stream::framing::WriteProgressTimeout;

#[cfg(feature = "tcp")]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(feature = "tcp")]
const MAX_CONNECT_ADDRESSES: usize = 8;
/// Match the reference keepalive discipline: probe after 5 seconds idle and every 2 seconds
/// thereafter. The byte-stream driver independently enforces the same 24-second write-progress
/// deadline on every host.
#[cfg(feature = "tcp")]
const TCP_PROBE_AFTER: Duration = Duration::from_secs(5);
#[cfg(feature = "tcp")]
const TCP_PROBE_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(feature = "tcp")]
const TCP_PROBES: u32 = 12;
#[cfg(feature = "tcp")]
const TCP_USER_TIMEOUT: Duration = Duration::from_secs(24);
const I2P_PROBE_AFTER: Duration = Duration::from_secs(10);
const I2P_PROBE_INTERVAL: Duration = Duration::from_secs(9);
const I2P_PROBES: u32 = 5;
const I2P_USER_TIMEOUT: Duration = Duration::from_secs(45);

#[cfg(feature = "tcp")]
pub(crate) const fn write_progress_timeout(tunnel: TcpTunnelMode) -> WriteProgressTimeout {
    let timeout = match tunnel {
        #[cfg(feature = "tcp")]
        TcpTunnelMode::Direct => TCP_USER_TIMEOUT,
        TcpTunnelMode::I2p => I2P_USER_TIMEOUT,
    };
    WriteProgressTimeout::after(timeout)
}

#[cfg(feature = "tcp")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamilyPreference {
    System,
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpTunnelMode {
    #[cfg(feature = "tcp")]
    Direct,
    I2p,
}

#[cfg(feature = "tcp")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectLimit {
    Unlimited,
    Attempts(u32),
}

#[cfg(feature = "tcp")]
impl ReconnectLimit {
    pub const fn exhausted(self, completed_attempts: u32) -> bool {
        match self {
            Self::Unlimited => false,
            Self::Attempts(limit) => completed_attempts >= limit,
        }
    }
}

#[cfg(feature = "tcp")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpConnectionSettings {
    pub connect_timeout: Duration,
    pub reconnect_policy: ReconnectPolicy,
    pub reconnect_limit: ReconnectLimit,
    pub address_family: AddressFamilyPreference,
    pub tunnel: TcpTunnelMode,
}

#[cfg(feature = "tcp")]
impl TcpConnectionSettings {
    pub const STOCK: Self = Self {
        connect_timeout: CONNECT_TIMEOUT,
        reconnect_policy: ReconnectPolicy::STANDARD,
        reconnect_limit: ReconnectLimit::Unlimited,
        address_family: AddressFamilyPreference::System,
        tunnel: TcpTunnelMode::Direct,
    };
}

#[cfg(feature = "backbone")]
pub(crate) async fn connect(
    target: &str,
    settings: TcpConnectionSettings,
) -> io::Result<TcpStream> {
    connect_classified(target, settings)
        .await
        .map_err(|error| error.source)
}

#[cfg(feature = "tcp")]
#[derive(Debug)]
pub(crate) struct TcpConnectFailure {
    pub reason: &'static str,
    source: io::Error,
}

#[cfg(feature = "tcp")]
impl std::fmt::Display for TcpConnectFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.source)
    }
}

#[cfg(feature = "tcp")]
impl std::error::Error for TcpConnectFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(feature = "tcp")]
fn connect_failure(error: io::Error) -> TcpConnectFailure {
    let reason = match error.kind() {
        io::ErrorKind::PermissionDenied => "Network access was denied",
        io::ErrorKind::ConnectionRefused => "The TCP peer refused the connection",
        io::ErrorKind::TimedOut => "The TCP connection timed out",
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => {
            "The TCP peer network is unreachable"
        }
        _ => "The TCP connection could not be established",
    };
    TcpConnectFailure {
        reason,
        source: error,
    }
}

#[cfg(feature = "tcp")]
pub(crate) async fn connect_classified(
    target: &str,
    settings: TcpConnectionSettings,
) -> Result<TcpStream, TcpConnectFailure> {
    let addresses = tokio::time::timeout(
        settings.connect_timeout,
        resolve(target, settings.address_family),
    )
    .await
    .map_err(|_| TcpConnectFailure {
        reason: "DNS resolution timed out",
        source: io::Error::new(io::ErrorKind::TimedOut, "DNS resolution timed out"),
    })?
    .map_err(|source| TcpConnectFailure {
        reason: if source.kind() == io::ErrorKind::PermissionDenied {
            "Network access was denied"
        } else {
            "The TCP peer hostname could not be resolved"
        },
        source,
    })?;
    connect_addresses(&addresses, settings.connect_timeout).await
}

/// Race a bounded DNS answer set without letting one unreachable family consume
/// every reconnect attempt. All candidates share one total connection deadline;
/// dropping the remaining futures closes their sockets when a candidate wins.
#[cfg(feature = "tcp")]
async fn connect_addresses(
    addresses: &[SocketAddr],
    timeout: Duration,
) -> Result<TcpStream, TcpConnectFailure> {
    let count = addresses.len().min(MAX_CONNECT_ADDRESSES);
    let stagger = (timeout / count.max(1) as u32).min(Duration::from_millis(250));
    let mut attempts = addresses
        .iter()
        .copied()
        .take(count)
        .enumerate()
        .map(|(index, address)| async move {
            if index > 0 {
                tokio::time::sleep(stagger * index as u32).await;
            }
            TcpStream::connect(address).await
        })
        .collect::<FuturesUnordered<_>>();
    tokio::time::timeout(timeout, async move {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "TCP target resolved to nothing");
        while let Some(result) = attempts.next().await {
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => last = error,
            }
        }
        Err(last)
    })
    .await
    .map_err(|_| {
        connect_failure(io::Error::new(
            io::ErrorKind::TimedOut,
            "TCP connect timed out",
        ))
    })?
    .map_err(connect_failure)
}

#[cfg(feature = "tcp")]
async fn resolve(target: &str, preference: AddressFamilyPreference) -> io::Result<Vec<SocketAddr>> {
    let addresses = tokio::net::lookup_host(target)
        .await?
        .take(64)
        .collect::<std::vec::Vec<_>>();
    let addresses = ordered_addresses(&addresses, preference);
    if addresses.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "TCP target resolved to nothing",
        ))
    } else {
        Ok(addresses)
    }
}

#[cfg(feature = "tcp")]
fn ordered_addresses(
    addresses: &[SocketAddr],
    preference: AddressFamilyPreference,
) -> Vec<SocketAddr> {
    let mut ordered = Vec::with_capacity(MAX_CONNECT_ADDRESSES);
    let prefer_v4 = match preference {
        AddressFamilyPreference::System => addresses.first().is_none_or(SocketAddr::is_ipv4),
        AddressFamilyPreference::Ipv4 => true,
        AddressFamilyPreference::Ipv6 => false,
    };
    let mut preferred = addresses
        .iter()
        .filter(|address| address.is_ipv4() == prefer_v4);
    let mut fallback = addresses
        .iter()
        .filter(|address| address.is_ipv4() != prefer_v4);
    loop {
        let pair = [preferred.next(), fallback.next()];
        if pair.iter().all(Option::is_none) {
            return ordered;
        }
        for address in pair.into_iter().flatten() {
            if !ordered.contains(address) {
                ordered.push(*address);
                if ordered.len() == MAX_CONNECT_ADDRESSES {
                    return ordered;
                }
            }
        }
    }
}

/// Socket tuning is best-effort because a socket that refuses an option can still carry frames.
#[cfg(feature = "tcp")]
pub fn tune(stream: &TcpStream) {
    tune_for_tunnel(stream, TcpTunnelMode::Direct);
}

#[cfg(feature = "i2p")]
pub(crate) fn tune_i2p(stream: &TcpStream) {
    tune_for_tunnel(stream, TcpTunnelMode::I2p);
}

pub(crate) fn tune_for_tunnel(stream: &TcpStream, tunnel: TcpTunnelMode) {
    let _ = stream.set_nodelay(true);
    let socket = SockRef::from(stream);
    let (probe_after, probe_interval, probes, _user_timeout) = match tunnel {
        #[cfg(feature = "tcp")]
        TcpTunnelMode::Direct => (
            TCP_PROBE_AFTER,
            TCP_PROBE_INTERVAL,
            TCP_PROBES,
            TCP_USER_TIMEOUT,
        ),
        TcpTunnelMode::I2p => (
            I2P_PROBE_AFTER,
            I2P_PROBE_INTERVAL,
            I2P_PROBES,
            I2P_USER_TIMEOUT,
        ),
    };
    let keepalive = TcpKeepalive::new()
        .with_time(probe_after)
        .with_interval(probe_interval)
        .with_retries(probes);
    let _ = socket.set_tcp_keepalive(&keepalive);
    #[cfg(target_os = "linux")]
    let _ = socket.set_tcp_user_timeout(Some(_user_timeout));
}

#[cfg(all(test, feature = "tcp"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_host_reports_dns_separately_from_connection_errors() {
        let failure = connect_classified(
            "not-a-valid-ip-or-host:invalid-port",
            TcpConnectionSettings::STOCK,
        )
        .await
        .expect_err("invalid endpoint");
        assert_eq!(
            failure.reason,
            "The TCP peer hostname could not be resolved"
        );
        assert_eq!(
            connect_failure(io::Error::from(io::ErrorKind::PermissionDenied)).reason,
            "Network access was denied"
        );
        assert_eq!(
            connect_failure(io::Error::from(io::ErrorKind::TimedOut)).reason,
            "The TCP connection timed out"
        );
    }

    #[test]
    fn address_selection_honors_the_requested_family_with_a_system_fallback() {
        let v6 = "[::1]:4242".parse().expect("valid IPv6 address");
        let v4 = "127.0.0.1:4242".parse().expect("valid IPv4 address");
        let addresses = [v6, v4];

        assert_eq!(
            ordered_addresses(&addresses, AddressFamilyPreference::System),
            vec![v6, v4]
        );
        assert_eq!(
            ordered_addresses(&addresses, AddressFamilyPreference::Ipv4),
            vec![v4, v6]
        );
        assert_eq!(
            ordered_addresses(&[v4, v4], AddressFamilyPreference::Ipv6),
            vec![v4]
        );
    }

    #[tokio::test]
    async fn an_unavailable_first_dns_answer_does_not_hide_a_reachable_peer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let reachable = listener.local_addr().expect("address");
        let first = SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), reachable.port());
        let stream = connect_addresses(&[first, reachable], Duration::from_secs(2))
            .await
            .expect("fallback connects");
        assert_eq!(stream.peer_addr().expect("connected endpoint"), reachable);
    }

    #[test]
    fn dns_candidates_are_distinct_and_bounded() {
        let answers = (1..32)
            .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
            .collect::<Vec<_>>();
        assert_eq!(
            ordered_addresses(&answers, AddressFamilyPreference::System),
            answers[..MAX_CONNECT_ADDRESSES]
        );
        let mut mixed = answers.clone();
        let v6 = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 4242));
        mixed.push(v6);
        let ordered = ordered_addresses(&mixed, AddressFamilyPreference::System);
        assert_eq!(ordered[0], answers[0]);
        assert_eq!(
            ordered[1], v6,
            "the other family is tried within the bounded set"
        );
    }

    #[test]
    fn reconnect_limits_count_only_reconnect_attempts() {
        assert!(!ReconnectLimit::Unlimited.exhausted(u32::MAX));
        assert!(ReconnectLimit::Attempts(0).exhausted(0));
        assert!(!ReconnectLimit::Attempts(2).exhausted(1));
        assert!(ReconnectLimit::Attempts(2).exhausted(2));
    }

    #[test]
    fn write_progress_deadlines_follow_the_tunnel_liveness_policy() {
        assert_eq!(
            write_progress_timeout(TcpTunnelMode::Direct),
            WriteProgressTimeout::after(Duration::from_secs(24))
        );
        assert_eq!(
            write_progress_timeout(TcpTunnelMode::I2p),
            WriteProgressTimeout::after(Duration::from_secs(45))
        );
    }
}
