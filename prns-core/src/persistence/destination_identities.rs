use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::destination_identity::{
    DestinationIdentity, DestinationIdentityRetentionState, DestinationIdentitySeed,
};
use crate::identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey};
use crate::units::InstantMillis;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

const ROW_COUNT_LEN: usize = 4;
const INSTANT_LEN: usize = 8;
const TAG_LEN: usize = 1;
const APP_DATA_LEN_PREFIX_LEN: usize = 2;

const NEVER_USED_TAG: u8 = 0;
const USED_AT_TAG: u8 = 1;
const RETAINED_TAG: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationIdentitiesSnapshotWriteError {
    BufferTooShort,
    AppDataOutgrewLengthPrefix,
    TooManyRows,
}

impl From<SnapshotSealError> for DestinationIdentitiesSnapshotWriteError {
    fn from(SnapshotSealError::BufferTooShort: SnapshotSealError) -> Self {
        DestinationIdentitiesSnapshotWriteError::BufferTooShort
    }
}

pub fn persisted_destination_identity_wire_len(row: &DestinationIdentity<'_>) -> usize {
    TRUNCATED_HASH_BYTE_LEN
        + X25519PublicKey::LEN
        + Ed25519PublicKey::LEN
        + INSTANT_LEN
        + TAG_LEN
        + match row.retention {
            DestinationIdentityRetentionState::UsedAt(_) => INSTANT_LEN,
            DestinationIdentityRetentionState::NeverUsed
            | DestinationIdentityRetentionState::Retained => 0,
        }
        + APP_DATA_LEN_PREFIX_LEN
        + row.app_data.len()
}

pub fn destination_identities_snapshot_len<'a>(
    rows: impl Iterator<Item = DestinationIdentity<'a>>,
) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + ROW_COUNT_LEN
        + rows
            .map(|row| persisted_destination_identity_wire_len(&row))
            .sum::<usize>()
}

