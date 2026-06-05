use crate::routing::announce::DottedNameHash;
use crate::routing::storage::ColumnsFull;
use crate::routing::upstream_app_destinations::{
    UpstreamAppDestinationColumns, UpstreamAppDestinationKind,
};
use crate::wire::{DestinationHash, DOTTED_NAME_HASH_LEN, TRUNCATED_HASH_BYTE_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedUpstreamAppDestinationColumns<const MAX_UPSTREAM_APP_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_UPSTREAM_APP_DESTINATIONS],
    kind: [UpstreamAppDestinationKind; MAX_UPSTREAM_APP_DESTINATIONS],
    name_hash: [DottedNameHash; MAX_UPSTREAM_APP_DESTINATIONS],
}

impl<const MAX_UPSTREAM_APP_DESTINATIONS: usize> Default
    for FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; TRUNCATED_HASH_BYTE_LEN]);
                MAX_UPSTREAM_APP_DESTINATIONS],
            kind: [UpstreamAppDestinationKind::Plain; MAX_UPSTREAM_APP_DESTINATIONS],
            name_hash: [DottedNameHash::new([0u8; DOTTED_NAME_HASH_LEN]);
                MAX_UPSTREAM_APP_DESTINATIONS],
        }
    }
}

impl<const MAX_UPSTREAM_APP_DESTINATIONS: usize> UpstreamAppDestinationColumns
    for FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>
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

    fn push(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
    ) -> Result<usize, ColumnsFull> {
        if self.len >= MAX_UPSTREAM_APP_DESTINATIONS {
            return Err(ColumnsFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.kind[i] = kind;
        self.name_hash[i] = name_hash;
        self.len += 1;
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityHash;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; TRUNCATED_HASH_BYTE_LEN])
    }
    fn name(byte: u8) -> DottedNameHash {
        DottedNameHash::new([byte; DOTTED_NAME_HASH_LEN])
    }

    #[test]
    fn exposes_only_pushed_rows_and_reports_a_full_table() {
        let mut columns = FixedUpstreamAppDestinationColumns::<2>::default();
        assert_eq!(columns.capacity(), 2);
        assert!(columns.is_empty());
        assert!(columns.destinations().is_empty());

        assert_eq!(
            columns.push(dest(1), UpstreamAppDestinationKind::Plain, name(1)),
            Ok(0)
        );
        assert_eq!(
            columns.push(
                dest(2),
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([2; 16])
                },
                name(2)
            ),
            Ok(1)
        );
        assert_eq!(
            columns.push(dest(3), UpstreamAppDestinationKind::Plain, name(3)),
            Err(ColumnsFull)
        );

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.destinations(), &[dest(1), dest(2)]);
        assert_eq!(
            columns.kinds(),
            &[
                UpstreamAppDestinationKind::Plain,
                UpstreamAppDestinationKind::Single {
                    identity: IdentityHash::new([2; 16])
                }
            ]
        );
        assert_eq!(columns.name_hashes(), &[name(1), name(2)]);
    }
}
