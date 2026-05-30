//! Held-cache: a structured store of announces awaiting action.
//!
//! Mirrors RNS's `Interface.held_announces` shape (destination-keyed,
//! lowest-hops-first selection) but lives at the engine level today since
//! we don't have interfaces yet. When interfaces arrive, per-interface
//! held caches can share this same type.
//!
//! ## Why announces are held
//!
//! Today's caller: [`upsert_route`](crate::routing::RoutingTable::upsert_route)
//! returns `Dropped(PayloadArenaFull)` and the engine parks the announce here
//! for retry on subsequent ticks. RNS rate-limiting and bandwidth-aware
//! emission ratios will share this cache as those slices arrive.
//!
//! ## Behavior (RNS parity)
//!
//! - **Destination-keyed.** Park scans for an existing entry for the same
//!   destination; if found, the newer announce overwrites the older. RNS
//!   does the same: `Transport.held_announces` is a dict keyed by
//!   destination_hash.
//! - **Cap-full drops the new one.** When at `CAPACITY` and a fresh
//!   destination arrives, the new announce is silently dropped — same as
//!   RNS's [Interface.hold_announce](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L228).
//!   No LRU eviction: holds are already "soft" data that the network will
//!   refresh on its own.
//! - **Lowest-hops-first release.** [`take_next`](HeldAnnouncesCache::take_next)
//!   returns the entry with the smallest `received_hops`, ties broken by
//!   oldest `arrived_at`. Mirrors RNS's
//!   [process_held_announces](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L234)
//!   and the same prioritization in `process_announce_queue` — closer
//!   destinations carry more useful routing info, so they ship first.
//!
//! ## Storage shape
//!
//! Struct-of-arrays: hot scan columns (`destinations`, `received_hops`,
//! `arrived_at`) pack densely for cache-friendly walks during park dedup
//! and take_next selection; cold columns hold the announce content the
//! caller assembles back into an [`Announce`] only on take. The per-entry
//! `app_data` inline buffer is sized to the wire-protocol maximum
//! ([`HELD_APP_DATA_LIMIT`]) so every valid announce fits without
//! `AppDataTooLarge`.

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::engine::InstantMillis;
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::announce::{
    Announce, AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey, ANNOUNCE_ID_WIRE_LEN,
};
use crate::wire::{
    DestinationHash, ANNOUNCE_PUBLIC_KEY_LEN, DOTTED_NAME_HASH_LEN, HEADER_LEN, MTU, SIGNATURE_LEN,
};

pub const DEFAULT_HELD_CACHE_CAPACITY: usize = 64;

/// Per-entry inline budget for an announce's `app_data` — the wire-protocol
/// max after the fixed-width fields. Computed from the wire constants so it
/// stays coupled to MTU and field widths; every valid announce on the wire
/// fits without `AppDataTooLarge`.
pub const HELD_APP_DATA_LIMIT: usize = MTU
    - HEADER_LEN
    - ANNOUNCE_PUBLIC_KEY_LEN
    - DOTTED_NAME_HASH_LEN
    - ANNOUNCE_ID_WIRE_LEN
    - SIGNATURE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkOutcome {
    Parked,
    /// Same destination was already held; the newer announce replaced it.
    Overwrote,
    /// Cap reached and the destination wasn't already held — dropped.
    CacheFull,
    /// `announce.app_data.len() > HELD_APP_DATA_LIMIT`. The cache is
    /// unchanged. Wire-valid announces never trigger this — the limit
    /// matches the wire-protocol max — but a malformed-but-parsed announce
    /// could theoretically.
    AppDataTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldReason {
    /// The routing-table `app_data` arena was full at upsert time, so the
    /// announce was never installed. Release condition: any subsequent
    /// tick (the arena may have cleared). Follow-up: retry
    /// [`upsert_route`](crate::routing::RoutingTable::upsert_route); on
    /// success the destination installs into the routing table for the
    /// first time (pre-install hold, mirroring RNS Path A semantics).
    #[default]
    RoutingArenaPressure,
    // Future variants when their slices land:
    // IngressBurst — interface ingress-rate burst; release on cooldown,
    //                follow-up: re-inject via ingest.
}

/// An owned snapshot of one held entry, returned by
/// [`HeldAnnouncesCache::take_next`]. Borrows nothing back at the cache —
/// callers can drive `upsert_route`, `Announce::to_wire`, or whatever the
/// hold's reason calls for without lifetime tangles.
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
    /// The held announce as an [`Announce`] borrowing this entry's own
    /// `app_data` buffer. Use it to re-run the acceptance predicate or to
    /// serialize back to wire via [`Announce::to_wire`].
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

    /// Interface this announce was originally delivered on. Threads
    /// through to emission so the engine can apply RNS's "don't gossip
    /// back to source" rule when a held entry recovers and gets
    /// re-emitted.
    pub fn source_interface(&self) -> InterfaceId {
        self.source_interface
    }
}

