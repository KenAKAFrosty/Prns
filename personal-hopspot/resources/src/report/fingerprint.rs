use std::fmt;

use prns_flash_manifest::Sha256Digest;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint(String);

impl Fingerprint {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn parse(value: String) -> Result<Self, prns_flash_manifest::DomainValueError> {
        Sha256Digest::parse(value).map(|digest| Self(digest.as_str().to_string()))
    }
}

impl Serialize for Fingerprint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Sha256Digest::parse(value)
            .map(|digest| Self(digest.as_str().to_string()))
            .map_err(D::Error::custom)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub(super) fn fingerprint(value: &impl Serialize) -> Result<Fingerprint, serde_json::Error> {
    serde_json::to_vec(value).map(|bytes| Fingerprint(prns_flash_manifest::sha256_hex(&bytes)))
}
