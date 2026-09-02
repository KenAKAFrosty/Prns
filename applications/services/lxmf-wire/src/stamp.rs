use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{MESSAGE_ID_LENGTH, POW_STAMP_LENGTH, TICKET_LENGTH};

/// Python LXMF regular-message workblock expansion rounds.
pub const POW_EXPAND_ROUNDS: usize = 3_000;

/// Explicit CPU-work authorization for one proof-of-work validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowBudget {
    /// Maximum HKDF expansion rounds the caller authorizes.
    pub max_expand_rounds: usize,
}

impl PowBudget {
    /// Construct a proof-of-work budget.
    pub const fn new(max_expand_rounds: usize) -> Self {
        Self { max_expand_rounds }
    }
}

/// Result of an exact Python LXMF proof-of-work stamp calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowStampValidation {
    /// Whether the digest integer is at most Python's inclusive target.
    pub valid: bool,
    /// Number of leading zero bits in the calculated digest.
    pub value: u16,
}

/// Failure before a stamp validity decision can be calculated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampError {
    /// Proof-of-work stamps are exactly 32 bytes.
    InvalidPowStampLength { actual: usize },
    /// Python's 256-bit target cannot represent costs above 256.
    InvalidTargetCost { actual: u16 },
    /// The caller did not authorize all protocol-required expansion rounds.
    BudgetExceeded { required: usize, authorized: usize },
    /// HKDF rejected the fixed protocol output size.
    KdfFailure,
}

/// Validate a ticket-derived stamp against one caller-owned trusted prior ticket.
///
/// The ticket must come from the receiver's previously issued ticket store.
/// A `FIELD_TICKET` carried inside the message being admitted is never an
/// authority for this check.
pub fn validate_ticket_stamp(
    stamp: &[u8],
    message_id: &[u8; MESSAGE_ID_LENGTH],
    trusted_prior_ticket: &[u8; TICKET_LENGTH],
) -> bool {
    if stamp.len() != TICKET_LENGTH {
        return false;
    }
    let mut hasher = Sha256::new();
    hasher.update(trusted_prior_ticket);
    hasher.update(message_id);
    let digest = hasher.finalize();
    digest[..TICKET_LENGTH].ct_eq(stamp).unwrap_u8() == 1
}

/// Validate a regular 32-byte Python LXMF proof-of-work stamp without
/// materializing the 768,000-byte workblock.
///
/// Every 256-byte HKDF block is streamed directly into the final SHA-256
/// state. The function always uses the protocol's 3,000 rounds; `budget`
/// explicitly authorizes that CPU work instead of changing the wire result.
pub fn validate_pow_stamp(
    stamp: &[u8],
    message_id: &[u8; MESSAGE_ID_LENGTH],
    target_cost: u16,
    budget: PowBudget,
) -> Result<PowStampValidation, StampError> {
    if stamp.len() != POW_STAMP_LENGTH {
        return Err(StampError::InvalidPowStampLength {
            actual: stamp.len(),
        });
    }
    if target_cost > 256 {
        return Err(StampError::InvalidTargetCost {
            actual: target_cost,
        });
    }
    if budget.max_expand_rounds < POW_EXPAND_ROUNDS {
        return Err(StampError::BudgetExceeded {
            required: POW_EXPAND_ROUNDS,
            authorized: budget.max_expand_rounds,
        });
    }

    let mut workblock_hash = Sha256::new();
    for round in 0..POW_EXPAND_ROUNDS {
        let mut salt_hash = Sha256::new();
        salt_hash.update(message_id);
        let (encoded_round, encoded_length) = encode_msgpack_uint(round as u64);
        salt_hash.update(&encoded_round[..encoded_length]);
        let salt: [u8; 32] = salt_hash.finalize().into();

        let hkdf = Hkdf::<Sha256>::new(Some(&salt), message_id);
        let mut block = [0_u8; 256];
        hkdf.expand(&[], &mut block)
            .map_err(|_| StampError::KdfFailure)?;
        workblock_hash.update(block);
    }
    workblock_hash.update(stamp);
    let digest: [u8; 32] = workblock_hash.finalize().into();
    Ok(PowStampValidation {
        valid: meets_python_target(&digest, target_cost),
        value: leading_zero_bits(&digest),
    })
}

fn meets_python_target(digest: &[u8; 32], target_cost: u16) -> bool {
    if target_cost == 0 {
        return true;
    }
    let target_bit = usize::from(target_cost - 1);
    let mut target = [0_u8; 32];
    target[target_bit / 8] = 0x80 >> (target_bit % 8);
    digest <= &target
}

fn leading_zero_bits(digest: &[u8; 32]) -> u16 {
    let mut value = 0_u16;
    for byte in digest {
        if *byte == 0 {
            value += 8;
        } else {
            value += byte.leading_zeros() as u16;
            break;
        }
    }
    value
}

fn encode_msgpack_uint(value: u64) -> ([u8; 9], usize) {
    let mut encoded = [0_u8; 9];
    let length = if value < 128 {
        encoded[0] = value as u8;
        1
    } else if value < 256 {
        encoded[0] = 0xcc;
        encoded[1] = value as u8;
        2
    } else if value < 65_536 {
        encoded[0] = 0xcd;
        encoded[1..3].copy_from_slice(&(value as u16).to_be_bytes());
        3
    } else if value < 4_294_967_296 {
        encoded[0] = 0xce;
        encoded[1..5].copy_from_slice(&(value as u32).to_be_bytes());
        5
    } else {
        encoded[0] = 0xcf;
        encoded[1..9].copy_from_slice(&value.to_be_bytes());
        9
    };
    (encoded, length)
}

#[cfg(test)]
mod tests {
    use super::{encode_msgpack_uint, meets_python_target};

    #[test]
    fn round_encoding_matches_python_width_boundaries() {
        for (value, expected) in [
            (0, &[0x00][..]),
            (127, &[0x7f][..]),
            (128, &[0xcc, 0x80][..]),
            (255, &[0xcc, 0xff][..]),
            (256, &[0xcd, 0x01, 0x00][..]),
            (65_535, &[0xcd, 0xff, 0xff][..]),
            (65_536, &[0xce, 0x00, 0x01, 0x00, 0x00][..]),
        ] {
            let (encoded, length) = encode_msgpack_uint(value);
            assert_eq!(&encoded[..length], expected);
        }
    }

    #[test]
    fn python_target_is_inclusive_at_boundary() {
        let mut equal_cost_eight = [0_u8; 32];
        equal_cost_eight[0] = 0x01;
        assert!(meets_python_target(&equal_cost_eight, 8));
        assert!(!meets_python_target(&equal_cost_eight, 9));
        assert!(meets_python_target(&[0xff; 32], 0));
    }
}
