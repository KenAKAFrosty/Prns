mod impls;

pub use impls::*;

use crate::crypto::Ed25519Signature;
use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::{
    Announce, AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey, ANNOUNCE_FIXED_FIELDS_LEN,
};
use crate::wire::{DestinationHash, HEADER_LEN, MTU};

pub const HELD_APP_DATA_LIMIT: usize = MTU - HEADER_LEN - ANNOUNCE_FIXED_FIELDS_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkOutcome {
    Parked,
    Overwrote,
    CacheFull,
    AppDataTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldReason {
    #[default]
    RoutingArenaPressure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldAnnounce {
    reason: HoldReason,
    destination: DestinationHash,
    public_keys: IdentityPublicKeys,
    dotted_name_hash: DottedNameHash,
    announce_id: AnnounceId,
    maybe_ratchet: Option<RatchetKey>,
    signature: Ed25519Signature,
    app_data_buf: [u8; HELD_APP_DATA_LIMIT],
    app_data_len: u16,
    arrived_at: InstantMillis,
    received_hops: u8,
    source_interface: InterfaceId,
}

impl HeldAnnounce {
    pub fn announce(&self) -> Announce<'_> {
        Announce {
            destination: self.destination,
            public_keys: self.public_keys,
            dotted_name_hash: self.dotted_name_hash,
            announce_id: self.announce_id,
            maybe_ratchet: self.maybe_ratchet,
            signature: self.signature,
            app_data: &self.app_data_buf[..self.app_data_len as usize],
        }
    }

    pub fn arrived_at(&self) -> InstantMillis {
        self.arrived_at
    }

    pub fn received_hops(&self) -> u8 {
        self.received_hops
    }

    pub fn reason(&self) -> HoldReason {
        self.reason
    }

    pub fn source_interface(&self) -> InterfaceId {
        self.source_interface
    }
}

pub trait HeldAnnounces {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn park(
        &mut self,
        announce: &Announce<'_>,
        arrived_at: InstantMillis,
        received_hops: u8,
        reason: HoldReason,
        source_interface: InterfaceId,
    ) -> ParkOutcome;
    fn take_next(&mut self) -> Option<HeldAnnounce>;
}
