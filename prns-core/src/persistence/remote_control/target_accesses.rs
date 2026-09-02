use super::super::envelope::SnapshotSealError;
use super::super::{SnapshotReadError, SnapshotRegion};
use super::rows::{
    read_remote_control_authorization_rows, remote_control_authorization_snapshot_capacity,
    write_remote_control_authorization_rows, PersistedRemoteControlAuthorizationRows,
    RemoteControlAuthorizationRow,
};
use crate::remote_control::{
    RemoteControlTargetAccess, RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
};

pub const fn remote_control_target_accesses_snapshot_capacity(access_count: usize) -> usize {
    remote_control_authorization_snapshot_capacity(access_count)
}

pub fn write_remote_control_target_accesses_snapshot(
    accesses: &impl RemoteControlTargetAccessTable,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    write_remote_control_authorization_rows(
        SnapshotRegion::RemoteControlTargetAccesses,
        accesses.len(),
        accesses
            .accesses_in_identity_hash_order()
            .iter()
            .map(|access| RemoteControlAuthorizationRow {
                public_keys: access.target().public_keys(),
                permitted_requests: access.permitted_requests(),
            }),
        out,
    )
}

pub fn read_remote_control_target_accesses_snapshot(
    bytes: &[u8],
) -> Result<PersistedRemoteControlTargetAccesses<'_>, SnapshotReadError> {
    let rows =
        read_remote_control_authorization_rows(SnapshotRegion::RemoteControlTargetAccesses, bytes)?;
    Ok(PersistedRemoteControlTargetAccesses { rows })
}

#[derive(Debug, Clone)]
pub struct PersistedRemoteControlTargetAccesses<'a> {
    rows: PersistedRemoteControlAuthorizationRows<'a>,
}

impl PersistedRemoteControlTargetAccesses<'_> {
    pub const fn access_count(&self) -> usize {
        self.rows.len()
    }
}

impl Iterator for PersistedRemoteControlTargetAccesses<'_> {
    type Item = RemoteControlTargetAccess;

    fn next(&mut self) -> Option<Self::Item> {
        let row = self.rows.next()?;
        RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(row.public_keys),
            row.permitted_requests,
        )
        .ok()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.rows.size_hint()
    }
}

impl ExactSizeIterator for PersistedRemoteControlTargetAccesses<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::persistence::{SnapshotOpenError, SNAPSHOT_OVERHEAD_LEN};
    use crate::remote_control::{
        FixedRemoteControlTargetAccessTable, RemoteControlRequestKind, RemoteControlRequestSet,
        SetRemoteControlTargetAccessOutcome,
    };
    use proptest::prelude::*;
    use std::vec::Vec;

    fn identity(fill: u8) -> RemoteControlTargetIdentity {
        RemoteControlTargetIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn access(fill: u8, permitted_requests: RemoteControlRequestSet) -> RemoteControlTargetAccess {
        RemoteControlTargetAccess::new(identity(fill), permitted_requests).unwrap()
    }

    fn table<const N: usize>(
        accesses: [RemoteControlTargetAccess; N],
    ) -> FixedRemoteControlTargetAccessTable<N> {
        let mut table = FixedRemoteControlTargetAccessTable::default();
        for access in accesses {
            assert!(matches!(
                table.set_target_access(access),
                Ok(SetRemoteControlTargetAccessOutcome::Added)
            ));
        }
        table
    }

    #[test]
    fn target_accesses_round_trip_and_restore_into_a_fixed_table() {
        let accesses = table([
            access(
                0x21,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
            access(
                0x43,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
            access(0x65, RemoteControlRequestSet::all()),
        ]);
        let mut out = std::vec![
            0u8;
            remote_control_target_accesses_snapshot_capacity(accesses.len())
        ];
        let len = write_remote_control_target_accesses_snapshot(&accesses, &mut out).unwrap();
        let persisted = read_remote_control_target_accesses_snapshot(&out[..len]).unwrap();
        assert_eq!(persisted.access_count(), accesses.len());

        let mut restored = FixedRemoteControlTargetAccessTable::<3>::default();
        for access in persisted {
            assert!(restored.set_target_access(access).is_ok());
        }
        assert_eq!(
            restored.accesses_in_identity_hash_order(),
            accesses.accesses_in_identity_hash_order()
        );
    }

    #[test]
    fn empty_target_accesses_round_trip() {
        let accesses = FixedRemoteControlTargetAccessTable::<0>::default();
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + super::super::rows::ROW_COUNT_LEN];
        let len = write_remote_control_target_accesses_snapshot(&accesses, &mut out).unwrap();

        assert_eq!(
            read_remote_control_target_accesses_snapshot(&out[..len])
                .unwrap()
                .count(),
            0,
        );
    }

    #[test]
    fn insertion_order_does_not_change_target_access_snapshots() {
        let forward = table([
            access(
                0x87,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
            access(
                0xA9,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
        ]);
        let reverse = table([
            access(
                0xA9,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
            access(
                0x87,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
        ]);
        let capacity = remote_control_target_accesses_snapshot_capacity(2);
        let mut forward_bytes = std::vec![0u8; capacity];
        let mut reverse_bytes = std::vec![0u8; capacity];
        let forward_len =
            write_remote_control_target_accesses_snapshot(&forward, &mut forward_bytes).unwrap();
        let reverse_len =
            write_remote_control_target_accesses_snapshot(&reverse, &mut reverse_bytes).unwrap();

        assert_eq!(&forward_bytes[..forward_len], &reverse_bytes[..reverse_len],);
    }

    #[test]
    fn controller_grant_snapshots_are_refused_as_target_accesses() {
        let payload = 0u32.to_le_bytes();
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = crate::persistence::seal_snapshot(
            SnapshotRegion::RemoteControlControllerGrants,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert_eq!(
            read_remote_control_target_accesses_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::Envelope(
                SnapshotOpenError::WrongRegion {
                    found: SnapshotRegion::RemoteControlControllerGrants.tag(),
                },
            )),
        );
    }

    #[test]
    fn a_short_target_access_buffer_is_refused() {
        let accesses = table([access(0xCB, RemoteControlRequestSet::all())]);
        let mut short = std::vec![
            0u8;
            remote_control_target_accesses_snapshot_capacity(accesses.len()) - 1
        ];

        assert_eq!(
            write_remote_control_target_accesses_snapshot(&accesses, &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_are_total_for_target_access_reads(bytes in any::<Vec<u8>>()) {
            let _ = read_remote_control_target_accesses_snapshot(&bytes);
        }
    }
}
