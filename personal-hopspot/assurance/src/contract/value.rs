use std::fmt;

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    EvidenceFingerprint,
    EvidencePath,
}

impl fmt::Display for ValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EvidenceFingerprint => "evidence fingerprint",
            Self::EvidencePath => "evidence path",
        })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid {kind} {value:?}")]
pub struct ValueError {
    kind: ValueKind,
    value: String,
}

macro_rules! value {
    ($name:ident, $kind:expr, $valid:expr) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
                let value = value.into();
                if ($valid)(&value) {
                    Ok(Self(value))
                } else {
                    Err(ValueError { kind: $kind, value })
                }
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
                let raw = String::deserialize(deserializer)?;
                Self::parse(raw).map_err(D::Error::custom)
            }
        }
    };
}

fn fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && value.split('/').all(|component| {
            !component.is_empty()
                && !matches!(component, "." | "..")
                && component.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
                })
        })
}

value!(
    EvidenceFingerprint,
    ValueKind::EvidenceFingerprint,
    fingerprint
);
value!(EvidencePath, ValueKind::EvidencePath, path);

#[cfg(test)]
mod tests {
    use super::{EvidenceFingerprint, EvidencePath};

    #[test]
    fn serialized_values_validate_at_the_boundary() {
        assert!(EvidenceFingerprint::parse("a".repeat(64)).is_ok());
        assert!(EvidenceFingerprint::parse("A".repeat(64)).is_err());
        assert!(EvidencePath::parse("results/runner/transcript.log").is_ok());
        assert!(EvidencePath::parse("../transcript.log").is_err());
    }
}
