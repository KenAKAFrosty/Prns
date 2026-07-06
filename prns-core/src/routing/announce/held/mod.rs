mod impls;

pub use impls::*;

use crate::interfaces::InterfaceId;
use crate::routing::announce::retained::{
    AppDataHandle, RetainedAnnounceEntry, RetainedAppData, RetainedAppDataError,
};
use crate::routing::announce::Announce;
use crate::routing::NextHop;
use crate::wire::DestinationHash;

///  RNS `Interface.MAX_HELD_ANNOUNCES`
pub const MAX_HELD_ANNOUNCES_PER_INTERFACE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub is_path_response: bool,
    pub announce: RetainedAnnounceEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOutcome {
    Held,
    Replaced,
    StaleKept,
    NewcomerDropped(HeldDropCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldDropCause {
    InterfaceAtCap,
    PoolFull,
    ArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldFull {
    InterfaceAtCap,
    PoolFull,
}

impl From<HeldFull> for HeldDropCause {
    fn from(full: HeldFull) -> Self {
        match full {
            HeldFull::InterfaceAtCap => HeldDropCause::InterfaceAtCap,
            HeldFull::PoolFull => HeldDropCause::PoolFull,
        }
    }
}

pub trait HeldStore {
    type Slot: Copy;

    fn find(&self, interface: InterfaceId, destination: DestinationHash) -> Option<Self::Slot>;
    fn app_data_handle(&self, slot: Self::Slot) -> Option<AppDataHandle>;
    fn overwrite(&mut self, slot: Self::Slot, record: HeldAnnounce);
    fn insert(&mut self, record: HeldAnnounce) -> Result<(), HeldFull>;
    fn take_lowest_hop_for(&mut self, interface: InterfaceId) -> Option<HeldAnnounce>;
    fn drop_interface(
        &mut self,
        interface: InterfaceId,
        on_removed: impl FnMut(Option<AppDataHandle>),
    );
    fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Default)]
pub struct HeldAnnounces<S: HeldStore, A: RetainedAppData> {
    store: S,
    app_data: A,
}

impl<S: HeldStore, A: RetainedAppData> HeldAnnounces<S, A> {
    pub fn hold(
        &mut self,
        hops: u8,
        receiving_interface: InterfaceId,
        next_hop: NextHop,
        is_path_response: bool,
        announce: &Announce<'_>,
    ) -> HoldOutcome {
        if let Some(slot) = self.store.find(receiving_interface, announce.destination) {
            let current = self.store.app_data_handle(slot);
            let refreshed = match self.upsert_app_data(current, announce.app_data) {
                Ok(handle) => handle,
                Err(AppDataFull) => return HoldOutcome::StaleKept,
            };
            self.store.overwrite(
                slot,
                into_held_record(IntoHeldRecordInputs {
                    hops,
                    receiving_interface,
                    next_hop,
                    is_path_response,
                    announce,
                    handle: refreshed,
                }),
            );
            return HoldOutcome::Replaced;
        }

        let handle = match self.retain_app_data(announce.app_data) {
            Ok(handle) => handle,
            Err(AppDataFull) => return HoldOutcome::NewcomerDropped(HeldDropCause::ArenaFull),
        };
        match self.store.insert(into_held_record(IntoHeldRecordInputs {
            hops,
            receiving_interface,
            next_hop,
            is_path_response,
            announce,
            handle,
        })) {
            Ok(()) => HoldOutcome::Held,
            Err(full) => {
                if let Some(handle) = handle {
                    self.app_data.free(handle);
                }
                HoldOutcome::NewcomerDropped(full.into())
            }
        }
    }

    pub fn release_lowest_hop_for(
        &mut self,
        interface: InterfaceId,
        scratch: &mut [u8],
    ) -> Option<ReleasedLowestHops> {
        let record = self.store.take_lowest_hop_for(interface)?;

        let app_data_len = match record.announce.maybe_app_data_handle {
            None => 0,
            Some(handle) => {
                let bytes = self.app_data.get(handle);
                let len = bytes.len().min(scratch.len());
                scratch[..len].copy_from_slice(&bytes[..len]);
                self.app_data.free(handle);
                len
            }
        };

        Some(ReleasedLowestHops {
            held_announce: record,
            app_data_len,
        })
    }

    pub fn drop_interface(&mut self, interface: InterfaceId) {
        let app_data = &mut self.app_data;
        self.store.drop_interface(interface, |handle| {
            if let Some(handle) = handle {
                app_data.free(handle);
            }
        });
    }

    pub fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.store.interfaces()
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    fn retain_app_data(&mut self, app_data: &[u8]) -> Result<Option<AppDataHandle>, AppDataFull> {
        if app_data.is_empty() {
            return Ok(None);
        }
        match self.app_data.insert(app_data) {
            Ok(handle) => Ok(Some(handle)),
            Err(RetainedAppDataError::ArenaFull | RetainedAppDataError::TooManyEntries) => {
                Err(AppDataFull)
            }
        }
    }

    fn upsert_app_data(
        &mut self,
        previous_handle: Option<AppDataHandle>,
        new_app_data: &[u8],
    ) -> Result<Option<AppDataHandle>, AppDataFull> {
        match (previous_handle, new_app_data.is_empty()) {
            (Some(handle), true) => {
                self.app_data.free(handle);
                Ok(None)
            }
            (Some(handle), false) => match self.app_data.replace(handle, new_app_data) {
                Ok(()) => Ok(Some(handle)),
                Err(_) => Err(AppDataFull),
            },
            (None, _) => self.retain_app_data(new_app_data),
        }
    }
}

pub struct ReleasedLowestHops {
    pub held_announce: HeldAnnounce,
    pub app_data_len: usize,
}

struct IntoHeldRecordInputs<'announce> {
    hops: u8,
    receiving_interface: InterfaceId,
    next_hop: NextHop,
    is_path_response: bool,
    announce: &'announce Announce<'announce>,
    handle: Option<AppDataHandle>,
}

fn into_held_record(inputs: IntoHeldRecordInputs) -> HeldAnnounce {
    let IntoHeldRecordInputs {
        hops,
        next_hop,
        announce,
        receiving_interface,
        is_path_response,
        handle,
    } = inputs;

    HeldAnnounce {
        destination: announce.destination,
        hops,
        receiving_interface,
        next_hop,
        is_path_response,
        announce: RetainedAnnounceEntry {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            retained_announce_id: announce.announce_id,
            signature: announce.signature,
            ratchet: announce.ratchet,
            maybe_app_data_handle: handle,
        },
    }
}

struct AppDataFull;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::announce::retained::PackedAppDataArena;
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};

    type Held = HeldAnnounces<FixedHeldStore<4>, PackedAppDataArena<512, 4>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn announce<'a>(destination: DestinationHash, id: u8, app_data: &'a [u8]) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: AnnounceId::from_wire([id; 10]),
            ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    fn hold(
        held: &mut Held,
        destination: DestinationHash,
        hops: u8,
        interface: InterfaceId,
        id: u8,
        app_data: &[u8],
    ) -> HoldOutcome {
        held.hold(
            hops,
            interface,
            NextHop::Direct,
            false,
            &announce(destination, id, app_data),
        )
    }

    #[test]
    fn holding_parks_an_announce_and_a_resend_replaces_it_in_place() {
        let mut held = Held::default();
        assert_eq!(
            hold(&mut held, dest(0xA1), 3, iface(1), 1, b"first"),
            HoldOutcome::Held,
        );
        assert_eq!(held.len(), 1);
        assert_eq!(
            hold(&mut held, dest(0xA1), 2, iface(1), 2, b"second"),
            HoldOutcome::Replaced,
        );
        assert_eq!(held.len(), 1);

        let mut scratch = [0u8; 64];
        let ReleasedLowestHops {
            held_announce: row,
            app_data_len: len,
        } = held.release_lowest_hop_for(iface(1), &mut scratch).unwrap();
        assert_eq!(row.hops, 2);
        assert_eq!(&scratch[..len], b"second");
        assert!(held.is_empty());
    }

    #[test]
    fn release_picks_the_lowest_hop_announce_for_the_interface() {
        let mut held = Held::default();
        hold(&mut held, dest(0xA1), 5, iface(1), 1, b"far");
        hold(&mut held, dest(0xB2), 2, iface(1), 2, b"near");
        hold(&mut held, dest(0xC3), 9, iface(1), 3, b"farther");

        let mut scratch = [0u8; 64];
        let ReleasedLowestHops {
            held_announce: row,
            app_data_len: len,
        } = held.release_lowest_hop_for(iface(1), &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        assert_eq!(row.hops, 2);
        assert_eq!(&scratch[..len], b"near");
        assert_eq!(held.len(), 2);
    }

    #[test]
    fn the_same_destination_on_two_interfaces_is_two_independent_holds() {
        let mut held = Held::default();
        assert_eq!(
            hold(&mut held, dest(0xA1), 3, iface(1), 1, b"one"),
            HoldOutcome::Held,
        );
        assert_eq!(
            hold(&mut held, dest(0xA1), 4, iface(2), 2, b"two"),
            HoldOutcome::Held,
        );
        assert_eq!(held.len(), 2);

        let mut scratch = [0u8; 64];
        let ReleasedLowestHops {
            held_announce: row,
            app_data_len: len,
        } = held.release_lowest_hop_for(iface(2), &mut scratch).unwrap();
        assert_eq!(row.receiving_interface, iface(2));
        assert_eq!(&scratch[..len], b"two");
        assert!(held
            .release_lowest_hop_for(iface(1), &mut scratch)
            .is_some());
    }

    #[test]
    fn a_full_pool_refuses_the_newcomer() {
        let mut held = Held::default();
        hold(&mut held, dest(0x10), 2, iface(1), 1, b"");
        hold(&mut held, dest(0x11), 9, iface(1), 2, b"");
        hold(&mut held, dest(0x20), 5, iface(2), 3, b"");
        hold(&mut held, dest(0x21), 3, iface(2), 4, b"");
        assert_eq!(held.len(), 4);

        assert_eq!(
            hold(&mut held, dest(0x30), 1, iface(3), 5, b""),
            HoldOutcome::NewcomerDropped(HeldDropCause::PoolFull),
        );
        assert_eq!(
            hold(&mut held, dest(0x12), 1, iface(1), 6, b""),
            HoldOutcome::NewcomerDropped(HeldDropCause::PoolFull),
        );
        assert_eq!(held.len(), 4);
    }

    #[test]
    fn dropping_an_interface_frees_all_of_its_holds() {
        let mut held = Held::default();
        hold(&mut held, dest(0xA1), 3, iface(1), 1, b"a");
        hold(&mut held, dest(0xB2), 4, iface(1), 2, b"b");
        hold(&mut held, dest(0xC3), 5, iface(2), 3, b"c");
        assert_eq!(held.len(), 3);

        held.drop_interface(iface(1));
        assert_eq!(held.len(), 1);
        let remaining: std::vec::Vec<_> = held.interfaces().collect();
        assert_eq!(remaining, std::vec![iface(2)]);

        let mut scratch = [0u8; 64];
        let ReleasedLowestHops {
            held_announce: row,
            app_data_len: len,
        } = held.release_lowest_hop_for(iface(2), &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xC3));
        assert_eq!(&scratch[..len], b"c");
    }

    #[test]
    fn interfaces_release_independently() {
        let mut held = Held::default();
        hold(&mut held, dest(0xA1), 4, iface(1), 1, b"a");
        hold(&mut held, dest(0xB2), 7, iface(2), 2, b"b");

        let seen: std::vec::Vec<_> = held.interfaces().collect();
        assert!(seen.contains(&iface(1)) && seen.contains(&iface(2)));

        let mut scratch = [0u8; 64];
        let ReleasedLowestHops {
            held_announce: row, ..
        } = held.release_lowest_hop_for(iface(2), &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        let after: std::vec::Vec<_> = held.interfaces().collect();
        assert_eq!(after, std::vec![iface(1)]);
    }
}
