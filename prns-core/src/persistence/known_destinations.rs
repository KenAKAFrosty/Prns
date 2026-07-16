use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::known::{
    KnownDestination, KnownDestinationRetentionState, KnownDestinationSeed,
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
pub enum KnownDestinationsSnapshotWriteError {
    BufferTooShort,
    AppDataOutgrewLengthPrefix,
    TooManyRows,
}

impl From<SnapshotSealError> for KnownDestinationsSnapshotWriteError {
    fn from(SnapshotSealError::BufferTooShort: SnapshotSealError) -> Self {
        KnownDestinationsSnapshotWriteError::BufferTooShort
    }
}

pub fn persisted_known_destination_wire_len(row: &KnownDestination<'_>) -> usize {
    TRUNCATED_HASH_BYTE_LEN
        + X25519PublicKey::LEN
        + Ed25519PublicKey::LEN
        + INSTANT_LEN
        + TAG_LEN
        + match row.retention {
            KnownDestinationRetentionState::UsedAt(_) => INSTANT_LEN,
            KnownDestinationRetentionState::NeverUsed
            | KnownDestinationRetentionState::Retained => 0,
        }
        + APP_DATA_LEN_PREFIX_LEN
        + row.app_data.len()
}

pub fn known_destinations_snapshot_len<'a>(
    rows: impl Iterator<Item = KnownDestination<'a>>,
) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + ROW_COUNT_LEN
        + rows
            .map(|row| persisted_known_destination_wire_len(&row))
            .sum::<usize>()
}

pub fn write_known_destinations_snapshot<'a>(
    rows: impl Iterator<Item = KnownDestination<'a>>,
    out: &mut [u8],
) -> Result<usize, KnownDestinationsSnapshotWriteError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(KnownDestinationsSnapshotWriteError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut row_count = 0u32;
    for row in rows {
        if row.app_data.len() > u16::MAX as usize {
            return Err(KnownDestinationsSnapshotWriteError::AppDataOutgrewLengthPrefix);
        }
        let row_len = persisted_known_destination_wire_len(&row);
        if out.len() < at + row_len {
            return Err(KnownDestinationsSnapshotWriteError::BufferTooShort);
        }
        at += write_row(&row, &mut out[at..at + row_len]);
        row_count = row_count
            .checked_add(1)
            .ok_or(KnownDestinationsSnapshotWriteError::TooManyRows)?;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    Ok(seal_snapshot_in_place(
        SnapshotRegion::KnownDestinations,
        at - SNAPSHOT_HEADER_LEN,
        out,
    )?)
}

fn write_row(row: &KnownDestination<'_>, buf: &mut [u8]) -> usize {
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
        KnownDestinationRetentionState::NeverUsed => put(&[NEVER_USED_TAG], &mut at),
        KnownDestinationRetentionState::UsedAt(used_at) => {
            put(&[USED_AT_TAG], &mut at);
            put(&used_at.0.to_le_bytes(), &mut at);
        }
        KnownDestinationRetentionState::Retained => put(&[RETAINED_TAG], &mut at),
    }
    put(&(row.app_data.len() as u16).to_le_bytes(), &mut at);
    put(row.app_data, &mut at);
    at
}

pub fn read_known_destinations_snapshot(
    bytes: &[u8],
) -> Result<PersistedKnownDestinationRows<'_>, SnapshotReadError> {
    let payload = open_snapshot(SnapshotRegion::KnownDestinations, bytes)
        .map_err(SnapshotReadError::Envelope)?;
    let Some((row_count, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    Ok(PersistedKnownDestinationRows {
        rest: rows,
        remaining_rows: u32::from_le_bytes(*row_count),
        poisoned: false,
    })
}

#[derive(Debug, Clone)]
pub struct PersistedKnownDestinationRows<'a> {
    rest: &'a [u8],
    remaining_rows: u32,
    poisoned: bool,
}

