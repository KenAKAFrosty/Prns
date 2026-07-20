//! The reference's TCP socket discipline and connection timings under tokio, shared by the
//! [`client`](super::client) and [`server`](super::server) bodies: Nagle off so a Reticulum frame
//! hits the wire when it is written, the keepalive probe schedule that declares a silent peer dead,
//! and the connect wait (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT`).

#[cfg(feature = "tcp")]
use crate::reconnect::ReconnectPolicy;
#[cfg(feature = "tcp")]
use std::io;
#[cfg(feature = "tcp")]
use std::net::SocketAddr;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

/// How long one connect attempt gets (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT`).
#[cfg(feature = "tcp")]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The reference's keepalive discipline: probe after 5s idle, every 2s, and a peer that
/// misses 12 probes — or leaves writes unacknowledged for 24s — is declared dead, so the
/// stream drops and the reconnect loop takes over.
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

#[cfg(feature = "tcp")]
pub(crate) async fn connect(
    target: &str,
    settings: TcpConnectionSettings,
) -> io::Result<TcpStream> {
    let address = resolve(target, settings.address_family).await?;
    tokio::time::timeout(settings.connect_timeout, TcpStream::connect(address))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TCP connect timed out"))?
}

#[cfg(feature = "tcp")]
async fn resolve(target: &str, preference: AddressFamilyPreference) -> io::Result<SocketAddr> {
    let addresses = tokio::net::lookup_host(target)
        .await?
        .collect::<std::vec::Vec<_>>();
    select_address(&addresses, preference)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "TCP target resolved to nothing"))
}

#[cfg(feature = "tcp")]
fn select_address(
    addresses: &[SocketAddr],
    preference: AddressFamilyPreference,
) -> Option<SocketAddr> {
    let preferred = match preference {
        AddressFamilyPreference::System => return addresses.first().copied(),
        AddressFamilyPreference::Ipv4 => addresses.iter().find(|address| address.is_ipv4()),
        AddressFamilyPreference::Ipv6 => addresses.iter().find(|address| address.is_ipv6()),
    };
    preferred.copied().or_else(|| addresses.first().copied())
}

/// The reference's socket discipline, best-effort: Nagle off (a Reticulum frame should hit
/// the wire when it's written, not pool behind the ACK clock) and the keepalive probes
/// above. A socket that refuses an option still carries frames, so refusals don't fail the
/// connection.
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

    #[test]
    fn address_selection_honors_the_requested_family_with_a_system_fallback() {
        let v6 = "[::1]:4242".parse().expect("valid IPv6 address");
        let v4 = "127.0.0.1:4242".parse().expect("valid IPv4 address");
        let addresses = [v6, v4];

        assert_eq!(
            select_address(&addresses, AddressFamilyPreference::System),
            Some(v6)
        );
        assert_eq!(
            select_address(&addresses, AddressFamilyPreference::Ipv4),
            Some(v4)
        );
        assert_eq!(
            select_address(&[v4], AddressFamilyPreference::Ipv6),
            Some(v4)
        );
    }

    #[test]
    fn reconnect_limits_count_only_reconnect_attempts() {
        assert!(!ReconnectLimit::Unlimited.exhausted(u32::MAX));
        assert!(ReconnectLimit::Attempts(0).exhausted(0));
        assert!(!ReconnectLimit::Attempts(2).exhausted(1));
        assert!(ReconnectLimit::Attempts(2).exhausted(2));
    }
}