/// Fixed-capacity SoA store of held announces. Hot scan columns
/// (destinations, received_hops, arrived_at) are laid out separately from
/// cold per-row content so park dedup and take_next selection walk dense
/// arrays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldAnnouncesCache<const CAPACITY: usize = DEFAULT_HELD_CACHE_CAPACITY> {
    len: usize,
    // Hot-scan columns.
    destinations: [DestinationHash; CAPACITY],
    received_hops: [u8; CAPACITY],
    arrived_at: [InstantMillis; CAPACITY],
    // Cold per-row content.
    public_keys: [IdentityPublicKeys; CAPACITY],
    dotted_name_hash: [DottedNameHash; CAPACITY],
    announce_id: [AnnounceId; CAPACITY],
    maybe_ratchet: [Option<RatchetKey>; CAPACITY],
    signature: [Ed25519Signature; CAPACITY],
    app_data_buf: [[u8; HELD_APP_DATA_LIMIT]; CAPACITY],
    app_data_len: [u16; CAPACITY],
    reason: [HoldReason; CAPACITY],
    source_interface: [InterfaceId; CAPACITY],
}

impl<const CAPACITY: usize> Default for HeldAnnouncesCache<CAPACITY> {
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); CAPACITY],
            received_hops: [0u8; CAPACITY],
            arrived_at: [InstantMillis(0); CAPACITY],
            public_keys: [IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            }; CAPACITY],
            dotted_name_hash: [DottedNameHash::new([0u8; 10]); CAPACITY],
            announce_id: [AnnounceId::from_wire([0u8; 10]); CAPACITY],
            maybe_ratchet: [None; CAPACITY],
            signature: [Ed25519Signature([0u8; 64]); CAPACITY],
            app_data_buf: [[0u8; HELD_APP_DATA_LIMIT]; CAPACITY],
            app_data_len: [0u16; CAPACITY],
            reason: [HoldReason::RoutingArenaPressure; CAPACITY],
            source_interface: [InterfaceId::new([0u8; 16]); CAPACITY],
        }
    }
}

