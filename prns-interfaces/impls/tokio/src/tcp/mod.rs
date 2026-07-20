#[cfg(feature = "tcp")]
mod client;
mod connection;
#[cfg(feature = "tcp")]
mod server;

#[cfg(feature = "tcp")]
pub use client::TcpClientInterface;
pub use connection::TcpTunnelMode;
#[cfg(feature = "tcp")]
pub use connection::{
    tune, AddressFamilyPreference, ReconnectLimit, TcpConnectionSettings, CONNECT_TIMEOUT,
};
#[cfg(feature = "tcp")]
pub use server::{TcpServer, TcpServerConnection, TcpServerStatus};

#[cfg(feature = "i2p")]
pub(crate) use connection::tune_i2p;
#[cfg(feature = "tcp")]
pub(crate) use connection::{connect, tune_for_tunnel};
