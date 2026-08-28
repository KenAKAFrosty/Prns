use alloc::vec::Vec;

use crate::routing::announce::emit::AnnounceAppDataBytes;
use crate::routing::announce::DottedNameHash;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationKind, UpstreamAppDestinationTable,
};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapUpstreamAppDestinationTable {
    destination: Vec<DestinationHash>,
    kind: Vec<UpstreamAppDestinationKind>,
    name_hash: Vec<DottedNameHash>,
    app_data: Vec<AnnounceAppDataBytes>,
}

impl UpstreamAppDestinationTable for HeapUpstreamAppDestinationTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destination.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination
    }
    fn kinds(&self) -> &[UpstreamAppDestinationKind] {
        &self.kind
    }
    fn name_hashes(&self) -> &[DottedNameHash] {
        &self.name_hash
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
        if index >= self.destination.len()
            || index >= self.kind.len()
            || index >= self.name_hash.len()
            || index >= self.app_data.len()
        {
            return None;
        }
        self.destination.swap_remove(index);
        self.kind.swap_remove(index);
        self.name_hash.swap_remove(index);
        Some(self.app_data.swap_remove(index))
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
        app_data: AnnounceAppDataBytes,
    ) -> Result<usize, TablePushError> {
        if let Some(i) = self
            .destination
            .iter()
            .position(|candidate| *candidate == destination)
        {
            self.kind[i] = kind;
            self.name_hash[i] = name_hash;
            self.app_data[i] = app_data;
            return Ok(i);
        }
        let i = self.destination.len();
        self.destination.push(destination);
        self.kind.push(kind);
        self.name_hash.push(name_hash);
        self.app_data.push(app_data);
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
    use crate::wire::{DOTTED_NAME_HASH_BYTE_LEN, TRUNCATED_HASH_BYTE_LEN};

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut table = HeapUpstreamAppDestinationTable::default();
        assert_eq!(table.capacity(), usize::MAX);

        for n in 0..100u8 {
            let upserted = table.upsert(
                DestinationHash::new([n; TRUNCATED_HASH_BYTE_LEN]),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([n; 16]),
                    proof_strategy: ProofStrategy::ProveNone,
                    link_request_policy: LinkRequestPolicy::AcceptAll,
                    resource_strategy: ResourceStrategy::AcceptNone,
                    maximum_request_bytes: crate::units::ByteLimit::Unlimited,
                    ratchet_policy: RatchetPolicy::NoRatchets,
                },
                DottedNameHash::new([n; DOTTED_NAME_HASH_BYTE_LEN]),
                AnnounceAppDataBytes::new(),
            );
            assert_eq!(upserted, Ok(n as usize));
        }
        assert_eq!(table.len(), 100);
        assert_eq!(table.destinations().len(), 100);
        assert_eq!(table.kinds().len(), 100);
        assert_eq!(table.name_hashes().len(), 100);
    }

    #[test]
    fn removing_a_row_keeps_every_heap_column_aligned() {
        let mut table = HeapUpstreamAppDestinationTable::default();
        for n in 1..=3u8 {
            table
                .upsert(
                    DestinationHash::new([n; TRUNCATED_HASH_BYTE_LEN]),
                    UpstreamAppDestinationKind::Plain,
                    DottedNameHash::new([n; DOTTED_NAME_HASH_BYTE_LEN]),
                    AnnounceAppDataBytes::from_slice(&[n]).unwrap(),
                )
                .unwrap();
        }

        assert!(table.swap_remove(3).is_none());
        assert_eq!(table.len(), 3);
        let removed = table.swap_remove(1);

        assert_eq!(
            removed.as_ref().map(|data| data.as_slice()),
            Some([2].as_slice())
        );
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.destinations(),
            &[
                DestinationHash::new([1; TRUNCATED_HASH_BYTE_LEN]),
                DestinationHash::new([3; TRUNCATED_HASH_BYTE_LEN]),
            ],
        );
        assert_eq!(
            table.name_hashes(),
            &[
                DottedNameHash::new([1; DOTTED_NAME_HASH_BYTE_LEN]),
                DottedNameHash::new([3; DOTTED_NAME_HASH_BYTE_LEN]),
            ],
        );
        assert_eq!(table.app_data_at(0), Some([1].as_slice()));
        assert_eq!(table.app_data_at(1), Some([3].as_slice()));
    }
}