pub fn write_destination_identities_snapshot<'a>(
    rows: impl Iterator<Item = DestinationIdentity<'a>>,
    out: &mut [u8],
) -> Result<usize, DestinationIdentitiesSnapshotWriteError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(DestinationIdentitiesSnapshotWriteError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut row_count = 0u32;
    for row in rows {
        if row.app_data.len() > u16::MAX as usize {
            return Err(DestinationIdentitiesSnapshotWriteError::AppDataOutgrewLengthPrefix);
        }
        let row_len = persisted_destination_identity_wire_len(&row);
        if out.len() < at + row_len {
            return Err(DestinationIdentitiesSnapshotWriteError::BufferTooShort);
        }
        at += write_row(&row, &mut out[at..at + row_len]);
        row_count = row_count
            .checked_add(1)
            .ok_or(DestinationIdentitiesSnapshotWriteError::TooManyRows)?;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    Ok(seal_snapshot_in_place(
        SnapshotRegion::DestinationIdentities,
        at - SNAPSHOT_HEADER_LEN,
        out,
    )?)
}

fn write_row(row: &DestinationIdentity<'_>, buf: &mut [u8]) -> usize {
    let mut at = 0;
    let mut put = |bytes: &[u8], at: &mut usize| {
        buf[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    put(row.destination.as_bytes(), &mut at);
    put(row.public_keys.encryption.as_bytes(), &mut at);
    put(row.public_keys.signing.as_bytes(), &mut at);
    put(&row.announced_at.0.to_le_bytes(), &mut at);
    match row.retention {
        DestinationIdentityRetentionState::NeverUsed => put(&[NEVER_USED_TAG], &mut at),
        DestinationIdentityRetentionState::UsedAt(used_at) => {
            put(&[USED_AT_TAG], &mut at);
            put(&used_at.0.to_le_bytes(), &mut at);
        }
        DestinationIdentityRetentionState::Retained => put(&[RETAINED_TAG], &mut at),
    }
    put(&(row.app_data.len() as u16).to_le_bytes(), &mut at);
    put(row.app_data, &mut at);
    at
}

pub fn read_destination_identities_snapshot(
    bytes: &[u8],
) -> Result<PersistedDestinationIdentityRows<'_>, SnapshotReadError> {
    let payload = open_snapshot(SnapshotRegion::DestinationIdentities, bytes)
        .map_err(SnapshotReadError::Envelope)?;
    let Some((row_count, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    Ok(PersistedDestinationIdentityRows {
        rest: rows,
        remaining_rows: u32::from_le_bytes(*row_count),
        poisoned: false,
    })
}

#[derive(Debug, Clone)]
pub struct PersistedDestinationIdentityRows<'a> {
    rest: &'a [u8],
    remaining_rows: u32,
    poisoned: bool,
}

impl<'a> Iterator for PersistedDestinationIdentityRows<'a> {
    type Item = Result<DestinationIdentitySeed<'a>, SnapshotReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned {
            return None;
        }
        if self.remaining_rows == 0 {
            if self.rest.is_empty() {
                return None;
            }
            self.poisoned = true;
            return Some(Err(SnapshotReadError::MalformedPayload));
        }
        match parse_row(self.rest) {
            Some((row, rest)) => {
                self.rest = rest;
                self.remaining_rows -= 1;
                Some(Ok(row))
            }
            None => {
                self.poisoned = true;
                Some(Err(SnapshotReadError::MalformedPayload))
            }
        }
    }
}

fn parse_row(bytes: &[u8]) -> Option<(DestinationIdentitySeed<'_>, &[u8])> {
    let (destination, rest) = bytes.split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()?;
    let (encryption, rest) = rest.split_first_chunk::<{ X25519PublicKey::LEN }>()?;
    let (signing, rest) = rest.split_first_chunk::<{ Ed25519PublicKey::LEN }>()?;
    let (announced_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
    let (&[retention_tag], rest) = rest.split_first_chunk::<TAG_LEN>()?;
    let (retention, rest) = match retention_tag {
        NEVER_USED_TAG => (DestinationIdentityRetentionState::NeverUsed, rest),
        USED_AT_TAG => {
            let (used_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
            (
                DestinationIdentityRetentionState::UsedAt(InstantMillis(u64::from_le_bytes(
                    *used_at,
                ))),
                rest,
            )
        }
        RETAINED_TAG => (DestinationIdentityRetentionState::Retained, rest),
        _ => return None,
    };
    let (app_data_len, rest) = rest.split_first_chunk::<APP_DATA_LEN_PREFIX_LEN>()?;
    let app_data_len = u16::from_le_bytes(*app_data_len) as usize;
    if rest.len() < app_data_len {
        return None;
    }
    let (app_data, rest) = rest.split_at(app_data_len);
    let public_keys = IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(*encryption)),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey(*signing)),
    };
    Some((
        DestinationIdentitySeed {
            destination: DestinationHash::new(*destination),
            public_keys,
            announced_at: InstantMillis(u64::from_le_bytes(*announced_at)),
            retention,
            app_data,
        },
        rest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn row(
        seed: u8,
        retention: DestinationIdentityRetentionState,
        app_data: &[u8],
    ) -> DestinationIdentity<'_> {
        let public_keys = IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed.wrapping_add(1); 32])),
        };
        DestinationIdentity {
            destination: DestinationHash::new([seed; TRUNCATED_HASH_BYTE_LEN]),
            identity: public_keys.identity_hash(),
            public_keys,
            announced_at: InstantMillis(1_000 + u64::from(seed)),
            retention,
            app_data,
        }
    }

    #[test]
    fn every_retention_state_round_trips() {
        let rows = [
            row(1, DestinationIdentityRetentionState::NeverUsed, b"never"),
            row(
                2,
                DestinationIdentityRetentionState::UsedAt(InstantMillis(2_500)),
                b"used",
            ),
            row(3, DestinationIdentityRetentionState::Retained, b"retained"),
        ];
        let mut out = std::vec![0u8; destination_identities_snapshot_len(rows.iter().copied())];
        let len = write_destination_identities_snapshot(rows.iter().copied(), &mut out).unwrap();
        let read: Vec<_> = read_destination_identities_snapshot(&out[..len])
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            read,
            rows.into_iter()
                .map(DestinationIdentitySeed::from)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn an_empty_table_round_trips() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN];
        let len = write_destination_identities_snapshot(core::iter::empty(), &mut out).unwrap();
        assert_eq!(
            read_destination_identities_snapshot(&out[..len])
                .unwrap()
                .count(),
            0,
        );
    }

    #[test]
    fn an_unknown_retention_tag_poisons_the_reader() {
        let rows = [row(4, DestinationIdentityRetentionState::NeverUsed, b"")];
        let mut out = std::vec![0u8; destination_identities_snapshot_len(rows.iter().copied())];
        write_destination_identities_snapshot(rows.iter().copied(), &mut out).unwrap();
        let tag_at = SNAPSHOT_HEADER_LEN
            + ROW_COUNT_LEN
            + TRUNCATED_HASH_BYTE_LEN
            + X25519PublicKey::LEN
            + Ed25519PublicKey::LEN
            + INSTANT_LEN;
        let mut payload = out[SNAPSHOT_HEADER_LEN..].to_vec();
        payload.truncate(payload.len() - super::super::envelope::SNAPSHOT_CHECKSUM_LEN);
        payload[tag_at - SNAPSHOT_HEADER_LEN] = 0x7f;
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::DestinationIdentities,
            &payload,
            &mut resealed,
        )
        .unwrap();
        let mut reader = read_destination_identities_snapshot(&resealed[..len]).unwrap();
        assert_eq!(
            reader.next(),
            Some(Err(SnapshotReadError::MalformedPayload)),
        );
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn payload_bytes_past_the_declared_rows_are_refused() {
        let rows = [row(5, DestinationIdentityRetentionState::Retained, b"tail")];
        let mut out = std::vec![0u8; destination_identities_snapshot_len(rows.iter().copied())];
        let len = write_destination_identities_snapshot(rows.iter().copied(), &mut out).unwrap();
        let mut payload =
            out[SNAPSHOT_HEADER_LEN..len - super::super::envelope::SNAPSHOT_CHECKSUM_LEN].to_vec();
        payload[..ROW_COUNT_LEN].copy_from_slice(&0u32.to_le_bytes());
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::DestinationIdentities,
            &payload,
            &mut resealed,
        )
        .unwrap();
        let mut reader = read_destination_identities_snapshot(&resealed[..len]).unwrap();
        assert_eq!(
            reader.next(),
            Some(Err(SnapshotReadError::MalformedPayload)),
        );
    }
}
