//! The self-ratchets region: one destination's rotation clock and its retained secrets, newest first.
//! RNS 1.4.2 `Destination._persist_ratchets` writes each destination's list to open disk, signed by the destination identity; ours seals the same record and stores it as a vault blob beside the identity secret itself.
//! Intentional deviation from reference: the signature is dropped — a forger who can write the vault already owns the identity, and the envelope checksum still refuses corruption. Willing to revisit this deviation if evidence points towards it being beneficial.
//! One blob per destination: the vault label (`ratchets.<destination-hex>`) addresses it, so the destination never rides the payload.

use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::crypto::ratchets::LastRotated;
use crate::crypto::X25519SecretKey;
use crate::units::InstantMillis;

const LAST_ROTATED_TAG_LEN: usize = 1;
const INSTANT_LEN: usize = 8;
const SECRET_COUNT_LEN: usize = 2;
const LAST_ROTATED_NEVER_TAG: u8 = 0;
const LAST_ROTATED_AT_TAG: u8 = 1;

pub const fn self_ratchets_snapshot_len(secret_count: usize) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + LAST_ROTATED_TAG_LEN
        + INSTANT_LEN
        + SECRET_COUNT_LEN
        + secret_count * X25519SecretKey::LEN
}

pub fn write_self_ratchets_snapshot(
    last_rotated: LastRotated,
    secrets_newest_first: &[X25519SecretKey],
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let total = self_ratchets_snapshot_len(secrets_newest_first.len());
    if out.len() < total {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let mut at = SNAPSHOT_HEADER_LEN;
    let (tag, instant) = match last_rotated {
        LastRotated::Never => (LAST_ROTATED_NEVER_TAG, 0u64),
        LastRotated::At(instant) => (LAST_ROTATED_AT_TAG, instant.0),
    };
    out[at] = tag;
    at += LAST_ROTATED_TAG_LEN;
    out[at..at + INSTANT_LEN].copy_from_slice(&instant.to_le_bytes());
    at += INSTANT_LEN;
    out[at..at + SECRET_COUNT_LEN]
        .copy_from_slice(&(secrets_newest_first.len() as u16).to_le_bytes());
    at += SECRET_COUNT_LEN;
    for secret in secrets_newest_first {
        out[at..at + X25519SecretKey::LEN].copy_from_slice(secret.secret_bytes());
        at += X25519SecretKey::LEN;
    }
    seal_snapshot_in_place(SnapshotRegion::SelfRatchets, at - SNAPSHOT_HEADER_LEN, out)
}

pub fn read_self_ratchets_snapshot(
    bytes: &[u8],
) -> Result<PersistedSelfRatchets<'_>, SnapshotReadError> {
    let payload =
        open_snapshot(SnapshotRegion::SelfRatchets, bytes).map_err(SnapshotReadError::Envelope)?;
    let Some((&[tag], rest)) = payload.split_first_chunk::<LAST_ROTATED_TAG_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let Some((instant, rest)) = rest.split_first_chunk::<INSTANT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let last_rotated = match tag {
        LAST_ROTATED_NEVER_TAG => LastRotated::Never,
        LAST_ROTATED_AT_TAG => LastRotated::At(InstantMillis(u64::from_le_bytes(*instant))),
        _ => return Err(SnapshotReadError::MalformedPayload),
    };
    let Some((secret_count, secrets)) = rest.split_first_chunk::<SECRET_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let secret_count = u64::from(u16::from_le_bytes(*secret_count));
    if secrets.len() as u64 != secret_count * X25519SecretKey::LEN as u64 {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedSelfRatchets {
        last_rotated,
        secrets,
    })
}

/// Fixed-size secrets let the whole payload validate at open, so the secret iterator is infallible.
pub struct PersistedSelfRatchets<'a> {
    pub last_rotated: LastRotated,
    secrets: &'a [u8],
}

