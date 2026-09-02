use super::super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::super::{SnapshotReadError, SnapshotRegion};
use crate::identity::{
    IdentityHash, IdentityPublicKeys, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN,
};
use crate::remote_control::{RemoteControlRequestKind, RemoteControlRequestSet};

pub(super) const ROW_COUNT_LEN: usize = 4;
const REQUEST_COUNT_LEN: usize = 1;
const REMOTE_CONTROL_IDENTITY_WIRE_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;
const MAX_REMOTE_CONTROL_ACCESS_ROW_WIRE_LEN: usize =
    REMOTE_CONTROL_IDENTITY_WIRE_LEN + REQUEST_COUNT_LEN + RemoteControlRequestKind::ALL.len();

pub(super) const fn remote_control_authorization_snapshot_capacity(row_count: usize) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        .saturating_add(ROW_COUNT_LEN)
        .saturating_add(row_count.saturating_mul(MAX_REMOTE_CONTROL_ACCESS_ROW_WIRE_LEN))
}

pub(super) struct RemoteControlAuthorizationRow<'a> {
    pub(super) public_keys: &'a IdentityPublicKeys,
    pub(super) permitted_requests: &'a RemoteControlRequestSet,
}

pub(super) fn write_remote_control_authorization_rows<'a>(
    region: SnapshotRegion,
    row_count: usize,
    rows: impl Iterator<Item = RemoteControlAuthorizationRow<'a>>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let payload_start = SNAPSHOT_HEADER_LEN.saturating_add(ROW_COUNT_LEN);
    if out.len() < payload_start {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let row_count = u32::try_from(row_count).map_err(|_| SnapshotSealError::BufferTooShort)?;
    let mut at = payload_start;
    let mut encoded_rows = 0u32;
    for row in rows {
        let encoded_len = REMOTE_CONTROL_IDENTITY_WIRE_LEN
            .saturating_add(REQUEST_COUNT_LEN)
            .saturating_add(row.permitted_requests.len());
        let Some(next_at) = at.checked_add(encoded_len) else {
            return Err(SnapshotSealError::BufferTooShort);
        };
        if out.len() < next_at {
            return Err(SnapshotSealError::BufferTooShort);
        }
        out[at..at + REMOTE_CONTROL_IDENTITY_WIRE_LEN]
            .copy_from_slice(&row.public_keys.public_key_bytes());
        at += REMOTE_CONTROL_IDENTITY_WIRE_LEN;
        out[at] = row.permitted_requests.wire_count();
        at += REQUEST_COUNT_LEN;
        for request in row.permitted_requests.iter() {
            out[at] = request.wire_value();
            at += 1;
        }
        encoded_rows = encoded_rows
            .checked_add(1)
            .ok_or(SnapshotSealError::BufferTooShort)?;
    }
    if encoded_rows != row_count {
        return Err(SnapshotSealError::BufferTooShort);
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    seal_snapshot_in_place(region, at - SNAPSHOT_HEADER_LEN, out)
}

#[derive(Debug, Clone)]
pub(super) struct PersistedRemoteControlAuthorizationRows<'a> {
    rest: &'a [u8],
    row_count: usize,
}

impl PersistedRemoteControlAuthorizationRows<'_> {
    pub(super) const fn len(&self) -> usize {
        self.row_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParsedRemoteControlAuthorizationRow {
    pub(super) public_keys: IdentityPublicKeys,
    pub(super) permitted_requests: RemoteControlRequestSet,
}

impl Iterator for PersistedRemoteControlAuthorizationRows<'_> {
    type Item = ParsedRemoteControlAuthorizationRow;

    fn next(&mut self) -> Option<Self::Item> {
        let (row, rest) = parse_remote_control_authorization_row(self.rest)?;
        self.rest = rest;
        self.row_count = self.row_count.saturating_sub(1);
        Some(row)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.row_count, Some(self.row_count))
    }
}

impl ExactSizeIterator for PersistedRemoteControlAuthorizationRows<'_> {}

