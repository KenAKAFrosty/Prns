#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::tcp::{
    tune, AddressFamilyPreference, ReconnectLimit, TcpClientInterface, TcpConnectionSettings,
    TcpServer, TcpServerConnection, TcpServerStatus, TcpTunnelMode, CONNECT_TIMEOUT,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::tcp::{TcpClient, CONNECT_TIMEOUT, KEEP_ALIVE, SOCKET_TIMEOUT};