impl PersistedSelfRatchets<'_> {
    pub fn secret_count(&self) -> usize {
        self.secrets.len() / X25519SecretKey::LEN
    }

    pub fn secrets_newest_first(&self) -> impl DoubleEndedIterator<Item = X25519SecretKey> + '_ {
        self.secrets
            .as_chunks::<{ X25519SecretKey::LEN }>()
            .0
            .iter()
            .map(|raw| X25519SecretKey::new(*raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::x25519_public_key;
    use std::vec::Vec;

    fn secrets(seeds: &[u8]) -> Vec<X25519SecretKey> {
        seeds
            .iter()
            .map(|&seed| X25519SecretKey::new([seed; 32]))
            .collect()
    }

    fn assert_same_secrets(read: &PersistedSelfRatchets<'_>, written: &[X25519SecretKey]) {
        let read_publics: Vec<_> = read
            .secrets_newest_first()
            .map(|secret| x25519_public_key(&secret))
            .collect();
        let written_publics: Vec<_> = written.iter().map(x25519_public_key).collect();
        assert_eq!(read_publics, written_publics);
    }

    #[test]
    fn a_rotated_record_round_trips() {
        let written = secrets(&[0x11, 0x22, 0x33]);
        let mut out = std::vec![0u8; self_ratchets_snapshot_len(written.len())];
        let len =
            write_self_ratchets_snapshot(LastRotated::At(InstantMillis(9_000)), &written, &mut out)
                .unwrap();
        assert_eq!(len, out.len());

        let read = read_self_ratchets_snapshot(&out[..len]).unwrap();
        assert_eq!(read.last_rotated, LastRotated::At(InstantMillis(9_000)));
        assert_eq!(read.secret_count(), written.len());
        assert_same_secrets(&read, &written);
    }

    #[test]
    fn a_never_rotated_record_round_trips_empty() {
        let mut out = std::vec![0u8; self_ratchets_snapshot_len(0)];
        let len = write_self_ratchets_snapshot(LastRotated::Never, &[], &mut out).unwrap();
        let read = read_self_ratchets_snapshot(&out[..len]).unwrap();
        assert_eq!(read.last_rotated, LastRotated::Never);
        assert_eq!(read.secret_count(), 0);
    }

    #[test]
    fn a_count_that_disagrees_with_the_body_is_refused() {
        let written = secrets(&[0x44]);
        let mut out = std::vec![0u8; self_ratchets_snapshot_len(written.len())];
        let len = write_self_ratchets_snapshot(LastRotated::Never, &written, &mut out).unwrap();

        let mut payload = out[SNAPSHOT_HEADER_LEN..len - 4].to_vec();
        payload[LAST_ROTATED_TAG_LEN + INSTANT_LEN] = 2;
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let resealed_len = super::super::envelope::seal_snapshot(
            SnapshotRegion::SelfRatchets,
            &payload,
            &mut resealed,
        )
        .unwrap();
        assert!(matches!(
            read_self_ratchets_snapshot(&resealed[..resealed_len]),
            Err(SnapshotReadError::MalformedPayload),
        ));
    }

    #[test]
    fn an_unknown_rotation_tag_is_refused() {
        let mut out = std::vec![0u8; self_ratchets_snapshot_len(0)];
        let len = write_self_ratchets_snapshot(LastRotated::Never, &[], &mut out).unwrap();
        let mut payload = out[SNAPSHOT_HEADER_LEN..len - 4].to_vec();
        payload[0] = 0x7F;
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let resealed_len = super::super::envelope::seal_snapshot(
            SnapshotRegion::SelfRatchets,
            &payload,
            &mut resealed,
        )
        .unwrap();
        assert!(matches!(
            read_self_ratchets_snapshot(&resealed[..resealed_len]),
            Err(SnapshotReadError::MalformedPayload),
        ));
    }

    #[test]
    fn a_short_buffer_is_refused() {
        let written = secrets(&[0x55]);
        let mut short = std::vec![0u8; self_ratchets_snapshot_len(written.len()) - 1];
        assert_eq!(
            write_self_ratchets_snapshot(LastRotated::Never, &written, &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }
}
