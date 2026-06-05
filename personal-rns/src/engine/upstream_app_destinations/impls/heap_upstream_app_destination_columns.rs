use alloc::vec::Vec;

use crate::engine::upstream_app_destinations::{
    UpstreamAppDestinationColumns, UpstreamAppDestinationKind,
};
use crate::routing::announce::DottedNameHash;
use crate::routing::storage::ColumnsFull;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapUpstreamAppDestinationColumns {
    destination: Vec<DestinationHash>,
    kind: Vec<UpstreamAppDestinationKind>,
    name_hash: Vec<DottedNameHash>,
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

    fn push(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
    ) -> Result<usize, ColumnsFull> {
        let i = self.destination.len();
        self.destination.push(destination);
        self.kind.push(kind);
        self.name_hash.push(name_hash);
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{DOTTED_NAME_HASH_LEN, TRUNCATED_HASH_BYTE_LEN};

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut columns = HeapUpstreamAppDestinationColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);

        for n in 0..100u8 {
            let pushed = columns.push(
                DestinationHash::new([n; TRUNCATED_HASH_BYTE_LEN]),
                UpstreamAppDestinationKind::Single,
                DottedNameHash::new([n; DOTTED_NAME_HASH_LEN]),
            );
            assert_eq!(pushed, Ok(n as usize));
        }
        assert_eq!(columns.len(), 100);
        assert_eq!(columns.destinations().len(), 100);
        assert_eq!(columns.kinds().len(), 100);
        assert_eq!(columns.name_hashes().len(), 100);
    }
}
