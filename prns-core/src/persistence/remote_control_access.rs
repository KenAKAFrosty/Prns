use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::identity::{PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use crate::remote_control::RemoteControlControllerIdentity;

const ROW_COUNT_LEN: usize = 4;

pub const REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;

pub fn remote_control_access_snapshot_len(row_count: usize) -> usize {
    SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN + row_count * REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
}

pub fn write_remote_control_access_snapshot(
    rows: impl Iterator<Item = RemoteControlControllerIdentity>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut row_count: u32 = 0;
    for row in rows {
        if out.len() < at + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN {
            return Err(SnapshotSealError::BufferTooShort);
        }
        out[at..at + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN]
            .copy_from_slice(&row.public_keys().public_key_bytes());
        at += REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN;
        row_count += 1;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    seal_snapshot_in_place(
        SnapshotRegion::RemoteControlAccess,
        at - SNAPSHOT_HEADER_LEN,
        out,
    )
}

pub fn read_remote_control_access_snapshot(
    bytes: &[u8],
) -> Result<PersistedRemoteControlControllerIdentities<'_>, SnapshotReadError> {
    let payload = open_snapshot(SnapshotRegion::RemoteControlAccess, bytes)
        .map_err(SnapshotReadError::Envelope)?;
    let Some((row_count_bytes, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let row_count = u64::from(u32::from_le_bytes(*row_count_bytes));
    if rows.len() as u64 != row_count * REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN as u64 {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedRemoteControlControllerIdentities { rest: rows })
}

#[derive(Debug, Clone)]
pub struct PersistedRemoteControlControllerIdentities<'a> {
    rest: &'a [u8],
}

impl PersistedRemoteControlControllerIdentities<'_> {
    pub fn row_count(&self) -> usize {
        self.rest.len() / REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
    }
}

impl Iterator for PersistedRemoteControlControllerIdentities<'_> {
    type Item = RemoteControlControllerIdentity;

    fn next(&mut self) -> Option<Self::Item> {
        let (public_keys, rest) = self
            .rest
            .split_first_chunk::<REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN>()?;
        self.rest = rest;
        Some(RemoteControlControllerIdentity::new(
            PublicIdentityMaterial::from_bytes(*public_keys).public_keys(),
        ))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.row_count(), Some(self.row_count()))
    }
}

impl ExactSizeIterator for PersistedRemoteControlControllerIdentities<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::persistence::SnapshotOpenError;
    use crate::remote_control::{FixedRemoteControlAccessTable, RemoteControlAccessTable};
    use std::vec::Vec;

    fn identity(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    #[test]
    fn a_set_of_identities_round_trips() {
        let identities = [identity(0x21), identity(0x43), identity(0x65)];
        let mut out = std::vec![0u8; remote_control_access_snapshot_len(identities.len())];
        let len =
            write_remote_control_access_snapshot(identities.iter().copied(), &mut out).unwrap();

        let reader = read_remote_control_access_snapshot(&out[..len]).unwrap();
        assert_eq!(reader.row_count(), identities.len());
        assert_eq!(reader.collect::<Vec<_>>(), identities);
    }

    #[test]
    fn persisted_identities_restore_into_a_fixed_table() {
        let identities = [identity(0x87), identity(0xA9)];
        let mut out = std::vec![0u8; remote_control_access_snapshot_len(identities.len())];
        let len =
            write_remote_control_access_snapshot(identities.iter().copied(), &mut out).unwrap();
        let mut table = FixedRemoteControlAccessTable::<2>::default();

        for identity in read_remote_control_access_snapshot(&out[..len]).unwrap() {
            table.upsert(identity).unwrap();
        }

        assert_eq!(table.identities(), identities);
    }

    #[test]
    fn an_empty_table_round_trips_to_no_identities() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN];
        let len = write_remote_control_access_snapshot(core::iter::empty(), &mut out).unwrap();

        assert_eq!(
            read_remote_control_access_snapshot(&out[..len])
                .unwrap()
                .count(),
            0,
        );
    }

    #[test]
    fn malformed_row_counts_are_refused() {
        let mut payload =
            std::vec![0u8; ROW_COUNT_LEN + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN - 1];
        payload[..ROW_COUNT_LEN].copy_from_slice(&1u32.to_le_bytes());
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RemoteControlAccess,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert_eq!(
            read_remote_control_access_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::MalformedPayload),
        );
    }

    #[test]
    fn another_regions_snapshot_is_refused() {
        let payload = 0u32.to_le_bytes();
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RoutingTable,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert_eq!(
            read_remote_control_access_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::Envelope(
                SnapshotOpenError::WrongRegion {
                    found: SnapshotRegion::RoutingTable.tag(),
                },
            )),
        );
    }

    #[test]
    fn a_short_buffer_is_refused() {
        let identities = [identity(0xCB)];
        let mut short = std::vec![0u8; remote_control_access_snapshot_len(identities.len()) - 1];

        assert_eq!(
            write_remote_control_access_snapshot(identities.iter().copied(), &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }
}
