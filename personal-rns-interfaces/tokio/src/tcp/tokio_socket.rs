//! The reference's TCP socket discipline and connection timings under tokio, shared by the
//! [`client`](super::client) and [`server`](super::server) bodies: Nagle off so a Reticulum frame
//! hits the wire when it is written, the keepalive probe schedule that declares a silent peer dead,
//! and the connect/reconnect waits (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT` /
//! `RECONNECT_WAIT`). Both ends apply the same discipline, so it lives here once.

use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

/// How long one connect attempt gets (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a failed or dropped connection waits before the next attempt
/// (`TCPClientInterface.RECONNECT_WAIT`).
pub const RECONNECT_WAIT: Duration = Duration::from_secs(5);
/// The reference's keepalive discipline: probe after 5s idle, every 2s, and a peer that
/// misses 12 probes — or leaves writes unacknowledged for 24s — is declared dead, so the
/// stream drops and the reconnect loop takes over.
pub const TCP_PROBE_AFTER: Duration = Duration::from_secs(5);
pub const TCP_PROBE_INTERVAL: Duration = Duration::from_secs(2);
pub const TCP_PROBES: u32 = 12;
pub const TCP_USER_TIMEOUT: Duration = Duration::from_secs(24);

/// The reference's socket discipline, best-effort: Nagle off (a Reticulum frame should hit
/// the wire when it's written, not pool behind the ACK clock) and the keepalive probes
/// above. A socket that refuses an option still carries frames, so refusals don't fail the
/// connection.
pub fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let socket = SockRef::from(stream);
    let keepalive = TcpKeepalive::new()
        .with_time(TCP_PROBE_AFTER)
        .with_interval(TCP_PROBE_INTERVAL)
        .with_retries(TCP_PROBES);
    let _ = socket.set_tcp_keepalive(&keepalive);
    #[cfg(target_os = "linux")]
    let _ = socket.set_tcp_user_timeout(Some(TCP_USER_TIMEOUT));
}
