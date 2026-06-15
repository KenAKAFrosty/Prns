use crate::engine::RatchetPolicy;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name,
};
use crate::routing::ProofStrategy;
use crate::wire::{DestinationHash, TransportId};

use super::PrnsEvent;

pub enum StartingDestination<'a> {
    Plain {
        app_name: &'a str,
        aspects: &'a [&'a str],
    },
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
    /// The address this destination answers as, derived from its name (and key, for a `Single`) —
    /// so an announcing app can name itself before the node starts.
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

/// The complete definition of a node, handed to `Prns::new`: what it is
/// (`transport`, `destinations`), what it holds and how it reacts (`state`, `routes`, `on_event`),
/// and the wires it runs over (`interfaces`). The field bounds (`RouteSet`, `InterfaceSet`, the
/// event handler) are applied at `new()`, so the struct itself stays platform-neutral.
pub struct Recipe<D, St, R, F, I>
where
    F: FnMut(PrnsEvent<'_>, &St),
{
    pub transport: Option<TransportId>,
    pub destinations: D,
    pub state: St,
    pub routes: R,
    pub on_event: F,
    pub interfaces: I,
}
