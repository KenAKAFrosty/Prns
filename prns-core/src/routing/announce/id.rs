//! `AnnounceId` and its two halves, `AnnounceNonce` + `MonotonicTimebase`.
//!
//! This is the 10-byte field RNS calls `random_hash`, but is neither fully random nor a hash:
//! - Bytes 0..5 are a per-emission random nonce (the replay/loop dedup tag)
//! - Bytes 5..10 the origin's clock at emission (big-endian, the monotonic "announce time" receivers compare per destination).

use crate::engine::InstantMillis;
pub const ANNOUNCE_ID_WIRE_LEN: usize = 10;
const NONCE_LEN: usize = 5;
const TIMEBASE_LEN: usize = 5;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnounceNonce([u8; NONCE_LEN]);

impl AnnounceNonce {
    pub const fn new(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; NONCE_LEN] {
        &self.0
    }
}

/// The 5-byte announce entropy an [`AnnounceNonce`] is minted from.
/// Move-only: a wire-exposed, must-be-unique draw, so `AnnounceId::mint` consumes it and two announces can never be minted from one draw.
#[derive(Debug)]
pub struct AnnounceEntropy([u8; NONCE_LEN]);

impl AnnounceEntropy {
    pub const LEN: usize = NONCE_LEN;

    pub const fn new(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }
}

/// The origin's clock at announce emission: 5-byte big-endian whole seconds (`0..=2^40-1`).
/// Receivers only ever compare these, never add them to a wall clock, so the type stays distinct from epoch seconds.
/// Big-endian byte order makes `Ord` numeric.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonotonicTimebase([u8; TIMEBASE_LEN]);

impl MonotonicTimebase {
    pub const ZERO: Self = Self([0u8; TIMEBASE_LEN]);

    pub const fn from_wire(bytes: [u8; TIMEBASE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_wire(&self) -> &[u8; TIMEBASE_LEN] {
        &self.0
    }

    pub fn as_count(&self) -> u64 {
        let mut buf = [0u8; 8];
        buf[8 - TIMEBASE_LEN..].copy_from_slice(&self.0);
        u64::from_be_bytes(buf)
    }
}

impl core::fmt::Debug for MonotonicTimebase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MonotonicTimebase({})", self.as_count())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnnounceId {
    pub nonce: AnnounceNonce,
    pub timebase: MonotonicTimebase,
}

impl AnnounceId {
    pub fn mint(announce_entropy: AnnounceEntropy, now: InstantMillis) -> Self {
        let emitted_seconds = now.0 / 1_000; //RNS also flattens to second-level granularity on announce timing, so this is parity-faithful
        let mut timebase = [0u8; TIMEBASE_LEN];
        timebase.copy_from_slice(&emitted_seconds.to_be_bytes()[8 - TIMEBASE_LEN..]);
        Self {
            nonce: AnnounceNonce(announce_entropy.0),
            timebase: MonotonicTimebase(timebase),
        }
    }

    pub fn from_wire(bytes: [u8; ANNOUNCE_ID_WIRE_LEN]) -> Self {
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[..NONCE_LEN]);
        let mut timebase = [0u8; TIMEBASE_LEN];
        timebase.copy_from_slice(&bytes[NONCE_LEN..]);
        Self {
            nonce: AnnounceNonce(nonce),
            timebase: MonotonicTimebase(timebase),
        }
    }

    pub fn to_wire_bytes(&self) -> [u8; ANNOUNCE_ID_WIRE_LEN] {
        let mut out = [0u8; ANNOUNCE_ID_WIRE_LEN];
        out[..NONCE_LEN].copy_from_slice(&self.nonce.0);
        out[NONCE_LEN..].copy_from_slice(&self.timebase.0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_splits_at_five() {
        let id = AnnounceId::from_wire([1, 2, 3, 4, 5, 0, 0, 0, 0x01, 0x02]);
        assert_eq!(id.nonce, AnnounceNonce::new([1, 2, 3, 4, 5]));
        assert_eq!(id.timebase.as_count(), 0x0102);
        assert_eq!(id.to_wire_bytes(), [1, 2, 3, 4, 5, 0, 0, 0, 0x01, 0x02]);
    }

    #[test]
    fn timebase_orders_numerically() {
        let lo = MonotonicTimebase::from_wire([0, 0, 0, 0, 10]);
        let hi = MonotonicTimebase::from_wire([0, 0, 0, 1, 0]);
        assert!(hi > lo);
    }

    #[test]
    fn timebase_debug_prints_the_numeric_count() {
        let timebase = MonotonicTimebase::from_wire([0, 0, 0, 1, 0]);
        assert_eq!(format!("{timebase:?}"), "MonotonicTimebase(256)");
    }

    #[test]
    fn mint_floors_milliseconds_to_whole_seconds() {
        let id = AnnounceId::mint(
            AnnounceEntropy::new([9, 8, 7, 6, 5]),
            InstantMillis(123_456),
        );
        assert_eq!(id.timebase.as_count(), 123);
        assert_eq!(id.to_wire_bytes(), [9, 8, 7, 6, 5, 0, 0, 0, 0, 123]);
    }
}