impl<const CAPACITY: usize> HeldAnnouncesCache<CAPACITY> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    /// Park an announce with the lifecycle reason it's being held for.
    /// Destination-keyed: if an entry for the same destination already
    /// exists, it is overwritten (newer supersedes, including the
    /// reason). Otherwise pushes a new entry; if the cap is reached the
    /// new announce is dropped (`CacheFull`) — same as RNS's
    /// `Interface.hold_announce`.
    pub fn park(
        &mut self,
        announce: &Announce<'_>,
        arrived_at: InstantMillis,
        received_hops: u8,
        reason: HoldReason,
        source_interface: InterfaceId,
    ) -> ParkOutcome {
        if announce.app_data.len() > HELD_APP_DATA_LIMIT {
            return ParkOutcome::AppDataTooLarge;
        }

        // Destination-keyed dedup: same destination overwrites in place.
        for i in 0..self.len {
            if self.destinations[i] == announce.destination {
                self.write_at(
                    i,
                    announce,
                    arrived_at,
                    received_hops,
                    reason,
                    source_interface,
                );
                return ParkOutcome::Overwrote;
            }
        }

        if self.len >= CAPACITY {
            return ParkOutcome::CacheFull;
        }
        let i = self.len;
        self.write_at(
            i,
            announce,
            arrived_at,
            received_hops,
            reason,
            source_interface,
        );
        self.len += 1;
        ParkOutcome::Parked
    }

    /// Take the next entry to act on, removing it from the cache. Selects
    /// the lowest `received_hops` (best reach), ties broken by oldest
    /// `arrived_at`. Returns `None` if the cache is empty. Mirrors RNS's
    /// `process_held_announces` selection.
    pub fn take_next(&mut self) -> Option<HeldAnnounce> {
        if self.len == 0 {
            return None;
        }

        // SoA hot-column scan for min (hops, arrived_at).
        let mut best_idx = 0;
        for i in 1..self.len {
            let cur_key = (self.received_hops[i], self.arrived_at[i].0);
            let best_key = (self.received_hops[best_idx], self.arrived_at[best_idx].0);
            if cur_key < best_key {
                best_idx = i;
            }
        }

        let held = HeldAnnounce {
            reason: self.reason[best_idx],
            destination: self.destinations[best_idx],
            public_keys: self.public_keys[best_idx],
            dotted_name_hash: self.dotted_name_hash[best_idx],
            announce_id: self.announce_id[best_idx],
            maybe_ratchet: self.maybe_ratchet[best_idx],
            signature: self.signature[best_idx],
            app_data_buf: self.app_data_buf[best_idx],
            app_data_len: self.app_data_len[best_idx],
            arrived_at: self.arrived_at[best_idx],
            received_hops: self.received_hops[best_idx],
            source_interface: self.source_interface[best_idx],
        };

        self.swap_remove_at(best_idx);
        Some(held)
    }

    fn write_at(
        &mut self,
        i: usize,
        announce: &Announce<'_>,
        arrived_at: InstantMillis,
        received_hops: u8,
        reason: HoldReason,
        source_interface: InterfaceId,
    ) {
        self.destinations[i] = announce.destination;
        self.received_hops[i] = received_hops;
        self.arrived_at[i] = arrived_at;
        self.public_keys[i] = announce.public_keys;
        self.dotted_name_hash[i] = announce.dotted_name_hash;
        self.announce_id[i] = announce.announce_id;
        self.maybe_ratchet[i] = announce.maybe_ratchet;
        self.signature[i] = announce.signature;
        let len = announce.app_data.len();
        self.app_data_buf[i][..len].copy_from_slice(announce.app_data);
        // Zero the tail past `len` so PartialEq stays "identical
        // representation" determinism-style across parks of different
        // lengths into the same slot.
        for byte in self.app_data_buf[i][len..].iter_mut() {
            *byte = 0;
        }
        self.app_data_len[i] = len as u16;
        self.reason[i] = reason;
        self.source_interface[i] = source_interface;
    }

    fn swap_remove_at(&mut self, i: usize) {
        let last = self.len - 1;
        if i != last {
            self.destinations[i] = self.destinations[last];
            self.received_hops[i] = self.received_hops[last];
            self.arrived_at[i] = self.arrived_at[last];
            self.public_keys[i] = self.public_keys[last];
            self.dotted_name_hash[i] = self.dotted_name_hash[last];
            self.announce_id[i] = self.announce_id[last];
            self.maybe_ratchet[i] = self.maybe_ratchet[last];
            self.signature[i] = self.signature[last];
            self.app_data_buf[i] = self.app_data_buf[last];
            self.app_data_len[i] = self.app_data_len[last];
            self.reason[i] = self.reason[last];
            self.source_interface[i] = self.source_interface[last];
        }
        self.len = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(n: u64) -> InstantMillis {
        InstantMillis(n)
    }

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn announce_for<'a>(destination: DestinationHash, app_data: &'a [u8]) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: AnnounceId::from_wire([0u8; 10]),
            maybe_ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    #[test]
    fn empty_cache_takes_nothing() {
        let mut cache: HeldAnnouncesCache<4> = HeldAnnouncesCache::new();
        assert!(cache.is_empty());
        assert!(cache.take_next().is_none());
    }

    #[test]
    fn park_stores_announce_fields_and_round_trips_via_announce_accessor() {
        let mut cache: HeldAnnouncesCache<4> = HeldAnnouncesCache::new();
        let app_data = b"hello-personal";
        let a = announce_for(dest(1), app_data);
        let source = InterfaceId::new([0xA5; 16]);
        assert_eq!(
            cache.park(&a, ts(100), 3, HoldReason::RoutingArenaPressure, source),
            ParkOutcome::Parked
        );
        assert_eq!(cache.len(), 1);

        let held = cache.take_next().unwrap();
        let recovered = held.announce();
        assert_eq!(recovered.destination, dest(1));
        assert_eq!(recovered.app_data, app_data);
        assert_eq!(held.arrived_at(), ts(100));
        assert_eq!(held.received_hops(), 3);
        assert_eq!(held.reason(), HoldReason::RoutingArenaPressure);
        assert_eq!(held.source_interface(), source);
    }

    #[test]
    fn parking_same_destination_overwrites_in_place() {
        let mut cache: HeldAnnouncesCache<4> = HeldAnnouncesCache::new();
        let first = announce_for(dest(1), b"old");
        let second = announce_for(dest(1), b"new");
        let first_source = InterfaceId::new([0x01; 16]);
        let second_source = InterfaceId::new([0x02; 16]);

        assert_eq!(
            cache.park(
                &first,
                ts(100),
                5,
                HoldReason::RoutingArenaPressure,
                first_source
            ),
            ParkOutcome::Parked
        );
        assert_eq!(
            cache.park(
                &second,
                ts(200),
                2,
                HoldReason::RoutingArenaPressure,
                second_source
            ),
            ParkOutcome::Overwrote
        );
        assert_eq!(cache.len(), 1);

        let held = cache.take_next().unwrap();
        assert_eq!(held.announce().app_data, b"new");
        assert_eq!(held.arrived_at(), ts(200));
        assert_eq!(held.received_hops(), 2);
        assert_eq!(held.source_interface(), second_source);
    }

    #[test]
    fn park_past_capacity_drops_the_new_announce() {
        // RNS pattern: no LRU eviction; the new one is silently dropped.
        let mut cache: HeldAnnouncesCache<2> = HeldAnnouncesCache::new();
        assert_eq!(
            cache.park(
                &announce_for(dest(1), b"a"),
                ts(100),
                3,
                HoldReason::RoutingArenaPressure,
                InterfaceId::new([0u8; 16])
            ),
            ParkOutcome::Parked
        );
        assert_eq!(
            cache.park(
                &announce_for(dest(2), b"b"),
                ts(200),
                3,
                HoldReason::RoutingArenaPressure,
                InterfaceId::new([0u8; 16])
            ),
            ParkOutcome::Parked
        );
        assert_eq!(
            cache.park(
                &announce_for(dest(3), b"c"),
                ts(300),
                3,
                HoldReason::RoutingArenaPressure,
                InterfaceId::new([0u8; 16])
            ),
            ParkOutcome::CacheFull
        );
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn take_next_picks_the_lowest_received_hops() {
        let mut cache: HeldAnnouncesCache<4> = HeldAnnouncesCache::new();
        cache.park(
            &announce_for(dest(1), b"far"),
            ts(100),
            10,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        );
        cache.park(
            &announce_for(dest(2), b"near"),
            ts(200),
            2,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        );
        cache.park(
            &announce_for(dest(3), b"medium"),
            ts(300),
            5,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        );

        let held = cache.take_next().unwrap();
        assert_eq!(held.received_hops(), 2);
        assert_eq!(held.announce().destination, dest(2));

        let next = cache.take_next().unwrap();
        assert_eq!(next.received_hops(), 5);

        let last = cache.take_next().unwrap();
        assert_eq!(last.received_hops(), 10);
    }

    #[test]
    fn ties_on_hops_are_broken_by_oldest_arrived_at() {
        let mut cache: HeldAnnouncesCache<4> = HeldAnnouncesCache::new();
        cache.park(
            &announce_for(dest(1), b"a"),
            ts(300),
            3,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        );
        cache.park(
            &announce_for(dest(2), b"b"),
            ts(100),
            3,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        ); // oldest arrival, same hops
        cache.park(
            &announce_for(dest(3), b"c"),
            ts(200),
            3,
            HoldReason::RoutingArenaPressure,
            InterfaceId::new([0u8; 16]),
        );

        let held = cache.take_next().unwrap();
        assert_eq!(held.arrived_at(), ts(100));
        assert_eq!(held.announce().destination, dest(2));
    }

    #[test]
    fn held_app_data_limit_matches_the_wire_protocol_max() {
        // Ratchet-less announce envelope. If the wire constants drift, this
        // catches the mismatch.
        assert_eq!(
            HELD_APP_DATA_LIMIT,
            MTU - HEADER_LEN
                - ANNOUNCE_PUBLIC_KEY_LEN
                - DOTTED_NAME_HASH_LEN
                - ANNOUNCE_ID_WIRE_LEN
                - SIGNATURE_LEN
        );
    }
}
