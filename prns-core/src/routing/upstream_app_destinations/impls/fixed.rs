use heapless::Vec as HeaplessVec;

use crate::routing::announce::emit::AnnounceAppDataBytes;
use crate::routing::announce::DottedNameHash;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationKind, UpstreamAppDestinationTable,
};
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, DOTTED_NAME_HASH_BYTE_LEN, TRUNCATED_HASH_BYTE_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedUpstreamAppDestinationTable<const MAX_UPSTREAM_APP_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_UPSTREAM_APP_DESTINATIONS],
    kind: [UpstreamAppDestinationKind; MAX_UPSTREAM_APP_DESTINATIONS],
    name_hash: [DottedNameHash; MAX_UPSTREAM_APP_DESTINATIONS],
    app_data: HeaplessVec<AnnounceAppDataBytes, MAX_UPSTREAM_APP_DESTINATIONS>,
}

impl<const MAX_UPSTREAM_APP_DESTINATIONS: usize> Default
    for FixedUpstreamAppDestinationTable<MAX_UPSTREAM_APP_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; TRUNCATED_HASH_BYTE_LEN]);
                MAX_UPSTREAM_APP_DESTINATIONS],
            kind: [UpstreamAppDestinationKind::Plain; MAX_UPSTREAM_APP_DESTINATIONS],
            name_hash: [DottedNameHash::new([0u8; DOTTED_NAME_HASH_BYTE_LEN]);
                MAX_UPSTREAM_APP_DESTINATIONS],
            app_data: HeaplessVec::new(),
        }
    }
}

