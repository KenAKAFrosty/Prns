use crate::engine::RatchetPolicy;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name, ExpandNameError,
};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::{LinkRequestPolicy, ProofStrategy};
use crate::wire::DestinationHash;

use super::super::PrnsEvent;

pub enum PreConfiguredDestination<'a> {
    Plain {
        app_name: &'a str,
        aspects: &'a [&'a str],
    },
    Single {
        app_name: &'a str,
        aspects: &'a [&'a str],
        identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        announce_app_data: &'a [u8],
        proof: ProofStrategy,
        link_requests: LinkRequestPolicy,
        ratchet: RatchetPolicy,
        /// Whether links to this destination accept inbound resources, and how large. The runtime counterpart is the handle's `set_resource_strategy`; most destinations want `ResourceStrategy::AcceptNone` until they expect a transfer.
        resource_strategy: ResourceStrategy,
    },
}

impl PreConfiguredDestination<'_> {
    /// The address this destination answers as, derived from its name (and key, for a `Single`), so an announcing app can name itself before the node starts. `Err` only when the name is malformed (a dotted component, or past the length bound), the same validation `PrnsNode::new` runs as it stands the destination up.
    pub fn destination_hash(&self) -> Result<DestinationHash, ExpandNameError> {
        match self {
            PreConfiguredDestination::Plain { app_name, aspects } => Ok(
                derive_plain_destination_hash(&expand_name(app_name, aspects)?),
            ),
            PreConfiguredDestination::Single {
                app_name,
                aspects,
                identity,
                ..
            } => {
                let signer = InMemoryNodeIdentity::from_secret_key_bytes(identity);
                Ok(derive_destination_hash(
                    &signer.identity_hash(),
                    &expand_name(app_name, aspects)?,
                ))
            }
        }
    }
}

/// The explicit "I wire interfaces myself" answer to the recipe's `interfaces` field: attach everything after construction through the node handle (or, on a board, at slot activation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manual;

pub struct PrnsNodeRecipe<Destinations, AppState, Routes, OnEvent, Interfaces, Storage>
where
    OnEvent: FnMut(PrnsEvent<'_>, &AppState),
{
    /// The transport role takes a whole identity, never a bare address: a transport node signs (tunnel synthesis), and RNS 1.3.5 keeps a dedicated persisted transport identity.
    pub transport_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    pub pre_configured_destinations: Destinations,
    pub app_state: AppState,
    /// The storage layout the engine's columns run on: `GrowableHeap` on a std host, a fixed prepackage (`Esp32S3`/`Esp32C6`/`Nrf52840`) on a board. A type-level choice carried as a value so the recipe owns it and `PrnsNode::new` no longer assumes one.
    pub storage: Storage,
    pub routes: Routes,
    pub interfaces: Interfaces,
    pub on_event: OnEvent,
}
