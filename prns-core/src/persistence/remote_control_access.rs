use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::identity::{PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlRequestKind,
    RemoteControlRequestSet,
};

const ROW_COUNT_LEN: usize = 4;
const REQUEST_COUNT_LEN: usize = 1;

pub const REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;
const MAX_REMOTE_CONTROL_CONTROLLER_GRANT_WIRE_LEN: usize =
    REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
        + REQUEST_COUNT_LEN
        + RemoteControlRequestKind::ALL.len();

pub const fn remote_control_access_snapshot_capacity(grant_count: usize) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + ROW_COUNT_LEN
        + grant_count * MAX_REMOTE_CONTROL_CONTROLLER_GRANT_WIRE_LEN
}

pub fn write_remote_control_access_snapshot(
    grants: impl Iterator<Item = RemoteControlControllerGrant>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut grant_count: u32 = 0;
    for grant in grants {
        let permitted_requests = grant.permitted_requests();
        let encoded_len = REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
            .saturating_add(REQUEST_COUNT_LEN)
            .saturating_add(permitted_requests.len());
        if out.len() < at.saturating_add(encoded_len) {
            return Err(SnapshotSealError::BufferTooShort);
        }
        out[at..at + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN]
            .copy_from_slice(&grant.controller().public_keys().public_key_bytes());
        at += REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN;
        let request_count_at = at;
        at += REQUEST_COUNT_LEN;
        let mut request_count = 0u8;
        for request in permitted_requests.iter() {
            out[at] = request.wire_value();
            at += 1;
            request_count = request_count.saturating_add(1);
        }
        out[request_count_at] = request_count;
        grant_count += 1;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&grant_count.to_le_bytes());
    seal_snapshot_in_place(
        SnapshotRegion::RemoteControlAccess,
        at - SNAPSHOT_HEADER_LEN,
        out,
    )
}

pub fn read_remote_control_access_snapshot(
    bytes: &[u8],
) -> Result<PersistedRemoteControlControllerGrants<'_>, SnapshotReadError> {
    let payload = open_snapshot(SnapshotRegion::RemoteControlAccess, bytes)
        .map_err(SnapshotReadError::Envelope)?;
    let Some((row_count_bytes, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let grant_count = usize::try_from(u32::from_le_bytes(*row_count_bytes))
        .map_err(|_| SnapshotReadError::MalformedPayload)?;
    let mut rest = rows;
    for _ in 0..grant_count {
        let Some((_, remaining)) = parse_grant(rest) else {
            return Err(SnapshotReadError::MalformedPayload);
        };
        rest = remaining;
    }
    if !rest.is_empty() {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedRemoteControlControllerGrants {
        rest: rows,
        grant_count,
    })
}

#[derive(Debug, Clone)]
pub struct PersistedRemoteControlControllerGrants<'a> {
    rest: &'a [u8],
    grant_count: usize,
}

impl PersistedRemoteControlControllerGrants<'_> {
    pub const fn grant_count(&self) -> usize {
        self.grant_count
    }
}

impl Iterator for PersistedRemoteControlControllerGrants<'_> {
    type Item = RemoteControlControllerGrant;

    fn next(&mut self) -> Option<Self::Item> {
        let (grant, rest) = parse_grant(self.rest)?;
        self.rest = rest;
        self.grant_count = self.grant_count.saturating_sub(1);
        Some(grant)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.grant_count, Some(self.grant_count))
    }
}

impl ExactSizeIterator for PersistedRemoteControlControllerGrants<'_> {}

fn parse_grant(bytes: &[u8]) -> Option<(RemoteControlControllerGrant, &[u8])> {
    let (public_keys, rest) =
        bytes.split_first_chunk::<REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN>()?;
    let (request_count, rest) = rest.split_first()?;
    let (requests, rest) = rest.split_at_checked(usize::from(*request_count))?;
    let mut permitted_requests = RemoteControlRequestSet::empty();
    let mut previous = None;
    for request in requests {
        if previous.is_some_and(|previous| previous >= *request) {
            return None;
        }
        let request = RemoteControlRequestKind::from_wire(*request)?;
        if !permitted_requests.insert(request) {
            return None;
        }
        previous = Some(request.wire_value());
    }
    let controller = RemoteControlControllerIdentity::new(
        PublicIdentityMaterial::from_bytes(*public_keys).public_keys(),
    );
    let grant = RemoteControlControllerGrant::new(controller, permitted_requests).ok()?;
    Some((grant, rest))
}

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

    fn grant(
        fill: u8,
        permitted_requests: RemoteControlRequestSet,
    ) -> RemoteControlControllerGrant {
        RemoteControlControllerGrant::new(identity(fill), permitted_requests).unwrap()
    }

    #[test]
    fn a_set_of_controller_grants_round_trips() {
        let grants = [
            grant(
                0x21,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
            grant(
                0x43,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
            grant(0x65, RemoteControlRequestSet::all()),
        ];
        let mut out = std::vec![0u8; remote_control_access_snapshot_capacity(grants.len())];
        let len = write_remote_control_access_snapshot(grants.iter().copied(), &mut out).unwrap();

        let reader = read_remote_control_access_snapshot(&out[..len]).unwrap();
        assert_eq!(reader.grant_count(), grants.len());
        assert_eq!(reader.collect::<Vec<_>>(), grants);
    }

    #[test]
    fn persisted_controller_grants_restore_into_a_fixed_table() {
        let grants = [
            grant(
                0x87,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            ),
            grant(
                0xA9,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            ),
        ];
        let mut out = std::vec![0u8; remote_control_access_snapshot_capacity(grants.len())];
        let len = write_remote_control_access_snapshot(grants.iter().copied(), &mut out).unwrap();
        let mut table = FixedRemoteControlAccessTable::<2>::default();

        for grant in read_remote_control_access_snapshot(&out[..len]).unwrap() {
            table.upsert(grant).unwrap();
        }

        assert_eq!(table.grants(), grants);
    }

    #[test]
    fn an_empty_table_round_trips_to_no_grants() {
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
    fn empty_duplicate_and_unknown_request_sets_are_refused() {
        for requests in [&[][..], &[0x01, 0x01], &[0x02, 0x01], &[0xFE]] {
            let mut payload = std::vec![
                0u8;
                ROW_COUNT_LEN
                    + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
                    + REQUEST_COUNT_LEN
                    + requests.len()
            ];
            payload[..ROW_COUNT_LEN].copy_from_slice(&1u32.to_le_bytes());
            let request_count_at = ROW_COUNT_LEN + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN;
            payload[request_count_at] = u8::try_from(requests.len()).unwrap();
            payload[request_count_at + REQUEST_COUNT_LEN..].copy_from_slice(requests);
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
        let grant = grant(0xCB, RemoteControlRequestSet::all());
        let grants = [grant];
        let required = SNAPSHOT_OVERHEAD_LEN
            + ROW_COUNT_LEN
            + REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN
            + REQUEST_COUNT_LEN
            + grant.permitted_requests().len();
        let mut short = std::vec![0u8; required - 1];

        assert_eq!(
            write_remote_control_access_snapshot(grants.iter().copied(), &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }
}
