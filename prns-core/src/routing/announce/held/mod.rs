//! Announces held aside by inbound burst control: RNS 1.3.5 `Interface.held_announces`, drip-released lowest-hop-first when the burst subsides (RNS 1.3.5 `process_held_announces`).
//! Entries reuse the retained-announce machinery (app_data in a [`RetainedAppData`] arena), and the queue keeps its own capacity, isolated from the routing table, so a flood can never evict real routes.

mod impls;
pub use impls::*;

use crate::interfaces::InterfaceId;
use crate::routing::announce::retained::{
    AppDataHandle, RetainedAnnounceEntry, RetainedAppData, RetainedAppDataError,
};
use crate::routing::announce::Announce;
use crate::routing::NextHop;
use crate::wire::DestinationHash;

/// RNS `Interface.MAX_HELD_ANNOUNCES` (256): the ceiling a growable held queue caps itself at, so a flood can never make the defense itself unbounded.
pub const MAX_HELD_ANNOUNCES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub is_path_response: bool,
    pub announce: RetainedAnnounceEntry,
}

pub trait HeldAnnounceColumns {
    fn capacity(&self) -> usize;
    fn rows(&self) -> &[HeldAnnounce];
    fn rows_mut(&mut self) -> &mut [HeldAnnounce];

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.rows()
            .iter()
            .position(|row| row.destination == *destination)
    }

    fn push(&mut self, row: HeldAnnounce);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOutcome {
    Held,
    Replaced,
    QueueFull,
}

#[derive(Debug, Default)]
pub struct HeldAnnounces<C: HeldAnnounceColumns, A: RetainedAppData> {
    columns: C,
    app_data: A,
}

impl<C: HeldAnnounceColumns, A: RetainedAppData> HeldAnnounces<C, A> {
    /// RNS `Interface.hold_announce` (Interface.py:228): a fresher announce supersedes the waiting one; a full queue drops the newcomer rather than evicting another destination.
    pub fn hold(
        &mut self,
        hops: u8,
        receiving_interface: InterfaceId,
        next_hop: NextHop,
        is_path_response: bool,
        announce: &Announce<'_>,
    ) -> HoldOutcome {
        //This probably needs a cleaner re-work. One easy one is the capacity check at the beginning, should be cheap and fast, and no need to do a lookup across the table just to reject for fullness.

        //That should let us do early returns the other way too, I think. More let blah else{ return something } rather than if let blah else {}, else if
        if let Some(index) = self.columns.index_of(&announce.destination) {
            if let Some(handle) = self.columns.rows()[index].announce.maybe_app_data_handle {
                self.app_data.free(handle);
            }
            match self.retain(announce) {
                Ok(retained) => {
                    let row = &mut self.columns.rows_mut()[index];
                    row.hops = hops;
                    row.receiving_interface = receiving_interface;
                    row.next_hop = next_hop;
                    row.is_path_response = is_path_response;
                    row.announce = retained;
                    HoldOutcome::Replaced
                }
                Err(_) => {
                    self.columns.swap_remove(index);
                    HoldOutcome::QueueFull
                }
            }
        } else if self.columns.rows().len() < self.columns.capacity() {
            match self.retain(announce) {
                Ok(retained) => {
                    self.columns.push(HeldAnnounce {
                        destination: announce.destination,
                        hops,
                        receiving_interface,
                        next_hop,
                        is_path_response,
                        announce: retained,
                    });
                    HoldOutcome::Held
                }
                Err(_) => HoldOutcome::QueueFull,
            }
        } else {
            HoldOutcome::QueueFull
        }
    }

