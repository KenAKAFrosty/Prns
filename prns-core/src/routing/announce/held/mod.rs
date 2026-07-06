//! Announces held aside by inbound burst control: RNS 1.3.5 `Interface.held_announces`, drip-released lowest-hop-first when the burst subsides (RNS 1.3.5 `process_held_announces`).
//! Entries reuse the retained-announce machinery (app_data in a [`RetainedAppData`] arena), and the queue keeps its own capacity, isolated from the routing table, so a flood can never evict real routes.
//!
//! The reference gives every interface its own 256-entry dict. We share one physical slot pool
//! across interfaces, threaded by an intrusive per-interface chain, so every per-interface operation
//! walks only that interface's chain (bounded by [`MAX_HELD_PER_INTERFACE`]) instead of the whole
//! pool: a host carrying hundreds of peers never pays an all-pool scan to release one interface's
//! lowest-hop announce. The per-interface cap is the reference's parity number; the pool's own
//! capacity is the shared ceiling that keeps the defense itself bounded.

mod impls;

pub use impls::*;

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::announce::retained::{
    AppDataHandle, RetainedAnnounceEntry, RetainedAppData, RetainedAppDataError,
};
use crate::routing::announce::Announce;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};
use crate::routing::NextHop;
use crate::wire::DestinationHash;

/// RNS `Interface.MAX_HELD_ANNOUNCES` (256), read as the per-interface parity cap: the most
/// announces one interface can park at once, matching the reference's per-interface dict.
pub const MAX_HELD_PER_INTERFACE: usize = 256;

/// A slot index into the shared pool. [`NO_SLOT`] terminates a chain and marks an empty free list.
pub type HeldSlot = u32;
pub const NO_SLOT: HeldSlot = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub is_path_response: bool,
    pub announce: RetainedAnnounceEntry,
}

pub(crate) fn vacant_held_announce() -> HeldAnnounce {
    HeldAnnounce {
        destination: DestinationHash::new([0u8; 16]),
        hops: 0,
        receiving_interface: InterfaceId::new([0u8; 8]),
        next_hop: NextHop::Direct,
        is_path_response: false,
        announce: RetainedAnnounceEntry {
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            retained_announce_id: AnnounceId::from_wire([0u8; 10]),
            signature: Ed25519Signature([0u8; 64]),
            ratchet: None,
            maybe_app_data_handle: None,
        },
    }
}

/// One interface's chain: the head slot of its intrusive list and how many announces it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldInterfaceChain {
    pub interface: InterfaceId,
    pub head: HeldSlot,
    pub count: u16,
}

/// The shared slot pool. Backends provide the row storage, the intrusive link column, the free-list
/// head, and the per-interface chain index; [`HeldAnnounces`] owns all the threading logic over them.
pub trait HeldAnnouncePool {
    fn rows(&self) -> &[HeldAnnounce];
    fn rows_mut(&mut self) -> &mut [HeldAnnounce];
    fn links(&self) -> &[HeldSlot];
    fn links_mut(&mut self) -> &mut [HeldSlot];

    fn free_head(&self) -> HeldSlot;
    fn set_free_head(&mut self, slot: HeldSlot);

    fn chains(&self) -> &[HeldInterfaceChain];
    fn chains_mut(&mut self) -> &mut [HeldInterfaceChain];
    fn push_chain(&mut self, chain: HeldInterfaceChain);
    fn swap_remove_chain(&mut self, index: usize);