impl<const MAX_UPSTREAM_APP_DESTINATIONS: usize> UpstreamAppDestinationTable
    for FixedUpstreamAppDestinationTable<MAX_UPSTREAM_APP_DESTINATIONS>
{
    fn capacity(&self) -> usize {
        MAX_UPSTREAM_APP_DESTINATIONS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination[..self.len]
    }
    fn kinds(&self) -> &[UpstreamAppDestinationKind] {
        &self.kind[..self.len]
    }
    fn name_hashes(&self) -> &[DottedNameHash] {
        &self.name_hash[..self.len]
    }
    fn app_data_at(&self, index: usize) -> Option<&[u8]> {
        self.app_data.get(index).map(|data| data.as_slice())
    }
    fn app_data_at_mut(&mut self, index: usize) -> Option<&mut AnnounceAppDataBytes> {
        self.app_data.get_mut(index)
    }

    fn kind_mut(&mut self, index: usize) -> &mut UpstreamAppDestinationKind {
        &mut self.kind[index]
    }

    fn swap_remove(&mut self, index: usize) -> Option<AnnounceAppDataBytes> {
        let last = self.len.checked_sub(1)?;
        if index > last || index >= self.app_data.len() {
            return None;
        }
        self.destination.swap(index, last);
        self.kind.swap(index, last);
        self.name_hash.swap(index, last);
        let app_data = self.app_data.swap_remove(index);
        self.destination[last] = DestinationHash::new([0u8; TRUNCATED_HASH_BYTE_LEN]);
        self.kind[last] = UpstreamAppDestinationKind::Plain;
        self.name_hash[last] = DottedNameHash::new([0u8; DOTTED_NAME_HASH_BYTE_LEN]);
        self.len = last;
        Some(app_data)
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
        app_data: AnnounceAppDataBytes,
    ) -> Result<usize, TablePushError> {
        if let Some(i) = self.destination[..self.len]
            .iter()
            .position(|candidate| *candidate == destination)
        {
            self.kind[i] = kind;
            self.name_hash[i] = name_hash;
            self.app_data[i] = app_data;
            return Ok(i);
        }
        if self.len >= MAX_UPSTREAM_APP_DESTINATIONS {
            return Err(TablePushError::TableFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.kind[i] = kind;
        self.name_hash[i] = name_hash;
        let _ = self.app_data.push(app_data);
        self.len += 1;
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::RatchetPolicy;
    use crate::identity::IdentityHash;
    use crate::routing::links::resources::ResourceStrategy;
    use crate::routing::upstream_app_destinations::LinkRequestPolicy;
    use crate::routing::upstream_app_destinations::ProofStrategy;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; TRUNCATED_HASH_BYTE_LEN])
    }
    fn name(byte: u8) -> DottedNameHash {
        DottedNameHash::new([byte; DOTTED_NAME_HASH_BYTE_LEN])
    }

    #[test]
    fn exposes_only_upserted_rows_and_reports_a_full_table() {
        let mut table = FixedUpstreamAppDestinationTable::<2>::default();
        assert_eq!(table.capacity(), 2);
        assert!(table.is_empty());
        assert!(table.destinations().is_empty());

        assert_eq!(
            table.upsert(
                dest(1),
                UpstreamAppDestinationKind::Plain,
                name(1),
                AnnounceAppDataBytes::new()
            ),
            Ok(0)
        );
        assert_eq!(
            table.upsert(
                dest(2),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([2; 16]),
                    proof_strategy: ProofStrategy::ProveAll,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                    ratchet_policy: RatchetPolicy::NoRatchets,
                },
                name(2),
                AnnounceAppDataBytes::new()
            ),
            Ok(1)
        );
        assert_eq!(
            table.upsert(
                dest(3),
                UpstreamAppDestinationKind::Plain,
                name(3),
                AnnounceAppDataBytes::new()
            ),
            Err(TablePushError::TableFull)
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(1), dest(2)]);
        assert_eq!(
            table.kinds(),
            &[
                UpstreamAppDestinationKind::Plain,
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([2; 16]),
                    proof_strategy: ProofStrategy::ProveAll,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                    ratchet_policy: RatchetPolicy::NoRatchets,
                }
            ]
        );
        assert_eq!(table.name_hashes(), &[name(1), name(2)]);
    }

    #[test]
    fn upserting_a_known_destination_overwrites_its_row_in_place() {
        let mut table = FixedUpstreamAppDestinationTable::<2>::default();
        table
            .upsert(
                dest(1),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([1; 16]),
                    proof_strategy: ProofStrategy::ProveNone,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                    ratchet_policy: RatchetPolicy::NoRatchets,
                },
                name(1),
                AnnounceAppDataBytes::new(),
            )
            .unwrap();
        assert_eq!(
            table.upsert(
                dest(1),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([1; 16]),
                    proof_strategy: ProofStrategy::ProveAll,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                    ratchet_policy: RatchetPolicy::NoRatchets,
                },
                name(1),
                AnnounceAppDataBytes::from_slice(b"new").unwrap(),
            ),
            Ok(0),
            "a known destination keeps its slot",
        );

        assert_eq!(table.len(), 1);
        assert_eq!(
            table.kinds(),
            &[UpstreamAppDestinationKind::Single {
                identity: IdentityHash::new([1; 16]),
                proof_strategy: ProofStrategy::ProveAll,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::NoRatchets,
            }],
        );
        assert_eq!(table.app_data_at(0), Some(b"new".as_slice()));
    }

    #[test]
    fn removing_a_row_keeps_every_column_aligned_and_reclaims_capacity() {
        let mut table = FixedUpstreamAppDestinationTable::<2>::default();
        table
            .upsert(
                dest(1),
                UpstreamAppDestinationKind::Plain,
                name(1),
                AnnounceAppDataBytes::from_slice(b"first").unwrap(),
            )
            .unwrap();
        table
            .upsert(
                dest(2),
                UpstreamAppDestinationKind::Group,
                name(2),
                AnnounceAppDataBytes::from_slice(b"second").unwrap(),
            )
            .unwrap();

        assert!(table.swap_remove(2).is_none());
        assert_eq!(table.destinations(), &[dest(1), dest(2)]);
        let removed = table.swap_remove(0);
        assert_eq!(
            removed.as_ref().map(|data| data.as_slice()),
            Some(b"first".as_slice())
        );

        assert_eq!(table.len(), 1);
        assert_eq!(table.destinations(), &[dest(2)]);
        assert_eq!(table.kinds(), &[UpstreamAppDestinationKind::Group]);
        assert_eq!(table.name_hashes(), &[name(2)]);
        assert_eq!(table.app_data_at(0), Some(b"second".as_slice()));
        assert_eq!(
            table.upsert(
                dest(3),
                UpstreamAppDestinationKind::Plain,
                name(3),
                AnnounceAppDataBytes::from_slice(b"third").unwrap(),
            ),
            Ok(1),
        );
    }
}
