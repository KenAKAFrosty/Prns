use std::fmt;

pub(super) const MIN_PUBLIC_DESTINATION_BYTES: usize = 516;
pub(super) const MIN_PRIVATE_DESTINATION_BYTES: usize = 884;

#[derive(Debug, Clone, PartialEq, Eq)]
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
        validate_i2p_destination(
            &value,
            I2pDestinationKind::Public,
            MIN_PUBLIC_DESTINATION_BYTES,
        )?;
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
            MIN_PRIVATE_DESTINATION_BYTES,
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
    minimum: usize,
) -> Result<(), SamValueError> {
    validate_sam_value(value)?;
    if value.len() < minimum {
        return Err(SamValueError::DestinationTooShort {
            kind,
            minimum,
            actual: value.len(),
        });
    }
    if let Some(character) = value.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '-' | '~' | '=')
    }) {
        return Err(SamValueError::InvalidDestinationCharacter { kind, character });
    }
    Ok(())
}
