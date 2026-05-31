use super::Interface;

/// Semantic marker for **shared-multicast** transports: a membership group
/// on a switched network (an IP LAN), discovered by multicast, where
/// reaching every member is realized by *fanout* — a multicast send, or one
/// unicast per discovered member — rather than a single transmission a shared
/// physical medium propagates on its own. RNS's AutoInterface is the
/// archetype: nodes find each other with link-local multicast beacons, then
/// exchange data as unicast to each discovered peer. Pairs with
/// [`MediumKind::Multicast`](crate::interfaces::MediumKind::Multicast).
///
/// This is deliberately distinct from
/// [`SharedBroadcastInterface`](crate::interfaces::SharedBroadcastInterface).
/// On a shared-broadcast medium the medium *is* the propagation mechanism, so
/// [`Capabilities::repeats`](crate::interfaces::Capabilities::repeats)`=true`
/// (re-emitting on the source interface is how a packet reaches the next hop).
/// A shared-multicast interface sits on a switched fabric that does **not**
/// propagate for you: re-emitting to the group would only echo to members who
/// already heard it, so forwarding is per-member fanout the engine drives via
/// its `fire_on` lists, and `repeats=false`. The medium looks like one
/// interface but speaks to many peers — neither point-to-point nor a true
/// shared broadcast domain.
///
/// The universal byte I/O surface (`try_read`, `write`, `read_inbound`) lives
/// on the base [`Interface`] trait. `write` here means "emit to every current
/// group member"; the implementation owns how that fanout is realized. This
/// sub-trait declares shared-multicast semantics and is where group metadata
/// (membership churn, per-peer reception context) should land once needed.
///
/// **Self-echo:** a member that sends to the multicast group receives its own
/// transmission back through `try_read` (IPv6 multicast loopback). The engine
/// stays correct regardless — discovery beacons are recognized as self by
/// their source address, and data packets are caught by announce-id replay
/// defense as a duplicate of an id already accepted — at the cost of an ingest
/// cycle where the implementation does not suppress the echo itself.
///
/// Hosts declare the intent with a single empty impl:
///
/// ```ignore
/// impl Interface for MyAutoInterface { /* … */ }
/// impl SharedMulticastInterface for MyAutoInterface {}
/// ```
pub trait SharedMulticastInterface: Interface {}
