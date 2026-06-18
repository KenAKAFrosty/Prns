//! The RNS `TCPServerInterface`: the listening end of a TCP pair — bind a port and stand up a
//! distinct engine interface per client that connects, the way the reference spawns a child
//! interface per accepted connection. The tokio body is an [`InterfaceSupervisor`](crate::runtime::InterfaceSupervisor)
//! (`TcpServer`) over per-connection members (`TcpServerConnection`), mirroring the `LocalServer` /
//! `LocalClientInterface` pair. Shares the parent's framing [`core`](super::core) and (on tokio) the
//! [`tokio_socket`](super::tokio_socket) discipline.

#[cfg(feature = "tcp")]
pub mod tokio;