    /// Claim a fresh slot when the free list is empty. Growable backends push a row and return it;
    /// fixed backends return `None`, so the caller falls back to fairness eviction.
    fn grow_one(&mut self) -> Option<HeldSlot>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOutcome {
    Held,
    Replaced,
    /// A fresher announce for a destination already held did not fit the app-data arena; the
    /// waiting announce stands rather than being lost.
    StaleKept,
    NewcomerDropped(HeldDropCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldDropCause {
    InterfaceAtCap,
    PoolFull,
    ArenaFull,
}

#[derive(Debug, Default)]
pub struct HeldAnnounces<P: HeldAnnouncePool, A: RetainedAppData> {
    pool: P,
    app_data: A,
    len: usize,
}

impl<P: HeldAnnouncePool, A: RetainedAppData> HeldAnnounces<P, A> {
    /// RNS `Interface.hold_announce`: a fresher announce supersedes the one already waiting for the
    /// same `(interface, destination)`; a per-interface parity cap and the shared pool ceiling both
    /// drop a newcomer rather than evict another destination on the same interface. When the pool is
    /// physically full, the highest-hop announce on the fattest *other* interface is evicted, so no
    /// single interface can starve the rest.
    pub fn hold(
        &mut self,
        hops: u8,
        receiving_interface: InterfaceId,
        next_hop: NextHop,
        is_path_response: bool,
        announce: &Announce<'_>,
    ) -> HoldOutcome {
        if let Some(chain_i) = self.chain_index(receiving_interface) {
            if let Some(slot) = self.slot_in_chain(chain_i, &announce.destination) {
                return self.replace_in_place(slot, hops, next_hop, is_path_response, announce);
            }
            if self.pool.chains()[chain_i].count as usize >= MAX_HELD_PER_INTERFACE {
                return HoldOutcome::NewcomerDropped(HeldDropCause::InterfaceAtCap);
            }
        }

        let slot = match self.claim_slot(receiving_interface) {
            Ok(slot) => slot,
            Err(cause) => return HoldOutcome::NewcomerDropped(cause),
        };
        let handle = match self.retain_app_data(announce.app_data) {
            Ok(handle) => handle,
            Err(AppDataFull) => {
                self.release_slot(slot);
                return HoldOutcome::NewcomerDropped(HeldDropCause::ArenaFull);
            }
        };

        self.pool.rows_mut()[slot as usize] = HeldAnnounce {
            destination: announce.destination,
            hops,
            receiving_interface,
            next_hop,
            is_path_response,
            announce: retained_entry(announce, handle),
        };
        self.link_into_chain(receiving_interface, slot);
        self.len += 1;
        HoldOutcome::Held
    }

    /// The lowest-hop announce held for `interface` (RNS `process_held_announces`): copy its metadata
    /// and app_data into `scratch`, free the arena slot, unlink it, and return an owned snapshot the
    /// caller rebuilds an [`Announce`] from without borrowing the queue. `None` means nothing is held
    /// for this interface.
    pub fn release_lowest_hop_for(
        &mut self,
        interface: InterfaceId,
        scratch: &mut [u8],
    ) -> Option<(HeldAnnounce, usize)> {
        let chain_i = self.chain_index(interface)?;
        let slot = self.lowest_hop_slot(chain_i)?;
        let row = self.pool.rows()[slot as usize];
        let app_data_len = match row.announce.maybe_app_data_handle {
            Some(handle) => {
                let bytes = self.app_data.get(handle);
                let len = bytes.len().min(scratch.len());
                scratch[..len].copy_from_slice(&bytes[..len]);
                self.app_data.free(handle);
                len
            }
            None => 0,
        };
        self.unlink_from_chain(interface, slot);
        self.release_slot(slot);
        self.len -= 1;
        Some((row, app_data_len))
    }

    pub fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.pool.chains().iter().map(|chain| chain.interface)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn chain_index(&self, interface: InterfaceId) -> Option<usize> {
        self.pool
            .chains()
            .iter()
            .position(|chain| chain.interface == interface)
    }

    fn slot_in_chain(&self, chain_i: usize, destination: &DestinationHash) -> Option<HeldSlot> {
        let mut slot = self.pool.chains()[chain_i].head;
        while slot != NO_SLOT {
            if self.pool.rows()[slot as usize].destination == *destination {
                return Some(slot);
            }
            slot = self.pool.links()[slot as usize];
        }
        None
    }

    fn lowest_hop_slot(&self, chain_i: usize) -> Option<HeldSlot> {
        let mut slot = self.pool.chains()[chain_i].head;
        let mut best: Option<HeldSlot> = None;
        let mut best_hops = u8::MAX;
        while slot != NO_SLOT {
            let hops = self.pool.rows()[slot as usize].hops;
            if hops <= best_hops {
                best_hops = hops;
                best = Some(slot);
            }
            slot = self.pool.links()[slot as usize];
        }
        best
    }

    fn highest_hop_slot(&self, chain_i: usize) -> Option<HeldSlot> {
        let mut slot = self.pool.chains()[chain_i].head;
        let mut best: Option<HeldSlot> = None;
        let mut best_hops = 0u8;
        while slot != NO_SLOT {
            let hops = self.pool.rows()[slot as usize].hops;
            if best.is_none() || hops >= best_hops {
                best_hops = hops;
                best = Some(slot);
            }
            slot = self.pool.links()[slot as usize];
        }
        best
    }

    fn replace_in_place(
        &mut self,
        slot: HeldSlot,
        hops: u8,
        next_hop: NextHop,
        is_path_response: bool,
        announce: &Announce<'_>,
    ) -> HoldOutcome {
        let old_handle = self.pool.rows()[slot as usize]
            .announce
            .maybe_app_data_handle;
        let new_handle = match self.reseat_app_data(old_handle, announce.app_data) {
            Ok(handle) => handle,
            Err(AppDataFull) => return HoldOutcome::StaleKept,
        };
        let row = &mut self.pool.rows_mut()[slot as usize];
        row.hops = hops;
        row.next_hop = next_hop;
        row.is_path_response = is_path_response;
        row.announce = retained_entry(announce, new_handle);
        HoldOutcome::Replaced
    }

    /// Secure a pool slot for a new destination on `interface`: recycle a free slot, grow the pool,
    /// or (when it is physically full) evict the highest-hop announce off the fattest interface that
    /// is strictly fatter than this one, so no interface starves the rest.
    fn claim_slot(&mut self, interface: InterfaceId) -> Result<HeldSlot, HeldDropCause> {
        let free = self.pool.free_head();
        if free != NO_SLOT {
            self.pool.set_free_head(self.pool.links()[free as usize]);
            return Ok(free);
        }
        if let Some(slot) = self.pool.grow_one() {
            return Ok(slot);
        }
        self.evict_for(interface).ok_or(HeldDropCause::PoolFull)
    }

    fn evict_for(&mut self, newcomer: InterfaceId) -> Option<HeldSlot> {
        let newcomer_count = self
            .chain_index(newcomer)
            .map_or(0, |i| self.pool.chains()[i].count);
        let victim_interface = self
            .pool
            .chains()
            .iter()
            .filter(|chain| chain.count > newcomer_count)
            .max_by_key(|chain| chain.count)
            .map(|chain| chain.interface)?;
        let victim_chain = self.chain_index(victim_interface)?;
        let slot = self.highest_hop_slot(victim_chain)?;
        if let Some(handle) = self.pool.rows()[slot as usize]
            .announce
            .maybe_app_data_handle
        {
            self.app_data.free(handle);
        }
        self.unlink_from_chain(victim_interface, slot);
        self.len -= 1;
        Some(slot)
    }

    fn link_into_chain(&mut self, interface: InterfaceId, slot: HeldSlot) {
        match self.chain_index(interface) {
            Some(chain_i) => {
                let head = self.pool.chains()[chain_i].head;
                self.pool.links_mut()[slot as usize] = head;
                let chain = &mut self.pool.chains_mut()[chain_i];
                chain.head = slot;
                chain.count += 1;
            }
            None => {
                self.pool.links_mut()[slot as usize] = NO_SLOT;
                self.pool.push_chain(HeldInterfaceChain {
                    interface,
                    head: slot,
                    count: 1,
                });
            }
        }
    }

    fn unlink_from_chain(&mut self, interface: InterfaceId, slot: HeldSlot) {
        let Some(chain_i) = self.chain_index(interface) else {
            return;
        };
        let head = self.pool.chains()[chain_i].head;
        if head == slot {
            self.pool.chains_mut()[chain_i].head = self.pool.links()[slot as usize];
        } else {
            let mut prev = head;
            while prev != NO_SLOT {
                let next = self.pool.links()[prev as usize];
                if next == slot {
                    self.pool.links_mut()[prev as usize] = self.pool.links()[slot as usize];
                    break;
                }
                prev = next;
            }
        }
        let chain = &mut self.pool.chains_mut()[chain_i];
        chain.count -= 1;
        if chain.count == 0 {
            self.pool.swap_remove_chain(chain_i);
        }
    }

    fn release_slot(&mut self, slot: HeldSlot) {
        self.pool.links_mut()[slot as usize] = self.pool.free_head();
        self.pool.set_free_head(slot);
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

    /// Move an existing entry's app_data to a new payload without losing the old bytes on failure:
    /// [`RetainedAppData::replace`] validates before it touches the arena, so an over-budget payload
    /// leaves the waiting announce intact ([`HoldOutcome::StaleKept`]).
    fn reseat_app_data(
        &mut self,
        old_handle: Option<AppDataHandle>,
        app_data: &[u8],
    ) -> Result<Option<AppDataHandle>, AppDataFull> {
        match (old_handle, app_data.is_empty()) {
            (Some(handle), true) => {
                self.app_data.free(handle);
                Ok(None)
            }
            (Some(handle), false) => match self.app_data.replace(handle, app_data) {
                Ok(()) => Ok(Some(handle)),
                Err(_) => Err(AppDataFull),
            },
            (None, _) => self.retain_app_data(app_data),
        }
    }
}

fn retained_entry(announce: &Announce<'_>, handle: Option<AppDataHandle>) -> RetainedAnnounceEntry {
    RetainedAnnounceEntry {
        public_keys: announce.public_keys,
        dotted_name_hash: announce.dotted_name_hash,
        retained_announce_id: announce.announce_id,
        signature: announce.signature,
        ratchet: announce.ratchet,
        maybe_app_data_handle: handle,
    }
}

struct AppDataFull;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::announce::retained::PackedAppDataArena;

    type Held = HeldAnnounces<FixedHeldAnnouncePool<4>, PackedAppDataArena<512, 4>>;

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
            "a fresher announce for the same destination supersedes the waiting one",
        );
        assert_eq!(held.len(), 1, "replacing does not grow the queue");

        let mut scratch = [0u8; 64];
        let (row, len) = held.release_lowest_hop_for(iface(1), &mut scratch).unwrap();
        assert_eq!(row.hops, 2, "the replacement's hop count and payload won");
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
        let (row, len) = held.release_lowest_hop_for(iface(1), &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        assert_eq!(row.hops, 2);
        assert_eq!(&scratch[..len], b"near");
        assert_eq!(held.len(), 2, "only the released entry leaves the queue");
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
            "the key is (interface, destination): interface 2 does not replace interface 1's hold",
        );
        assert_eq!(held.len(), 2);

        let mut scratch = [0u8; 64];
        let (row, len) = held.release_lowest_hop_for(iface(2), &mut scratch).unwrap();
        assert_eq!(row.receiving_interface, iface(2));
        assert_eq!(&scratch[..len], b"two");
        assert!(
            held.release_lowest_hop_for(iface(1), &mut scratch)
                .is_some(),
            "interface 1's hold survives releasing interface 2's",
        );
    }

    #[test]
    fn a_full_pool_evicts_the_highest_hop_off_the_fattest_interface() {
        let mut held = Held::default();
        hold(&mut held, dest(0x10), 2, iface(1), 1, b"a");
        hold(&mut held, dest(0x11), 9, iface(1), 2, b"b");
        hold(&mut held, dest(0x12), 5, iface(1), 3, b"c");
        hold(&mut held, dest(0x20), 3, iface(2), 4, b"d");
        assert_eq!(held.len(), 4, "the pool is full");

        assert_eq!(
            hold(&mut held, dest(0x21), 1, iface(2), 5, b"e"),
            HoldOutcome::Held,
            "a newcomer on the lean interface evicts the fattest interface's worst announce",
        );
        assert_eq!(held.len(), 4);

        let mut scratch = [0u8; 64];
        let mut iface1_hops = std::vec::Vec::new();
        while let Some((row, _)) = held.release_lowest_hop_for(iface(1), &mut scratch) {
            iface1_hops.push(row.hops);
        }
        assert_eq!(
            iface1_hops,
            std::vec![2, 5],
            "the 9-hop announce on the fattest interface was the eviction victim",
        );
    }

    #[test]
    fn a_newcomer_on_the_fattest_interface_is_dropped_rather_than_self_evicting() {
        let mut held = Held::default();
        for i in 0..4u8 {
            hold(&mut held, dest(i), i + 1, iface(1), i, b"x");
        }
        assert_eq!(held.len(), 4);
        assert_eq!(
            hold(&mut held, dest(0xFF), 1, iface(1), 9, b"x"),
            HoldOutcome::NewcomerDropped(HeldDropCause::PoolFull),
            "on a single-interface node a full pool drops the newcomer, matching the reference",
        );
        assert_eq!(held.len(), 4);
    }

    #[test]
    fn interfaces_release_independently() {
        let mut held = Held::default();
        hold(&mut held, dest(0xA1), 4, iface(1), 1, b"a");
        hold(&mut held, dest(0xB2), 7, iface(2), 2, b"b");

        let seen: std::vec::Vec<_> = held.interfaces().collect();
        assert!(seen.contains(&iface(1)) && seen.contains(&iface(2)));

        let mut scratch = [0u8; 64];
        let (row, _) = held.release_lowest_hop_for(iface(2), &mut scratch).unwrap();
        assert_eq!(row.destination, dest(0xB2));
        let after: std::vec::Vec<_> = held.interfaces().collect();
        assert_eq!(
            after,
            std::vec![iface(1)],
            "the drained interface leaves the index",
        );
    }
}
