use super::super::envelope::SnapshotSealError;
use super::super::{SnapshotReadError, SnapshotRegion};
use super::rows::{
    read_remote_control_authorization_rows, remote_control_authorization_snapshot_capacity,
    write_remote_control_authorization_rows, PersistedRemoteControlAuthorizationRows,
    RemoteControlAuthorizationRow,
};
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerGrantTable,
    RemoteControlControllerIdentity,
};

pub const fn remote_control_controller_grants_snapshot_capacity(grant_count: usize) -> usize {
    remote_control_authorization_snapshot_capacity(grant_count)
}

pub fn write_remote_control_controller_grants_snapshot(
    grants: &impl RemoteControlControllerGrantTable,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    write_remote_control_authorization_rows(
        SnapshotRegion::RemoteControlControllerGrants,
        grants.len(),
        grants
            .grants_in_identity_hash_order()
            .iter()
            .map(|grant| RemoteControlAuthorizationRow {
                public_keys: grant.controller().public_keys(),
                permitted_requests: grant.permitted_requests(),
            }),
        out,
    )
}

pub fn read_remote_control_controller_grants_snapshot(
    bytes: &[u8],
) -> Result<PersistedRemoteControlControllerGrants<'_>, SnapshotReadError> {
    let rows = read_remote_control_authorization_rows(
        SnapshotRegion::RemoteControlControllerGrants,
        bytes,
    )?;
    Ok(PersistedRemoteControlControllerGrants { rows })
}

#[derive(Debug, Clone)]
pub struct PersistedRemoteControlControllerGrants<'a> {
    rows: PersistedRemoteControlAuthorizationRows<'a>,
}

impl PersistedRemoteControlControllerGrants<'_> {
    pub const fn grant_count(&self) -> usize {
        self.rows.len()
    }
}

impl Iterator for PersistedRemoteControlControllerGrants<'_> {
    type Item = RemoteControlControllerGrant;

    fn next(&mut self) -> Option<Self::Item> {
        let row = self.rows.next()?;
        RemoteControlControllerGrant::new(
            RemoteControlControllerIdentity::new(row.public_keys),
            row.permitted_requests,
        )
        .ok()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.rows.size_hint()
    }
}

impl ExactSizeIterator for PersistedRemoteControlControllerGrants<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::persistence::{SnapshotOpenError, SNAPSHOT_OVERHEAD_LEN};
    use crate::remote_control::{
        FixedRemoteControlControllerGrantTable, RemoteControlRequestKind, RemoteControlRequestSet,
        SetRemoteControlControllerGrantOutcome,
    };
    use proptest::prelude::*;
    use std::vec::Vec;

    fn identity(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn grant(
        fill: u8,
        permitted_requests: RemoteControlRequestSet,
    ) -> RemoteControlControllerGrant {
        RemoteControlControllerGrant::new(identity(fill), permitted_requests).unwrap()
    }

    fn table<const N: usize>(
        grants: [RemoteControlControllerGrant; N],
    ) -> FixedRemoteControlControllerGrantTable<N> {
        let mut table = FixedRemoteControlControllerGrantTable::default();
        for grant in grants {
            assert!(matches!(
                table.set_controller_grant(grant),
                Ok(SetRemoteControlControllerGrantOutcome::Added)
            ));
        }
        table
    }

    #[test]
    fn controller_grants_round_trip_and_restore_into_a_fixed_table() {
        let grants = table([
            grant(
                0x21,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
            grant(
                0x43,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
            grant(0x65, RemoteControlRequestSet::all()),
        ]);
        let mut out = std::vec![
            0u8;
            remote_control_controller_grants_snapshot_capacity(grants.len())
        ];
        let len = write_remote_control_controller_grants_snapshot(&grants, &mut out).unwrap();
        let persisted = read_remote_control_controller_grants_snapshot(&out[..len]).unwrap();
        assert_eq!(persisted.grant_count(), grants.len());

        let mut restored = FixedRemoteControlControllerGrantTable::<3>::default();
        for grant in persisted {
            assert!(restored.set_controller_grant(grant).is_ok());
        }
        assert_eq!(
            restored.grants_in_identity_hash_order(),
            grants.grants_in_identity_hash_order()
        );
    }

    #[test]
    fn empty_controller_grants_round_trip() {
        let grants = FixedRemoteControlControllerGrantTable::<0>::default();
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + super::super::rows::ROW_COUNT_LEN];
        let len = write_remote_control_controller_grants_snapshot(&grants, &mut out).unwrap();

        assert_eq!(
            read_remote_control_controller_grants_snapshot(&out[..len])
                .unwrap()
                .count(),
            0,
        );
    }

    #[test]
    fn insertion_order_does_not_change_controller_grant_snapshots() {
        let first = grant(
            0x87,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
        let second = grant(
            0xA9,
            RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
        );
        let forward = table([first, second]);
        let reverse = table([second, first]);
        let capacity = remote_control_controller_grants_snapshot_capacity(2);
        let mut forward_bytes = std::vec![0u8; capacity];
        let mut reverse_bytes = std::vec![0u8; capacity];
        let forward_len =
            write_remote_control_controller_grants_snapshot(&forward, &mut forward_bytes).unwrap();
        let reverse_len =
            write_remote_control_controller_grants_snapshot(&reverse, &mut reverse_bytes).unwrap();

        assert_eq!(&forward_bytes[..forward_len], &reverse_bytes[..reverse_len],);
    }

    #[test]
    fn malformed_controller_grant_rows_are_refused() {
        let mut payload = std::vec![0u8; super::super::rows::ROW_COUNT_LEN];
        payload[..super::super::rows::ROW_COUNT_LEN].copy_from_slice(&1u32.to_le_bytes());
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = crate::persistence::seal_snapshot(
            SnapshotRegion::RemoteControlControllerGrants,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert_eq!(
            read_remote_control_controller_grants_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::MalformedPayload),
        );
    }

    #[test]
    fn another_regions_snapshot_is_refused_as_controller_grants() {
        let payload = 0u32.to_le_bytes();
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = crate::persistence::seal_snapshot(
            SnapshotRegion::RemoteControlTargetAccesses,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert_eq!(
            read_remote_control_controller_grants_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::Envelope(
                SnapshotOpenError::WrongRegion {
                    found: SnapshotRegion::RemoteControlTargetAccesses.tag(),
                },
            )),
        );
    }

    #[test]
    fn a_short_controller_grant_buffer_is_refused() {
        let grants = table([grant(0xCB, RemoteControlRequestSet::all())]);
        let mut short = std::vec![
            0u8;
            remote_control_controller_grants_snapshot_capacity(grants.len()) - 1
        ];

        assert_eq!(
            write_remote_control_controller_grants_snapshot(&grants, &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_are_total_for_controller_grant_reads(bytes in any::<Vec<u8>>()) {
            let _ = read_remote_control_controller_grants_snapshot(&bytes);
        }
    }
}
