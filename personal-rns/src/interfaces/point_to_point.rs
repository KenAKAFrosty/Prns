use crate::interfaces::Interface;

/// Semantic marker for **point-to-point** transports: one interface
/// instance speaks to one identified peer. TCP, USB CDC, BLE GATT,
/// USB serial, paired loopback. Pairs with
/// [`MediumKind::DirectPeer`](crate::interfaces::MediumKind::DirectPeer)
/// and
/// [`MediumKind::SwitchedNetwork`](crate::interfaces::MediumKind::SwitchedNetwork).
///
/// The universal byte I/O surface (`try_read`, `write`, `read_inbound`)
/// lives on the base [`Interface`] trait. This sub-trait declares
/// point-to-point semantics and is where methods specific to direct-peer
/// transports should land once needed.
///
/// Hosts declare the intent with a single empty impl:
///
/// ```ignore
/// impl Interface for MyTcpInterface { /* … */ }
/// impl PointToPointInterface for MyTcpInterface {}
/// ```
pub trait PointToPointInterface: Interface {}