pub(super) fn read_remote_control_authorization_rows(
    region: SnapshotRegion,
    bytes: &[u8],
) -> Result<PersistedRemoteControlAuthorizationRows<'_>, SnapshotReadError> {
    let payload = open_snapshot(region, bytes).map_err(SnapshotReadError::Envelope)?;
    let Some((row_count_bytes, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let row_count = usize::try_from(u32::from_le_bytes(*row_count_bytes))
        .map_err(|_| SnapshotReadError::MalformedPayload)?;
    let mut rest = rows;
    let mut previous_identity: Option<IdentityHash> = None;
    for _ in 0..row_count {
        let Some((row, remaining)) = parse_remote_control_authorization_row(rest) else {
            return Err(SnapshotReadError::MalformedPayload);
        };
        let identity = row.public_keys.identity_hash();
        if previous_identity.is_some_and(|previous| previous.as_bytes() >= identity.as_bytes()) {
            return Err(SnapshotReadError::MalformedPayload);
        }
        previous_identity = Some(identity);
        rest = remaining;
    }
    if !rest.is_empty() {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedRemoteControlAuthorizationRows {
        rest: rows,
        row_count,
    })
}

fn parse_remote_control_authorization_row(
    bytes: &[u8],
) -> Option<(ParsedRemoteControlAuthorizationRow, &[u8])> {
    let (public_keys, rest) = bytes.split_first_chunk::<REMOTE_CONTROL_IDENTITY_WIRE_LEN>()?;
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
    if permitted_requests.is_empty() {
        return None;
    }
    Some((
        ParsedRemoteControlAuthorizationRow {
            public_keys: PublicIdentityMaterial::from_bytes(*public_keys).public_keys(),
            permitted_requests,
        },
        rest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::persistence::{seal_snapshot, SNAPSHOT_OVERHEAD_LEN};

    fn public_keys(fill: u8) -> IdentityPublicKeys {
        IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        }
    }

    fn row_bytes(public_keys: &IdentityPublicKeys, requests: &[u8]) -> std::vec::Vec<u8> {
        let mut row = std::vec::Vec::from(public_keys.public_key_bytes());
        row.push(u8::try_from(requests.len()).unwrap());
        row.extend_from_slice(requests);
        row
    }

    fn sealed_rows(rows: &[std::vec::Vec<u8>]) -> std::vec::Vec<u8> {
        let row_bytes = rows.iter().map(std::vec::Vec::len).sum::<usize>();
        let mut payload = std::vec![0u8; ROW_COUNT_LEN + row_bytes];
        payload[..ROW_COUNT_LEN].copy_from_slice(&u32::try_from(rows.len()).unwrap().to_le_bytes());
        let mut at = ROW_COUNT_LEN;
        for row in rows {
            let next = at + row.len();
            payload[at..next].copy_from_slice(row);
            at = next;
        }
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = seal_snapshot(
            SnapshotRegion::RemoteControlControllerGrants,
            &payload,
            &mut sealed,
        )
        .unwrap();
        sealed.truncate(len);
        sealed
    }

    #[test]
    fn empty_duplicate_descending_and_unknown_request_sets_are_refused() {
        for requests in [&[][..], &[0x01, 0x01], &[0x02, 0x01], &[0xFE]] {
            let sealed = sealed_rows(&[row_bytes(&public_keys(0x21), requests)]);

            assert_eq!(
                read_remote_control_authorization_rows(
                    SnapshotRegion::RemoteControlControllerGrants,
                    &sealed,
                )
                .err(),
                Some(SnapshotReadError::MalformedPayload),
            );
        }
    }

    #[test]
    fn duplicate_and_descending_identities_are_refused() {
        let first = public_keys(0x43);
        let second = public_keys(0x65);
        let (lower, higher) =
            if first.identity_hash().as_bytes() < second.identity_hash().as_bytes() {
                (first, second)
            } else {
                (second, first)
            };
        let request = [RemoteControlRequestKind::Describe.wire_value()];
        for rows in [
            [row_bytes(&lower, &request), row_bytes(&lower, &request)],
            [row_bytes(&higher, &request), row_bytes(&lower, &request)],
        ] {
            let sealed = sealed_rows(&rows);

            assert_eq!(
                read_remote_control_authorization_rows(
                    SnapshotRegion::RemoteControlControllerGrants,
                    &sealed,
                )
                .err(),
                Some(SnapshotReadError::MalformedPayload),
            );
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let request = [RemoteControlRequestKind::Describe.wire_value()];
        let mut row = row_bytes(&public_keys(0x87), &request);
        row.push(0xFF);
        let sealed = sealed_rows(&[row]);

        assert_eq!(
            read_remote_control_authorization_rows(
                SnapshotRegion::RemoteControlControllerGrants,
                &sealed,
            )
            .err(),
            Some(SnapshotReadError::MalformedPayload),
        );
    }
}