    pub fn lowest_hop_slot(&self, interface: InterfaceId) -> Option<usize> {
        self.columns
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, row)| row.receiving_interface == interface)
            .min_by_key(|(_, row)| row.hops)
            .map(|(index, _)| index)
    }

    //`has_for` is a bit of weird language here, maybe something slightly semantically better would be good
    pub fn has_for(&self, interface: InterfaceId) -> bool {
        self.columns
            .rows()
            .iter()
            .any(|row| row.receiving_interface == interface)
    }

    pub fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.columns
            .rows()
            .iter()
            .map(|row| row.receiving_interface)
    }

    /// Returns an owned snapshot the caller can rebuild an [`Announce`] from without borrowing the queue.
    pub fn take(
        &mut self,
        index: usize,
        app_data_scratch: &mut [u8],
    ) -> Option<(HeldAnnounce, usize)> {
        let row = *self.columns.rows().get(index)?; //instead of flattened-option-ifying everything here, probably need to flip this to a result instead yeah?
        let app_data_len = match row.announce.maybe_app_data_handle {
            Some(handle) => {
                let bytes = self.app_data.get(handle);
                let len = bytes.len().min(app_data_scratch.len());
                app_data_scratch[..len].copy_from_slice(&bytes[..len]);
                self.app_data.free(handle);
                len
            }
            None => 0,
        };
        self.columns.swap_remove(index);
        Some((row, app_data_len))
    }

    pub fn len(&self) -> usize {
        self.columns.rows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.rows().is_empty()
    }

    // I'm really not quite following what these 2 retain functions do. Either a rename or a rework is probably in order
    fn retain(
        &mut self,
        announce: &Announce<'_>,
    ) -> Result<RetainedAnnounceEntry, AppDataHoldError> {
        let maybe_app_data_handle = self.retain_app_data(announce.app_data)?;
        Ok(RetainedAnnounceEntry {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            retained_announce_id: announce.announce_id,
            signature: announce.signature,
            ratchet: announce.ratchet,
            maybe_app_data_handle,
        })
    }

    fn retain_app_data(
        &mut self,
        app_data: &[u8],
    ) -> Result<Option<AppDataHandle>, AppDataHoldError> {
        if app_data.is_empty() {
            return Ok(None);
        }
        match self.app_data.insert(app_data) {
            Ok(handle) => Ok(Some(handle)),
            Err(RetainedAppDataError::ArenaFull | RetainedAppDataError::TooManyEntries) => {
                Err(AppDataHoldError::ArenaFull)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppDataHoldError {
    ArenaFull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::announce::retained::PackedAppDataArena;
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};

    type Held = HeldAnnounces<FixedHeldAnnounceColumns<4>, PackedAppDataArena<512, 4>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn announce_id(byte: u8) -> AnnounceId {
        AnnounceId::from_wire([byte; 10])
    }

    fn announce<'a>(destination: DestinationHash, id: u8, app_data: &'a [u8]) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: announce_id(id),
            ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    #[test]
    fn holding_parks_an_announce_and_a_resend_replaces_it_in_place() {
        let mut held = Held::default();
        assert_eq!(
            held.hold(
                3,
                iface(1),
                NextHop::Direct,
                false,
                &announce(dest(0xA1), 1, b"first")
            ),
            HoldOutcome::Held,
        );
        assert_eq!(held.len(), 1);
        assert_eq!(
            held.hold(
                2,
                iface(1),
                NextHop::Direct,
                false,
                &announce(dest(0xA1), 2, b"second")
            ),
            HoldOutcome::Replaced,
            "a fresher announce for the same destination supersedes the waiting one",
        );
        assert_eq!(held.len(), 1, "replacing does not grow the queue");

        let mut scratch = [0u8; 64];
        let slot = held.lowest_hop_slot(iface(1)).unwrap();
        let (row, len) = held.take(slot, &mut scratch).unwrap();
        assert_eq!(row.hops, 2, "the replacement's hop count and payload won");
        assert_eq!(&scratch[..len], b"second");
        assert!(held.is_empty());
    }

    #[test]
    fn release_picks_the_lowest_hop_announce_for_the_interface() {
        let mut held = Held::default();
        held.hold(
            5,
            iface(1),
            NextHop::Direct,
            false,
            &announce(dest(0xA1), 1, b"far"),
        );
        held.hold(
            2,
            iface(1),
            NextHop::Direct,
            false,
            &announce(dest(0xB2), 2, b"near"),
        );
        held.hold(
            9,
            iface(1),
            NextHop::Direct,
            false,
            &announce(dest(0xC3), 3, b"farther"),
        );

        let mut scratch = [0u8; 64];
        let slot = held.lowest_hop_slot(iface(1)).unwrap();
        let (row, len) = held.take(slot, &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        assert_eq!(row.hops, 2);
        assert_eq!(&scratch[..len], b"near");
        assert_eq!(held.len(), 2, "only the released entry leaves the queue");
    }

    #[test]
    fn a_full_queue_drops_a_new_destination_rather_than_evicting_another() {
        let mut held = Held::default();
        for byte in 0..4u8 {
            assert_eq!(
                held.hold(
                    1,
                    iface(1),
                    NextHop::Direct,
                    false,
                    &announce(dest(byte), byte, b"x")
                ),
                HoldOutcome::Held,
            );
        }
        assert_eq!(
            held.hold(
                1,
                iface(1),
                NextHop::Direct,
                false,
                &announce(dest(0xFF), 9, b"x")
            ),
            HoldOutcome::QueueFull,
            "the queue's own capacity protects real routes from flood eviction",
        );
        assert_eq!(held.len(), 4);
    }

    #[test]
    fn interfaces_release_independently() {
        let mut held = Held::default();
        held.hold(
            4,
            iface(1),
            NextHop::Direct,
            false,
            &announce(dest(0xA1), 1, b"a"),
        );
        held.hold(
            7,
            iface(2),
            NextHop::Direct,
            false,
            &announce(dest(0xB2), 2, b"b"),
        );

        assert!(held.has_for(iface(1)) && held.has_for(iface(2)));
        let slot = held.lowest_hop_slot(iface(2)).unwrap();
        let (row, _) = held.take(slot, &mut [0u8; 16]).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        assert!(held.has_for(iface(1)) && !held.has_for(iface(2)));
    }
}
