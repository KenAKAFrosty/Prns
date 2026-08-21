use std::time::Duration;

use personal_rns::identity::IdentityHash;
use personal_rns::wire::{DestinationHash, TransportId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RnsHashArgument([u8; 16]);

impl RnsHashArgument {
    pub const fn destination(self) -> DestinationHash {
        DestinationHash::new(self.0)
    }

    pub const fn identity(self) -> IdentityHash {
        IdentityHash::new(self.0)
    }

    pub const fn transport(self) -> TransportId {
        TransportId::new(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositiveDuration(Duration);

impl PositiveDuration {
    pub const fn get(self) -> Duration {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonnegativeDuration(Duration);

impl NonnegativeDuration {
    pub const fn get(self) -> Duration {
        self.0
    }
}

pub fn parse_positive_duration(value: &str) -> Result<PositiveDuration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number of seconds"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!(
            "{value:?} must be a finite number greater than zero"
        ));
    }
    Ok(PositiveDuration(Duration::from_secs_f64(seconds)))
}

pub fn parse_nonnegative_duration(value: &str) -> Result<NonnegativeDuration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number of seconds"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!(
            "{value:?} must be a finite number greater than or equal to zero"
        ));
    }
    Ok(NonnegativeDuration(Duration::from_secs_f64(seconds)))
}

pub fn parse_identity_hash(value: &str) -> Result<IdentityHash, String> {
    parse_hash(value).map(IdentityHash::new)
}

pub fn parse_hash_argument(value: &str) -> Result<RnsHashArgument, String> {
    parse_hash(value).map(RnsHashArgument)
}

fn parse_hash(value: &str) -> Result<[u8; 16], String> {
    if value.len() != 32 {
        return Err(format!(
            "{value:?} must contain exactly 32 hexadecimal characters (16 bytes)"
        ));
    }
    let mut bytes = [0u8; 16];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair =
            std::str::from_utf8(pair).map_err(|_| format!("{value:?} is not hexadecimal"))?;
        bytes[index] =
            u8::from_str_radix(pair, 16).map_err(|_| format!("{value:?} is not hexadecimal"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_parsers_preserve_semantic_types() {
        let value = "00112233445566778899aabbccddeeff";
        assert_eq!(
            parse_hash_argument(value).map(|hash| *hash.destination().as_bytes()),
            Ok([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ])
        );
        assert!(parse_identity_hash("0011").is_err());
        assert!(parse_hash_argument("zz112233445566778899aabbccddeeff").is_err());
    }
}
