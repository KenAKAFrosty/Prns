use std::num::{NonZeroU64, NonZeroU8};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub(crate) fn other(&self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
    pub(crate) fn directory(&self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateId {
    pub revision: NonZeroU64,
    pub executable_sha256: ExecutableDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExecutableDigest(String);

impl ExecutableDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExecutableDigest {
    type Error = prns_flash_manifest::DomainValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        prns_flash_manifest::Sha256Digest::parse(&value)?;
        Ok(Self(value))
    }
}

impl From<ExecutableDigest> for String {
    fn from(value: ExecutableDigest) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Fallback {
    NoneInstalled,
    Confirmed { slot: Slot, candidate: CandidateId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Status {
    Uninstalled,
    Confirmed {
        slot: Slot,
        candidate: CandidateId,
    },
    Trial {
        slot: Slot,
        candidate: CandidateId,
        fallback: Fallback,
        launches_remaining: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub(crate) revision: u64,
    pub(crate) status: Status,
}

#[derive(Debug, Clone)]
pub struct LaunchBudget(NonZeroU8);

impl LaunchBudget {
    pub fn new(launches: NonZeroU8) -> Self {
        Self(launches)
    }
    pub(crate) fn get(&self) -> u8 {
        self.0.get()
    }
}

impl Activation {
    pub(crate) fn empty() -> Self {
        Self {
            revision: 0,
            status: Status::Uninstalled,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn status(&self) -> &Status {
        &self.status
    }

    pub(crate) fn is_consistent(&self) -> bool {
        match &self.status {
            Status::Uninstalled => true,
            Status::Confirmed { candidate, .. } => candidate.revision.get() <= self.revision,
            Status::Trial {
                slot,
                candidate,
                fallback,
                ..
            } => {
                if candidate.revision.get() > self.revision {
                    return false;
                }
                match fallback {
                    Fallback::NoneInstalled => true,
                    Fallback::Confirmed {
                        slot: previous,
                        candidate: old,
                    } => previous != slot && old.revision < candidate.revision,
                }
            }
        }
    }
}
