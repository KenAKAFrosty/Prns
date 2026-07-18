use std::fmt;

use super::sam::{SamSessionId, SamValueError};

const SESSION_PREFIX: &str = "reticulum-";
const ASCII_LETTERS: &[u8; 52] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const UNBIASED_BYTE_CEILING: u8 = 208;

#[derive(Debug)]
pub enum I2pSessionIdError {
    Entropy(getrandom::Error),
    Invalid(SamValueError),
}

impl fmt::Display for I2pSessionIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entropy(error) => {
                write!(formatter, "could not generate an I2P session ID: {error}")
            }
            Self::Invalid(error) => {
                write!(formatter, "generated an invalid I2P session ID: {error}")
            }
        }
    }
}

impl std::error::Error for I2pSessionIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Entropy(_) => None,
            Self::Invalid(error) => Some(error),
        }
    }
}

pub fn generate_session_id() -> Result<SamSessionId, I2pSessionIdError> {
    let mut suffix = [0u8; 6];
    let mut filled = 0;
    let mut random = [0u8; 12];
    while filled < suffix.len() {
        getrandom::getrandom(&mut random).map_err(I2pSessionIdError::Entropy)?;
        for byte in random {
            if byte >= UNBIASED_BYTE_CEILING {
                continue;
            }
            suffix[filled] = ASCII_LETTERS[usize::from(byte % ASCII_LETTERS.len() as u8)];
            filled += 1;
            if filled == suffix.len() {
                break;
            }
        }
    }
    let mut value = String::with_capacity(SESSION_PREFIX.len() + suffix.len());
    value.push_str(SESSION_PREFIX);
    value.extend(suffix.into_iter().map(char::from));
    SamSessionId::new(value).map_err(I2pSessionIdError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_match_the_reference_shape() {
        for _ in 0..128 {
            let id = generate_session_id().expect("host entropy is available");
            let suffix = id
                .as_str()
                .strip_prefix("reticulum-")
                .expect("the reference prefix is present");
            assert_eq!(suffix.len(), 6);
            assert!(suffix.bytes().all(|byte| byte.is_ascii_alphabetic()));
        }
    }
}
