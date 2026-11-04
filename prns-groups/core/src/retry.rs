//! Allocation-free, time-bounded application request identifiers.
//!
//! These tokens are not credentials. Callers authenticate requests separately,
//! look up retained results before admitting new work, and persist the retirement
//! floor atomically with deleting results. A retired identifier must never be
//! interpreted as a fresh operation, even after the wall clock moves backwards.

const MAGIC: [u8; 4] = *b"PGR1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRetryError"))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RetryError {
    InvalidToken,
    Expired,
    Future,
    InvalidPolicy,
}
impl core::fmt::Display for RetryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "request retry: {self:?}")
    }
}
impl core::error::Error for RetryError {}

/// Version, creation time in seconds, and 160 caller-supplied random bits.
/// Generate entropy using the platform CSPRNG; a timestamp is not randomness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestToken([u8; 32]);
impl RequestToken {
    pub fn new(issued_secs: u64, entropy: [u8; 20]) -> Result<Self, RetryError> {
        if issued_secs == 0 {
            return Err(RetryError::InvalidToken);
        }
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4..12].copy_from_slice(&issued_secs.to_be_bytes());
        bytes[12..].copy_from_slice(&entropy);
        Ok(Self(bytes))
    }
    pub fn parse(bytes: [u8; 32]) -> Result<Self, RetryError> {
        let token = Self(bytes);
        if bytes[..4] != MAGIC || token.issued_secs() == 0 {
            return Err(RetryError::InvalidToken);
        }
        Ok(token)
    }
    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }
    pub fn issued_secs(self) -> u64 {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&self.0[4..12]);
        u64::from_be_bytes(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub horizon_secs: u64,
    pub future_skew_secs: u64,
}
impl RetryPolicy {
    /// Inclusive durable fence. Equality is retired, so advancing to this floor
    /// and removing every result at or below it is safe in the same transaction.
    pub fn retirement_floor(self, now: u64, prior: u64) -> u64 {
        prior.max(now.saturating_sub(self.horizon_secs))
    }
    /// Validate only after an exact retained-result lookup. Opaque legacy IDs
    /// may replay while retained, but cannot be admitted as unknown new work.
    pub fn validate_new(
        self,
        id: [u8; 32],
        now: u64,
        retired_before: u64,
    ) -> Result<(), RetryError> {
        if self.horizon_secs == 0 || now == 0 {
            return Err(RetryError::InvalidPolicy);
        }
        let issued = RequestToken::parse(id)?.issued_secs();
        if issued <= self.retirement_floor(now, retired_before) {
            return Err(RetryError::Expired);
        }
        if issued > now.saturating_add(self.future_skew_secs) {
            return Err(RetryError::Future);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    const POLICY: RetryPolicy = RetryPolicy {
        horizon_secs: 100,
        future_skew_secs: 5,
    };
    fn id(now: u64) -> [u8; 32] {
        RequestToken::new(now, [7; 20]).unwrap().into_bytes()
    }
    #[test]
    fn retirement_is_inclusive_and_cannot_be_reversed_by_clock_rollback() {
        assert_eq!(POLICY.validate_new(id(101), 200, 0), Ok(()));
        assert_eq!(
            POLICY.validate_new(id(100), 200, 0),
            Err(RetryError::Expired)
        );
        let floor = POLICY.retirement_floor(200, 0);
        assert_eq!(POLICY.retirement_floor(90, floor), 100);
        assert_eq!(
            POLICY.validate_new(id(100), 90, floor),
            Err(RetryError::Expired)
        );
        assert_eq!(
            POLICY.validate_new(id(90), 90, floor),
            Err(RetryError::Expired)
        );
    }
    #[test]
    fn unknown_legacy_ids_future_tokens_and_arithmetic_edges_fail_safely() {
        assert_eq!(
            POLICY.validate_new([7; 32], 200, 0),
            Err(RetryError::InvalidToken)
        );
        assert_eq!(
            POLICY.validate_new(id(206), 200, 0),
            Err(RetryError::Future)
        );
        assert_eq!(POLICY.validate_new(id(205), 200, 0), Ok(()));
        assert_eq!(POLICY.validate_new(id(u64::MAX), u64::MAX, 0), Ok(()));
        assert_eq!(RequestToken::new(0, [0; 20]), Err(RetryError::InvalidToken));
        assert_eq!(RequestToken::parse(id(1)).unwrap().issued_secs(), 1);
        assert_eq!(
            RequestToken::parse(id(1)).unwrap().into_bytes()[12..],
            [7; 20]
        );
    }
}
