use alloc::vec::Vec;

use crate::crypto::{hkdf_sha256, sha256, sha256_chunks};

pub const STAMP_SIZE: usize = 32;
pub const WORKBLOCK_EXPAND_ROUNDS: usize = 20;
pub const DEFAULT_STAMP_COST: StampCost = StampCost::new_const(14);

const WORKBLOCK_FRAGMENT_SIZE: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub struct AdvertisementHash([u8; 32]);

impl AdvertisementHash {
    pub fn for_advertisement(packed_advertisement: &[u8]) -> Self {
        Self(sha256(packed_advertisement))
    }

    pub const fn from_hash(hash: [u8; 32]) -> Self {
        Self(hash)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StampCost(u8);

impl StampCost {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = u8::MAX;

    pub const fn new(value: u16) -> Result<Self, StampCostError> {
        if value == 0 {
            return Err(StampCostError::Zero);
        }
        if value > Self::MAX as u16 {
            return Err(StampCostError::ExceedsMaximum { value });
        }
        Ok(Self(value as u8))
    }

    pub const fn new_const(value: u8) -> Self {
        assert!(value >= Self::MIN);
        Self(value)
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StampCostError {
    Zero,
    ExceedsMaximum { value: u16 },
}

impl core::fmt::Display for StampCostError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Zero => formatter.write_str("stamp cost cannot be zero"),
            Self::ExceedsMaximum { value } => {
                write!(formatter, "stamp cost {value} exceeds the maximum of 255")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StampCostError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StampValue(u16);

impl StampValue {
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StampValidation {
    MeetsCost {
        value: StampValue,
    },
    BelowCost {
        value: StampValue,
        required: StampCost,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct GeneratedStamp {
    pub stamp: [u8; STAMP_SIZE],
    pub value: StampValue,
    pub attempts: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StampGeneration<E> {
    Generated(GeneratedStamp),
    Cancelled,
    EntropyFailure(E),
}

pub fn validate_stamp(
    advertisement_hash: &AdvertisementHash,
    stamp: &[u8; STAMP_SIZE],
    cost: StampCost,
) -> StampValidation {
    let value = stamp_value(advertisement_hash, stamp);
    if value.get() >= u16::from(cost.get()) {
        StampValidation::MeetsCost { value }
    } else {
        StampValidation::BelowCost {
            value,
            required: cost,
        }
    }
}

pub fn stamp_value(advertisement_hash: &AdvertisementHash, stamp: &[u8; STAMP_SIZE]) -> StampValue {
    let workblock = workblock(advertisement_hash);
    StampValue(leading_zero_bits(sha256_chunks(&[&workblock, stamp])))
}

pub fn generate_stamp<E>(
    advertisement_hash: &AdvertisementHash,
    cost: StampCost,
    mut fill_entropy: impl FnMut(&mut [u8; STAMP_SIZE]) -> Result<(), E>,
    mut cancelled: impl FnMut() -> bool,
) -> StampGeneration<E> {
    let workblock = workblock(advertisement_hash);
    let mut attempts = 0u64;
    loop {
        if cancelled() {
            return StampGeneration::Cancelled;
        }
        let mut candidate = [0u8; STAMP_SIZE];
        if let Err(error) = fill_entropy(&mut candidate) {
            return StampGeneration::EntropyFailure(error);
        }
        attempts = attempts.saturating_add(1);
        let value = StampValue(leading_zero_bits(sha256_chunks(&[&workblock, &candidate])));
        if value.get() >= u16::from(cost.get()) {
            return StampGeneration::Generated(GeneratedStamp {
                stamp: candidate,
                value,
                attempts,
            });
        }
    }
}

fn workblock(advertisement_hash: &AdvertisementHash) -> Vec<u8> {
    let mut workblock = Vec::with_capacity(WORKBLOCK_FRAGMENT_SIZE * WORKBLOCK_EXPAND_ROUNDS);
    for round in 0..WORKBLOCK_EXPAND_ROUNDS {
        let encoded_round = [round as u8];
        let salt = sha256_chunks(&[advertisement_hash.as_bytes(), &encoded_round]);
        workblock.extend_from_slice(&hkdf_sha256::<WORKBLOCK_FRAGMENT_SIZE>(
            advertisement_hash.as_bytes(),
            &salt,
            &[],
        ));
    }
    workblock
}

fn leading_zero_bits(digest: [u8; 32]) -> u16 {
    let mut value = 0;
    for byte in digest {
        value += byte.leading_zeros() as u16;
        if byte != 0 {
            break;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_from_hex<const N: usize>(hex: &str) -> [u8; N] {
        let mut bytes = [0u8; N];
        assert_eq!(hex.len(), N * 2);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }

    fn stamp_cost(value: u16) -> StampCost {
        match StampCost::new(value) {
            Ok(cost) => cost,
            Err(error) => panic!("unexpected stamp cost error: {error}"),
        }
    }

    #[test]
    fn rns_lxmf_stamp_vector_validates_at_its_exact_value() {
        let packed = bytes_from_hex::<124>(
            "8b00b14261636b626f6e65496e7465726661636501c3ccfec41000112233445566778899aabbccddeeffccffaf5075626c6963204261636b626f6e6503cb402900000000000004cbc04120000000000005cb405ec0000000000002ae726f757465722e6578616d706c6506cd109207a46d65736808a6736563726574",
        );
        let advertisement_hash = AdvertisementHash::for_advertisement(&packed);
        assert_eq!(
            advertisement_hash.as_bytes(),
            &bytes_from_hex::<32>(
                "cef9e0fd856aa11474a62d85d02f5e464d56feb83949ee43b010444a7ce5e832",
            )
        );
        let stamp = bytes_from_hex::<32>(
            "00000000000000000000000000000000000000000000000000000000000000b6",
        );
        assert_eq!(
            validate_stamp(&advertisement_hash, &stamp, stamp_cost(8)),
            StampValidation::MeetsCost {
                value: StampValue(8),
            },
        );
        assert_eq!(
            validate_stamp(&advertisement_hash, &stamp, stamp_cost(9)),
            StampValidation::BelowCost {
                value: StampValue(8),
                required: stamp_cost(9),
            },
        );
        assert_eq!(stamp_value(&advertisement_hash, &stamp), StampValue(8));
    }

    #[test]
    fn deterministic_generation_reports_attempts_and_honors_cancellation() {
        let advertisement_hash = AdvertisementHash::from_hash([0x42; 32]);
        let mut candidate = 0u64;
        let generated = generate_stamp(
            &advertisement_hash,
            stamp_cost(4),
            |bytes| {
                bytes.fill(0);
                bytes[24..].copy_from_slice(&candidate.to_be_bytes());
                candidate += 1;
                Ok::<_, core::convert::Infallible>(())
            },
            || false,
        );
        let StampGeneration::Generated(generated) = generated else {
            panic!("expected a generated stamp");
        };
        assert!(generated.value.get() >= 4);
        assert_eq!(generated.attempts, candidate);

        let cancelled = generate_stamp(
            &advertisement_hash,
            stamp_cost(4),
            |_| Ok::<_, core::convert::Infallible>(()),
            || true,
        );
        assert_eq!(cancelled, StampGeneration::Cancelled);
    }

    #[test]
    fn stamp_cost_rejects_values_the_reference_cannot_evaluate() {
        assert_eq!(StampCost::new(0), Err(StampCostError::Zero));
        assert_eq!(
            StampCost::new(256),
            Err(StampCostError::ExceedsMaximum { value: 256 }),
        );
    }

    #[test]
    fn all_zero_digest_has_the_full_bit_value() {
        assert_eq!(leading_zero_bits([0; 32]), 256);
    }
}
