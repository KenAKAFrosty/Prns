//! The declarative description of a node: which destinations it serves, whether it forwards,
//! and the platform [`Bind`] that owns its interfaces and reactor. `Prns::run` consumes one of
//! these — the consumer never hand-wires the engine, the channels, or the lanes.
//!
//! There is no "self" identity. A node is whatever destinations it stands up; each `Single`
//! destination carries the key it answers as. The app (a daemon, Hopspot, a benchmark — the
//! thing that calls `Prns::run` *is* the app) registers as many, or as few, as it needs.
//!
//! Announces are entirely app policy. The runtime hands the controls (a command channel), and
//! the app — which knows it has started the moment `Prns::run` is reached — fires `AnnounceNow`
//! once, on a schedule, or never. The recipe says nothing about announce cadence.

use crate::engine::RatchetPolicy;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name,
};
use crate::routing::ProofStrategy;
use crate::wire::{DestinationHash, TransportId};

use super::Bind;

/// One destination the node serves from the moment it starts. More can be registered later
/// through the command surface; these are the ones the recipe stands up first.
pub enum StartingDestination<'a> {
    /// An unencrypted, identity-less destination (RNS PLAIN).
    Plain {
        app_name: &'a str,
        aspects: &'a [&'a str],
    },
    /// An encrypted destination and the key it answers as — its own identity, held for it
    /// alone. Carries its proof strategy, ratchet policy, and announce app-data.
    Single {
        app_name: &'a str,
        aspects: &'a [&'a str],
        identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        app_data: &'a [u8],
        proof: ProofStrategy,
        ratchet: RatchetPolicy,
    },
}

impl StartingDestination<'_> {
    /// The address this destination will answer as — the same hash `Prns::run` registers,
    /// derived purely from the name (and, for a `Single`, its key) so an app can learn it
    /// before the node starts. An announcing responder needs it to name itself in `AnnounceNow`;
    /// the recipe owns the registration, but the address is the app's to know.
    #[allow(clippy::expect_used)]
    pub fn address(&self) -> DestinationHash {
        match self {
            StartingDestination::Plain { app_name, aspects } => {
                let name =
                    expand_name(app_name, aspects).expect("recipe destination name is valid");
                derive_plain_destination_hash(&name)
            }
            StartingDestination::Single {
                app_name,
                aspects,
                identity,
                ..
            } => {
                let signer = InMemoryNodeIdentity::from_secret_key_bytes(identity);
                let name =
                    expand_name(app_name, aspects).expect("recipe destination name is valid");
                derive_destination_hash(&signer.identity_hash(), &name)
            }
        }
    }
}

/// Everything [`Prns::run`](super::Prns::run) needs to stand a node up. The storage recipe is
/// carried by the [`Bind`]'s `Storage` associated type, so the caller writes no turbofish; the
/// interface set, lanes, and host live inside the `Bind`.
pub struct Recipe<B: Bind, D> {
    /// `Some` opts this node into the transport role: relay announces and forward addressed
    /// packets. A bare 16-byte id suffices — forwarding never signs, so this is *not* an
    /// identity and stands apart from any destination's key.
    pub transport: Option<TransportId>,
    /// The destinations stood up before the first packet is ingested
    /// (`impl IntoIterator<Item = StartingDestination>`).
    pub destinations: D,
    /// The platform binding: interfaces, grant lanes, channels, host, reactor.
    pub bind: B,
}
