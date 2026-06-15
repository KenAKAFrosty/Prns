use crate::engine::RatchetPolicy;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name, ExpandNameError,
};
use crate::routing::ProofStrategy;
use crate::wire::{DestinationHash, TransportId};

use super::PrnsEvent;

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
        ratchet: RatchetPolicy,
    },
}

impl PreConfiguredDestination<'_> {
    /// The address this destination answers as, derived from its name (and key, for a `Single`),
    /// so an announcing app can name itself before the node starts. `Err` only when the name is
    /// malformed (a dotted component, or past the length bound), the same validation `Prns::new`
    /// runs as it stands the destination up.
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

pub struct PrnsRecipe<Destinations, AppState, Routes, OnEvent, Interfaces>
where
    OnEvent: FnMut(PrnsEvent<'_>, &AppState),
{
    pub transport: Option<TransportId>,
    pub pre_configured_destinations: Destinations,
    pub state: AppState,
    pub routes: Routes,
    pub interfaces: Interfaces,
    pub on_event: OnEvent,
}
