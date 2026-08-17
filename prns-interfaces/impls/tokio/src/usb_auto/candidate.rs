use std::fmt;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UsbAutoCandidate {
    locator: String,
    identity: CandidateIdentity,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UsbAutoIncarnation(String);

impl UsbAutoIncarnation {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum CandidateIdentity {
    UnclassifiedAttachment { incarnation: UsbAutoIncarnation },
    PrnsSpecific,
}

pub(super) enum HandshakeTimeoutDisposition {
    IgnoreIncarnation,
    Retry,
}

impl UsbAutoCandidate {
    #[must_use]
    pub fn unclassified_attachment(
        locator: impl Into<String>,
        incarnation: UsbAutoIncarnation,
    ) -> Self {
        Self {
            locator: locator.into(),
            identity: CandidateIdentity::UnclassifiedAttachment { incarnation },
        }
    }

    #[must_use]
    pub fn prns_specific(locator: impl Into<String>) -> Self {
        Self {
            locator: locator.into(),
            identity: CandidateIdentity::PrnsSpecific,
        }
    }

    #[must_use]
    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub(super) fn handshake_timeout_disposition(&self) -> HandshakeTimeoutDisposition {
        match &self.identity {
            CandidateIdentity::UnclassifiedAttachment { .. } => {
                HandshakeTimeoutDisposition::IgnoreIncarnation
            }
            CandidateIdentity::PrnsSpecific => HandshakeTimeoutDisposition::Retry,
        }
    }
}

impl fmt::Display for UsbAutoCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.locator)
    }
}
