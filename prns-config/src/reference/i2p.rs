use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum I2pPeerKind {
    Named,
    Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum I2pPeerError {
    Empty,
    UnsafeCharacter(char),
    InvalidDestinationCharacter(char),
    InvalidDestinationLength(usize),
    InvalidDestinationPadding,
}

impl fmt::Display for I2pPeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("is empty"),
            Self::UnsafeCharacter(character) => {
                write!(formatter, "contains unsafe character {character:?}")
            }
            Self::InvalidDestinationCharacter(character) => {
                write!(
                    formatter,
                    "contains invalid I2P base64 character {character:?}"
                )
            }
            Self::InvalidDestinationLength(length) => write!(
                formatter,
                "has {length} characters; an I2P base64 destination must be a multiple of 4"
            ),
            Self::InvalidDestinationPadding => {
                formatter.write_str("has invalid I2P base64 padding")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum I2pPeerListError {
    Invalid {
        position: usize,
        source: I2pPeerError,
    },
    Duplicate {
        position: usize,
        first_position: usize,
    },
}

impl fmt::Display for I2pPeerListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { position, source } => {
                write!(formatter, "I2P peer {position} {source}")
            }
            Self::Duplicate {
                position,
                first_position,
            } => write!(
                formatter,
                "I2P peer {position} duplicates peer {first_position}"
            ),
        }
    }
}

pub(crate) fn validate_peer(value: &str) -> Result<I2pPeerKind, I2pPeerError> {
    validate_safe_value(value)?;
    if value.ends_with(".i2p") {
        return Ok(I2pPeerKind::Named);
    }
    if let Some(character) = value.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '+' | '/' | '-' | '~' | '=')
    }) {
        return Err(I2pPeerError::InvalidDestinationCharacter(character));
    }
    if !value.len().is_multiple_of(4) {
        return Err(I2pPeerError::InvalidDestinationLength(value.len()));
    }
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2 || value.as_bytes()[..value.len() - padding].contains(&b'=') {
        return Err(I2pPeerError::InvalidDestinationPadding);
    }
    Ok(I2pPeerKind::Destination)
}

pub(crate) fn validate_peers<'a>(
    peers: impl IntoIterator<Item = &'a str>,
) -> Result<(), I2pPeerListError> {
    let mut positions = BTreeMap::new();
    for (index, peer) in peers.into_iter().enumerate() {
        let position = index + 1;
        validate_peer(peer).map_err(|source| I2pPeerListError::Invalid { position, source })?;
        if let Some(first_position) = positions.insert(peer, position) {
            return Err(I2pPeerListError::Duplicate {
                position,
                first_position,
            });
        }
    }
    Ok(())
}

fn validate_safe_value(value: &str) -> Result<(), I2pPeerError> {
    if value.is_empty() {
        return Err(I2pPeerError::Empty);
    }
    if let Some(character) = value.chars().find(|character| {
        character.is_whitespace() || character.is_control() || matches!(character, '"' | '\\')
    }) {
        return Err(I2pPeerError::UnsafeCharacter(character));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_validation_matches_the_runtime_intake_contract() {
        assert_eq!(validate_peer("example.i2p"), Ok(I2pPeerKind::Named));
        assert_eq!(validate_peer("QUJDRA=="), Ok(I2pPeerKind::Destination));
        assert!(matches!(
            validate_peer("EXAMPLE.I2P"),
            Err(I2pPeerError::InvalidDestinationCharacter('.'))
        ));
        assert!(matches!(
            validate_peer("abc"),
            Err(I2pPeerError::InvalidDestinationLength(3))
        ));
        assert!(matches!(
            validate_peer("A=AA"),
            Err(I2pPeerError::InvalidDestinationPadding)
        ));
    }

    #[test]
    fn duplicate_peer_positions_are_actionable() {
        assert_eq!(
            validate_peers(["one.i2p", "two.i2p", "one.i2p"]),
            Err(I2pPeerListError::Duplicate {
                position: 3,
                first_position: 1,
            })
        );
    }
}
