use std::fmt;

pub const I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES: usize = 387;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pDestinationKind {
    Public,
    Private,
}

impl fmt::Display for I2pDestinationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Public => formatter.write_str("public"),
            Self::Private => formatter.write_str("private"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamValueError {
    Empty,
    UnsafeCharacter(char),
    DestinationTooShort {
        kind: I2pDestinationKind,
        minimum: usize,
        actual: usize,
    },
    InvalidDestinationCharacter {
        kind: I2pDestinationKind,
        character: char,
    },
    InvalidDestinationLength {
        kind: I2pDestinationKind,
        length: usize,
    },
    InvalidDestinationPadding {
        kind: I2pDestinationKind,
    },
}

impl fmt::Display for SamValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("SAM value is empty"),
            Self::UnsafeCharacter(character) => {
                write!(
                    formatter,
                    "SAM value contains unsafe character {character:?}"
                )
            }
            Self::DestinationTooShort {
                kind,
                minimum,
                actual,
            } => write!(
                formatter,
                "I2P {kind} destination is {actual} bytes; expected at least {minimum}"
            ),
            Self::InvalidDestinationCharacter { kind, character } => write!(
                formatter,
                "I2P {kind} destination contains invalid base64 character {character:?}"
            ),
            Self::InvalidDestinationLength { kind, length } => write!(
                formatter,
                "I2P {kind} destination has invalid base64 length {length}"
            ),
            Self::InvalidDestinationPadding { kind } => {
                write!(
                    formatter,
                    "I2P {kind} destination has invalid base64 padding"
                )
            }
        }
    }
}

impl std::error::Error for SamValueError {}

macro_rules! sam_token {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
                let value = value.into();
                validate_sam_value(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = SamValueError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

sam_token!(SamSessionId);
sam_token!(I2pAddress);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pPublicDestination(String);

impl I2pPublicDestination {
    pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
        let value = value.into();
        validate_i2p_destination(&value, I2pDestinationKind::Public, None)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for I2pPublicDestination {
    type Error = SamValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct I2pPrivateDestination(String);

impl I2pPrivateDestination {
    pub fn new(value: impl Into<String>) -> Result<Self, SamValueError> {
        let value = value.into();
        validate_i2p_destination(
            &value,
            I2pDestinationKind::Private,
            Some(I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES),
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for I2pPrivateDestination {
    type Error = SamValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Debug for I2pPrivateDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("I2pPrivateDestination")
            .field(&"[REDACTED]")
            .finish()
    }
}

fn validate_sam_value(value: &str) -> Result<(), SamValueError> {
    if value.is_empty() {
        return Err(SamValueError::Empty);
    }
    if let Some(character) = value.chars().find(|character| {
        character.is_whitespace() || character.is_control() || matches!(character, '"' | '\\')
    }) {
        return Err(SamValueError::UnsafeCharacter(character));
    }
    Ok(())
}

fn validate_i2p_destination(
    value: &str,
    kind: I2pDestinationKind,
    minimum_decoded_bytes: Option<usize>,
) -> Result<(), SamValueError> {
    validate_sam_value(value)?;
    if let Some(character) = value.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '+' | '/' | '-' | '~' | '=')
    }) {
        return Err(SamValueError::InvalidDestinationCharacter { kind, character });
    }
    if !value.len().is_multiple_of(4) {
        return Err(SamValueError::InvalidDestinationLength {
            kind,
            length: value.len(),
        });
    }
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2 || value.as_bytes()[..value.len() - padding].contains(&b'=') {
        return Err(SamValueError::InvalidDestinationPadding { kind });
    }
    let decoded_bytes = value.len() / 4 * 3 - padding;
    if let Some(minimum) = minimum_decoded_bytes {
        if decoded_bytes < minimum {
            return Err(SamValueError::DestinationTooShort {
                kind,
                minimum,
                actual: decoded_bytes,
            });
        }
    }
    Ok(())
}
