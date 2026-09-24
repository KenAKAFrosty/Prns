use std::fmt;

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifierKind {
    Architecture,
    Component,
    Platform,
    Runner,
    Scenario,
    Target,
}

impl fmt::Display for IdentifierKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Architecture => "architecture",
            Self::Component => "component",
            Self::Platform => "platform",
            Self::Runner => "runner",
            Self::Scenario => "scenario",
            Self::Target => "target",
        })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid {kind} identifier {value:?}")]
pub struct IdentifierError {
    kind: IdentifierKind,
    value: String,
}

fn validate(kind: IdentifierKind, value: String) -> Result<String, IdentifierError> {
    let valid = !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.as_bytes().windows(2).any(|pair| pair == b"--");
    if valid {
        Ok(value)
    } else {
        Err(IdentifierError { kind, value })
    }
}

macro_rules! identifier {
    ($name:ident, $kind:expr) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentifierError> {
                validate($kind, value.into()).map(Self)
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(D::Error::custom)
            }
        }
    };
}

identifier!(ArchitectureId, IdentifierKind::Architecture);
identifier!(ComponentId, IdentifierKind::Component);
identifier!(PlatformId, IdentifierKind::Platform);
identifier!(RunnerId, IdentifierKind::Runner);
identifier!(ScenarioId, IdentifierKind::Scenario);
identifier!(TargetId, IdentifierKind::Target);

#[cfg(test)]
mod tests {
    use super::{IdentifierKind, ScenarioId};

    #[test]
    fn identifiers_accept_canonical_kebab_case() -> Result<(), super::IdentifierError> {
        let id = ScenarioId::parse("sx126x-state-machine")?;
        assert_eq!(id.as_str(), "sx126x-state-machine");
        Ok(())
    }

    #[test]
    fn identifiers_reject_ambiguous_shapes() {
        for value in ["", "Upper", "two--segments", "-leading", "trailing-"] {
            assert!(matches!(
                ScenarioId::parse(value),
                Err(super::IdentifierError {
                    kind: IdentifierKind::Scenario,
                    ..
                })
            ));
        }
    }
}
