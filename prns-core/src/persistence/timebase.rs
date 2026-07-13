//! The clock snapshot: the flushed high-water mark of the engine's logical timeline.
//! A wall-clocked host starts its timeline at the wall clock every boot, so downtime ages persisted timestamps naturally and the high-water is only its rollback floor.
//! A wall-less host restarts at zero, so it resumes past the high-water instead — the persisted timestamps stay meaningful and only the downtime goes uncounted.

use super::envelope::{
    open_snapshot, seal_snapshot, SnapshotOpenError, SnapshotSealError, SNAPSHOT_OVERHEAD_LEN,
};
use super::SnapshotRegion;
use crate::units::InstantMillis;

const TIMEBASE_PAYLOAD_LEN: usize = 8;
pub const TIMEBASE_SNAPSHOT_LEN: usize = SNAPSHOT_OVERHEAD_LEN + TIMEBASE_PAYLOAD_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReadError {
    Envelope(SnapshotOpenError),
    MalformedPayload,
}

pub fn write_timebase_snapshot(
    high_water: InstantMillis,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    seal_snapshot(SnapshotRegion::Timebase, &high_water.0.to_le_bytes(), out)
}

pub fn read_timebase_snapshot(bytes: &[u8]) -> Result<InstantMillis, SnapshotReadError> {
    let payload =
        open_snapshot(SnapshotRegion::Timebase, bytes).map_err(SnapshotReadError::Envelope)?;
    if payload.len() != TIMEBASE_PAYLOAD_LEN {
        return Err(SnapshotReadError::MalformedPayload);
    }
    let mut high_water_bytes = [0u8; TIMEBASE_PAYLOAD_LEN];
    high_water_bytes.copy_from_slice(payload);
    Ok(InstantMillis(u64::from_le_bytes(high_water_bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_high_water_round_trips() {
        let mut out = [0u8; TIMEBASE_SNAPSHOT_LEN];
        let len = write_timebase_snapshot(InstantMillis(1_770_000_000_000), &mut out).unwrap();
        assert_eq!(
            read_timebase_snapshot(&out[..len]).unwrap(),
            InstantMillis(1_770_000_000_000),
        );
    }

    #[test]
    fn a_wrong_size_payload_is_malformed() {
        let mut sealed = [0u8; TIMEBASE_SNAPSHOT_LEN + 1];
        let len = seal_snapshot(SnapshotRegion::Timebase, &[0u8; 9], &mut sealed).unwrap();
        assert_eq!(
            read_timebase_snapshot(&sealed[..len]),
            Err(SnapshotReadError::MalformedPayload),
        );
    }
}