impl<'a> Iterator for PersistedKnownDestinationRows<'a> {
    type Item = Result<KnownDestinationSeed<'a>, SnapshotReadError>;

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

fn parse_row(bytes: &[u8]) -> Option<(KnownDestinationSeed<'_>, &[u8])> {
    let (destination, rest) = bytes.split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()?;
    let (encryption, rest) = rest.split_first_chunk::<{ X25519PublicKey::LEN }>()?;
    let (signing, rest) = rest.split_first_chunk::<{ Ed25519PublicKey::LEN }>()?;
    let (announced_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
    let (&[retention_tag], rest) = rest.split_first_chunk::<TAG_LEN>()?;
    let (retention, rest) = match retention_tag {
        NEVER_USED_TAG => (KnownDestinationRetentionState::NeverUsed, rest),
        USED_AT_TAG => {
            let (used_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
            (
                KnownDestinationRetentionState::UsedAt(InstantMillis(u64::from_le_bytes(*used_at))),
                rest,
            )
        }
        RETAINED_TAG => (KnownDestinationRetentionState::Retained, rest),
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
        KnownDestinationSeed {
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
        retention: KnownDestinationRetentionState,
        app_data: &[u8],
    ) -> KnownDestination<'_> {
        let public_keys = IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed.wrapping_add(1); 32])),
        };
        KnownDestination {
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
            row(1, KnownDestinationRetentionState::NeverUsed, b"never"),
            row(
                2,
                KnownDestinationRetentionState::UsedAt(InstantMillis(2_500)),
                b"used",
            ),
            row(3, KnownDestinationRetentionState::Retained, b"retained"),
        ];
        let mut out = std::vec![0u8; known_destinations_snapshot_len(rows.iter().copied())];
        let len = write_known_destinations_snapshot(rows.iter().copied(), &mut out).unwrap();
        let read: Vec<_> = read_known_destinations_snapshot(&out[..len])
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            read,
            rows.into_iter()
                .map(KnownDestinationSeed::from)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn an_empty_table_round_trips() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN];
        let len = write_known_destinations_snapshot(core::iter::empty(), &mut out).unwrap();
        assert_eq!(
            read_known_destinations_snapshot(&out[..len])
                .unwrap()
                .count(),
            0,
        );
    }

    #[test]
    fn an_unknown_retention_tag_poisons_the_reader() {
        let rows = [row(4, KnownDestinationRetentionState::NeverUsed, b"")];
        let mut out = std::vec![0u8; known_destinations_snapshot_len(rows.iter().copied())];
        write_known_destinations_snapshot(rows.iter().copied(), &mut out).unwrap();
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
            SnapshotRegion::KnownDestinations,
            &payload,
            &mut resealed,
        )
        .unwrap();
        let mut reader = read_known_destinations_snapshot(&resealed[..len]).unwrap();
        assert_eq!(
            reader.next(),
            Some(Err(SnapshotReadError::MalformedPayload)),
        );
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn payload_bytes_past_the_declared_rows_are_refused() {
        let rows = [row(5, KnownDestinationRetentionState::Retained, b"tail")];
        let mut out = std::vec![0u8; known_destinations_snapshot_len(rows.iter().copied())];
        let len = write_known_destinations_snapshot(rows.iter().copied(), &mut out).unwrap();
        let mut payload =
            out[SNAPSHOT_HEADER_LEN..len - super::super::envelope::SNAPSHOT_CHECKSUM_LEN].to_vec();
        payload[..ROW_COUNT_LEN].copy_from_slice(&0u32.to_le_bytes());
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::KnownDestinations,
            &payload,
            &mut resealed,
        )
        .unwrap();
        let mut reader = read_known_destinations_snapshot(&resealed[..len]).unwrap();
        assert_eq!(
            reader.next(),
            Some(Err(SnapshotReadError::MalformedPayload)),
        );
    }
}
