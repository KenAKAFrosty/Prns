use crate::interfaces::Interface;

/// Semantic marker for **point-to-point** transports: one interface
/// instance speaks to one identified peer. TCP, USB CDC, BLE GATT,
/// USB serial, paired loopback. Pairs with
/// [`MediumKind::DirectPeer`](crate::interfaces::MediumKind::DirectPeer)
/// and
/// [`MediumKind::SwitchedNetwork`](crate::interfaces::MediumKind::SwitchedNetwork).
///
/// The universal byte I/O surface (`try_read`, `write`, `read_inbound`)
/// lives on the base [`Interface`] trait — every interface, regardless
/// of medium, has to accommodate those operations. This sub-trait is
/// an opt-in marker that declares semantic intent and a future-growth
/// hook for methods that only make sense for point-to-point transports
/// (e.g., a `peer_identity()` accessor when link establishment lands).
///
/// Hosts declare the intent with a single empty impl:
///
/// ```ignore
/// impl Interface for MyTcpInterface { /* … */ }
/// impl PointToPointInterface for MyTcpInterface {}
/// ```
pub trait PointToPointInterface: Interface {}
