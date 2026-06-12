use alloc::vec::Vec;

use crate::routing::announce::emit::AnnounceAppDataBytes;
use crate::routing::announce::DottedNameHash;
use crate::routing::storage::ColumnsFull;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationColumns, UpstreamAppDestinationKind,
};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapUpstreamAppDestinationColumns {
    destination: Vec<DestinationHash>,
    kind: Vec<UpstreamAppDestinationKind>,
    name_hash: Vec<DottedNameHash>,
    app_data: Vec<AnnounceAppDataBytes>,
}

impl UpstreamAppDestinationColumns for HeapUpstreamAppDestinationColumns {
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

    fn kind_mut(&mut self, index: usize) -> &mut UpstreamAppDestinationKind {
        &mut self.kind[index]
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
        app_data: AnnounceAppDataBytes,
    ) -> Result<usize, ColumnsFull> {
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
    use crate::routing::links::resources::ResourceStrategy;
    use crate::identity::IdentityHash;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::{DOTTED_NAME_HASH_LEN, TRUNCATED_HASH_BYTE_LEN};

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut columns = HeapUpstreamAppDestinationColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);

        for n in 0..100u8 {
            let upserted = columns.upsert(
                DestinationHash::new([n; TRUNCATED_HASH_BYTE_LEN]),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([n; 16]),
                    proof_strategy: ProofStrategy::ProveNone,
                    resource_strategy: ResourceStrategy::AcceptNone,
                },
                DottedNameHash::new([n; DOTTED_NAME_HASH_LEN]),
                AnnounceAppDataBytes::new(),
            );
            assert_eq!(upserted, Ok(n as usize));
        }
        assert_eq!(columns.len(), 100);
        assert_eq!(columns.destinations().len(), 100);
        assert_eq!(columns.kinds().len(), 100);
        assert_eq!(columns.name_hashes().len(), 100);
    }
}
