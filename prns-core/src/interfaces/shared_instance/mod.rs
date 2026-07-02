//! The local shared-instance interface: RNS `LocalServerInterface` / `LocalClientInterface` parity.
//! A node that owns the local bus (the Hopspot daemon) binds a loopback TCP port or an AF_UNIX
//! socket; every other RNS app on the host (Sideband, NomadNet, MeshChat) that fails to bind it
//! instead connects, and the daemon stands up one engine interface per connection. The wire is the
//! same RNS HDLC framing TCP and serial speak, at local-bus speed; the only thing special about it
//! is the free-transit hop discount (the [`LocalClient`](crate::interfaces::InterfaceKind::LocalClient)
//! kind, applied at ingress), which keeps the daemon and its apps a single node.
//!
//! The sizing brain is the host-agnostic [`core`]; the tokio body lives in `prns-interfaces-tokio`'s
//! `shared_instance` module. Core keeps the control-RPC seam the node handle implements ([`rpc`])
//! and the msgpack value vocabulary the RPC wire speaks ([`rpc_value`]).

pub mod core;
#[cfg(feature = "std")]
pub mod rpc;
#[cfg(feature = "std")]
pub mod rpc_value;
