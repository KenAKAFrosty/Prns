//! The declarative description of a node: the identity it answers as, whether it forwards,
//! the destinations it stands up before the first packet, and the platform [`Bind`] that owns
//! its interfaces and reactor. `Prns::run` consumes one of these — the consumer never hand-wires
//! the engine, the channels, or the lanes.

use crate::engine::RatchetPolicy;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::ProofStrategy;

use super::Bind;

/// Whether this node forwards for others. `Node` puts the primary identity in the transport
/// role (relay announces, forward addressed packets); `Endpoint` only serves its own destinations.
pub enum Transport {
    Endpoint,
    Node,
}

/// Whether a destination announces itself when the node starts. Recurring cadence stays app
/// policy — the app issues `AnnounceNow` through the binding's command channel on its own timer,
/// exactly as the firmware's announce ticker does today.
pub enum Announce {
    Off,
    AtStartup,
}

/// One destination the node serves from the moment it starts. More can be registered later
/// through the command surface; these are the ones the recipe stands up first.
pub enum StartingDestination<'a> {
    /// An unencrypted, identity-less destination (RNS PLAIN).
    Plain {
        app_name: &'a str,
        aspects: &'a [&'a str],
    },
    /// An encrypted destination answered by the node's primary identity, carrying its proof
    /// strategy, ratchet policy, and announce app-data.
    Single {
        app_name: &'a str,
        aspects: &'a [&'a str],
        app_data: &'a [u8],
        proof: ProofStrategy,
        ratchet: RatchetPolicy,
        announce: Announce,
    },
}

/// Everything [`Prns::run`](super::Prns::run) needs to stand a node up. The storage recipe is
/// carried by the [`Bind`]'s `Storage` associated type, so the caller writes no turbofish; the
/// interface set, lanes, and host live inside the `Bind`.
pub struct Recipe<B: Bind, D> {
    /// The node's primary identity. The engine is built holding it; `Single` destinations answer
    /// as it, and `Transport::Node` gives it the forwarding role.
    pub identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    pub transport: Transport,
    /// The destinations stood up before the first packet is ingested
    /// (`impl IntoIterator<Item = StartingDestination>`).
    pub destinations: D,
    /// The platform binding: interfaces, grant lanes, channels, host, reactor.
    pub bind: B,
}
