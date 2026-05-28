//! Fixed-capacity, inline-array destination-table columns — the no_std default.
//!
//! Each column is a `[T; MAX_TRACKED_DESTINATIONS]` stored inline in the
//! struct (and therefore inline in whatever `RoutingTable` it lives in).
//! No allocator, no heap, no growth: footprint is known at compile time and
//! sized by the const generic. Capacity overflow surfaces as
//! [`ColumnsFull`](crate::routing::storage::ColumnsFull) at the `push` call site.

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::engine::InstantMillis;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::routing::storage::{AppDataHandle, ColumnsFull, RouteColumns, RouteEntry};
use crate::routing::RouteResponsiveness;
use crate::wire::DestinationHash;

/// SoA destination-table columns backed by inline fixed-size arrays. The
/// capacity is the const generic; reaching it returns `ColumnsFull` from
/// `push`.
///
/// `PartialEq` is structural — every slot compares, including unused tail
/// past `len`. Determinism tests rely on this exactly as `RoutingTable`
/// already does; it is not "same set of destinations."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayRouteColumns<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    expires: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [RouteResponsiveness; MAX_TRACKED_DESTINATIONS],
    public_keys: [IdentityPublicKeys; MAX_TRACKED_DESTINATIONS],
    dotted_name_hash: [DottedNameHash; MAX_TRACKED_DESTINATIONS],
    retained_announce_id: [AnnounceId; MAX_TRACKED_DESTINATIONS],
    ratchet: [Option<RatchetKey>; MAX_TRACKED_DESTINATIONS],
    signature: [Ed25519Signature; MAX_TRACKED_DESTINATIONS],
    app_data_handle: [Option<AppDataHandle>; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            expires: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [RouteResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            public_keys: [IdentityPublicKeys {
                encryption: X25519PublicKey([0u8; 32]),
                signing: Ed25519PublicKey([0u8; 32]),
            }; MAX_TRACKED_DESTINATIONS],
            dotted_name_hash: [DottedNameHash::new([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            retained_announce_id: [AnnounceId::from_wire([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            ratchet: [None; MAX_TRACKED_DESTINATIONS],
            signature: [Ed25519Signature([0u8; 64]); MAX_TRACKED_DESTINATIONS],
            app_data_handle: [None; MAX_TRACKED_DESTINATIONS],
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize> RouteColumns
    for FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>
{
    fn capacity(&self) -> usize {
        MAX_TRACKED_DESTINATIONS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination[..self.len]
    }
    fn hops(&self) -> &[u8] {
        &self.hops[..self.len]
    }
    fn expires(&self) -> &[InstantMillis] {
        &self.expires[..self.len]
    }
    fn responsiveness(&self) -> &[RouteResponsiveness] {
        &self.responsiveness[..self.len]
    }
    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys[..self.len]
    }
    fn dotted_name_hash(&self) -> &[DottedNameHash] {
        &self.dotted_name_hash[..self.len]
    }
    fn retained_announce_id(&self) -> &[AnnounceId] {
        &self.retained_announce_id[..self.len]
    }
    fn ratchet(&self) -> &[Option<RatchetKey>] {
        &self.ratchet[..self.len]
    }
    fn signature(&self) -> &[Ed25519Signature] {
        &self.signature[..self.len]
    }
    fn app_data_handle(&self) -> &[Option<AppDataHandle>] {
        &self.app_data_handle[..self.len]
    }

    fn set_row(&mut self, i: usize, row: RouteEntry) {
        self.hops[i] = row.hops;
        self.expires[i] = row.expires;
        self.responsiveness[i] = row.responsiveness;
        self.public_keys[i] = row.public_keys;
        self.dotted_name_hash[i] = row.dotted_name_hash;
        self.retained_announce_id[i] = row.retained_announce_id;
        self.ratchet[i] = row.maybe_ratchet;
        self.signature[i] = row.signature;
        self.app_data_handle[i] = row.maybe_app_data_handle;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        row: RouteEntry,
    ) -> Result<usize, ColumnsFull> {
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(ColumnsFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.set_row(i, row);
        self.len += 1;
        Ok(i)
    }
}
