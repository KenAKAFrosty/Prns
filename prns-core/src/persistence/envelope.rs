//! The sealed frame every snapshot travels in: magic, version, region tag, payload length, payload, CRC-32.
//! A load is untrusted input, so truncation, cross-region mixups, and bit rot each refuse by name before any payload parse.

use super::SnapshotRegion;

pub const SNAPSHOT_MAGIC: [u8; 4] = *b"PRNS";
pub const SNAPSHOT_VERSION: u8 = 1;

pub const SNAPSHOT_HEADER_LEN: usize = SNAPSHOT_MAGIC.len() + 1 + 1 + 4;
pub const SNAPSHOT_CHECKSUM_LEN: usize = 4;
pub const SNAPSHOT_OVERHEAD_LEN: usize = SNAPSHOT_HEADER_LEN + SNAPSHOT_CHECKSUM_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotSealError {
    BufferTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOpenError {
    Truncated,
    BadMagic,
    UnsupportedVersion { found: u8 },
    WrongRegion { found: u8 },
    ChecksumMismatch,
}

pub fn seal_snapshot(
    region: SnapshotRegion,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let Some(total_len) = SNAPSHOT_OVERHEAD_LEN.checked_add(payload.len()) else {
        return Err(SnapshotSealError::BufferTooShort);
    };
    if out.len() < total_len {
        return Err(SnapshotSealError::BufferTooShort);
    }
    out[SNAPSHOT_HEADER_LEN..SNAPSHOT_HEADER_LEN + payload.len()].copy_from_slice(payload);
    seal_snapshot_in_place(region, payload.len(), out)
}

/// Seals a payload the caller already wrote at `out[SNAPSHOT_HEADER_LEN..SNAPSHOT_HEADER_LEN + payload_len]`, sparing a large payload the staging copy `seal_snapshot` takes.
pub fn seal_snapshot_in_place(
    region: SnapshotRegion,
    payload_len: usize,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let Some(total_len) = SNAPSHOT_OVERHEAD_LEN.checked_add(payload_len) else {
        return Err(SnapshotSealError::BufferTooShort);
    };
    if out.len() < total_len {
        return Err(SnapshotSealError::BufferTooShort);
    }
    out[..SNAPSHOT_MAGIC.len()].copy_from_slice(&SNAPSHOT_MAGIC);
    out[4] = SNAPSHOT_VERSION;
    out[5] = region.tag();
    out[6..SNAPSHOT_HEADER_LEN].copy_from_slice(&(payload_len as u32).to_le_bytes());
    let checksum = crc32(&out[..SNAPSHOT_HEADER_LEN + payload_len]);
    out[SNAPSHOT_HEADER_LEN + payload_len..total_len].copy_from_slice(&checksum.to_le_bytes());
    Ok(total_len)
}

pub fn open_snapshot(region: SnapshotRegion, bytes: &[u8]) -> Result<&[u8], SnapshotOpenError> {
    if bytes.len() < SNAPSHOT_OVERHEAD_LEN {
        return Err(SnapshotOpenError::Truncated);
    }
    if bytes[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC {
        return Err(SnapshotOpenError::BadMagic);
    }
    if bytes[4] != SNAPSHOT_VERSION {
        return Err(SnapshotOpenError::UnsupportedVersion { found: bytes[4] });
    }
    if bytes[5] != region.tag() {
        return Err(SnapshotOpenError::WrongRegion { found: bytes[5] });
    }
    let payload_len = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let Some(total_len) = SNAPSHOT_OVERHEAD_LEN.checked_add(payload_len) else {
        return Err(SnapshotOpenError::Truncated);
    };
    if bytes.len() < total_len {
        return Err(SnapshotOpenError::Truncated);
    }
    let checksum_at = SNAPSHOT_HEADER_LEN + payload_len;
    let found = u32::from_le_bytes([
        bytes[checksum_at],
        bytes[checksum_at + 1],
        bytes[checksum_at + 2],
        bytes[checksum_at + 3],
    ]);
    if found != crc32(&bytes[..checksum_at]) {
        return Err(SnapshotOpenError::ChecksumMismatch);
    }
    Ok(&bytes[SNAPSHOT_HEADER_LEN..checksum_at])
}

/// The checksum a sealed snapshot already carries, reborrowed as its change fingerprint: two seals of identical payloads carry identical fingerprints, so a flusher can skip rewriting a region whose fingerprint matches its last flush — no second hash pass.
/// A changed payload whose CRC-32 collides with the previous one (odds 2^-32 per flush) skips one write; the next real change heals it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotFingerprint([u8; SNAPSHOT_CHECKSUM_LEN]);

pub fn snapshot_fingerprint(sealed: &[u8]) -> Option<SnapshotFingerprint> {
    if sealed.len() < SNAPSHOT_OVERHEAD_LEN {
        return None;
    }
    let payload_len = u32::from_le_bytes([sealed[6], sealed[7], sealed[8], sealed[9]]) as usize;
    let checksum_at = SNAPSHOT_HEADER_LEN.checked_add(payload_len)?;
    let checksum = sealed.get(checksum_at..checksum_at + SNAPSHOT_CHECKSUM_LEN)?;
    let mut fingerprint = [0u8; SNAPSHOT_CHECKSUM_LEN];
    fingerprint.copy_from_slice(checksum);
    Some(SnapshotFingerprint(fingerprint))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low_bit_set = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & low_bit_set);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGION: SnapshotRegion = SnapshotRegion::Timebase;

    fn sealed(payload: &[u8]) -> std::vec::Vec<u8> {
        let mut out = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = seal_snapshot(REGION, payload, &mut out).unwrap();
        out.truncate(len);
        out
    }

    #[test]
    fn a_sealed_snapshot_opens_to_its_payload() {
        let bytes = sealed(b"the payload");
        assert_eq!(open_snapshot(REGION, &bytes).unwrap(), b"the payload");
    }

    #[test]
    fn tail_padding_after_the_checksum_is_ignored() {
        let mut bytes = sealed(b"the payload");
        bytes.extend_from_slice(&[0xFF; 32]);
        assert_eq!(open_snapshot(REGION, &bytes).unwrap(), b"the payload");
    }

    #[test]
    fn an_empty_payload_round_trips() {
        assert_eq!(open_snapshot(REGION, &sealed(b"")).unwrap(), b"");
    }

    #[test]
    fn a_short_seal_buffer_is_refused() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN];
        assert_eq!(
            seal_snapshot(REGION, b"x", &mut out),
            Err(SnapshotSealError::BufferTooShort),
        );
    }

    #[test]
    fn identical_payloads_carry_identical_fingerprints_and_a_changed_payload_does_not() {
        let first = snapshot_fingerprint(&sealed(b"the payload")).unwrap();
        let second = snapshot_fingerprint(&sealed(b"the payload")).unwrap();
        let changed = snapshot_fingerprint(&sealed(b"the payloae")).unwrap();
        assert_eq!(first, second);
        assert_ne!(first, changed);
    }

    #[test]
    fn a_truncated_snapshot_has_no_fingerprint() {
        let bytes = sealed(b"the payload");
        assert_eq!(snapshot_fingerprint(&bytes[..bytes.len() - 1]), None);
        assert_eq!(snapshot_fingerprint(&[]), None);
    }

    #[test]
    fn each_corruption_refuses_by_name() {
        let bytes = sealed(b"the payload");

        assert_eq!(
            open_snapshot(REGION, &bytes[..SNAPSHOT_OVERHEAD_LEN - 1]),
            Err(SnapshotOpenError::Truncated),
        );

        let mut wrong_magic = bytes.clone();
        wrong_magic[0] ^= 0xFF;
        assert_eq!(
            open_snapshot(REGION, &wrong_magic),
            Err(SnapshotOpenError::BadMagic),
        );

        let mut future_version = bytes.clone();
        future_version[4] = SNAPSHOT_VERSION + 1;
        assert_eq!(
            open_snapshot(REGION, &future_version),
            Err(SnapshotOpenError::UnsupportedVersion {
                found: SNAPSHOT_VERSION + 1
            }),
        );

        let mut foreign_region = bytes.clone();
        foreign_region[5] = 0xEE;
        assert_eq!(
            open_snapshot(REGION, &foreign_region),
            Err(SnapshotOpenError::WrongRegion { found: 0xEE }),
        );

        let mut bit_rot = bytes.clone();
        bit_rot[SNAPSHOT_HEADER_LEN] ^= 0x01;
        assert_eq!(
            open_snapshot(REGION, &bit_rot),
            Err(SnapshotOpenError::ChecksumMismatch),
        );

        let mut lying_length = bytes;
        lying_length[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            open_snapshot(REGION, &lying_length),
            Err(SnapshotOpenError::Truncated),
        );
    }
}
