use crate::routing::ingress::IgnoreReason;
use crate::routing::links::handshake::LinkRttError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IgnoreReasonKind {
    Consumed,
    Malformed,
    UnhandledContext,
    Duplicate,
    Superseded,
    NotForUs,
    NoRoute,
    HopLimitReached,
    LoopPrevented,
    RouteUnresponsive,
    OtherInstance,
    UnknownLink,
    LinkPhaseMismatch,
    LinkRttMalformed,
    LinkRttInvalidToken,
    LinkRttBufferTooShort,
    DecryptFailed,
    ProofInvalid,
    UnknownIdentity,
    LinkRequestsRefused,
    PermissionDenied,
    RateLimited,
    CapacityExhausted,
    StrategyDeclined,
    UnmatchedResponse,
    IfacRefused,
}

impl IgnoreReasonKind {
    pub const ALL: [Self; 26] = [
        Self::Consumed,
        Self::Malformed,
        Self::UnhandledContext,
        Self::Duplicate,
        Self::Superseded,
        Self::NotForUs,
        Self::NoRoute,
        Self::HopLimitReached,
        Self::LoopPrevented,
        Self::RouteUnresponsive,
        Self::OtherInstance,
        Self::UnknownLink,
        Self::LinkPhaseMismatch,
        Self::LinkRttMalformed,
        Self::LinkRttInvalidToken,
        Self::LinkRttBufferTooShort,
        Self::DecryptFailed,
        Self::ProofInvalid,
        Self::UnknownIdentity,
        Self::LinkRequestsRefused,
        Self::PermissionDenied,
        Self::RateLimited,
        Self::CapacityExhausted,
        Self::StrategyDeclined,
        Self::UnmatchedResponse,
        Self::IfacRefused,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

impl From<IgnoreReason> for IgnoreReasonKind {
    fn from(reason: IgnoreReason) -> Self {
        match reason {
            IgnoreReason::Consumed => Self::Consumed,
            IgnoreReason::Malformed => Self::Malformed,
            IgnoreReason::UnhandledContext => Self::UnhandledContext,
            IgnoreReason::Duplicate => Self::Duplicate,
            IgnoreReason::Superseded => Self::Superseded,
            IgnoreReason::NotForUs => Self::NotForUs,
            IgnoreReason::NoRoute => Self::NoRoute,
            IgnoreReason::HopLimitReached => Self::HopLimitReached,
            IgnoreReason::LoopPrevented => Self::LoopPrevented,
            IgnoreReason::RouteUnresponsive => Self::RouteUnresponsive,
            IgnoreReason::OtherInstance => Self::OtherInstance,
            IgnoreReason::UnknownLink => Self::UnknownLink,
            IgnoreReason::LinkPhaseMismatch => Self::LinkPhaseMismatch,
            IgnoreReason::LinkRttError(LinkRttError::Malformed) => Self::LinkRttMalformed,
            IgnoreReason::LinkRttError(LinkRttError::InvalidToken) => Self::LinkRttInvalidToken,
            IgnoreReason::LinkRttError(LinkRttError::BufferTooShort) => Self::LinkRttBufferTooShort,
            IgnoreReason::DecryptFailed => Self::DecryptFailed,
            IgnoreReason::ProofInvalid => Self::ProofInvalid,
            IgnoreReason::UnknownIdentity => Self::UnknownIdentity,
            IgnoreReason::LinkRequestsRefused => Self::LinkRequestsRefused,
            IgnoreReason::PermissionDenied => Self::PermissionDenied,
            IgnoreReason::RateLimited => Self::RateLimited,
            IgnoreReason::CapacityExhausted => Self::CapacityExhausted,
            IgnoreReason::StrategyDeclined => Self::StrategyDeclined,
            IgnoreReason::UnmatchedResponse => Self::UnmatchedResponse,
            IgnoreReason::IfacRefused => Self::IfacRefused,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IgnoreReasonCounts {
    counts: [u64; IgnoreReasonKind::ALL.len()],
}

impl Default for IgnoreReasonCounts {
    fn default() -> Self {
        Self {
            counts: [0; IgnoreReasonKind::ALL.len()],
        }
    }
}

impl IgnoreReasonCounts {
    pub const fn get(&self, reason: IgnoreReasonKind) -> u64 {
        self.counts[reason.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (IgnoreReasonKind, u64)> + '_ {
        IgnoreReasonKind::ALL
            .into_iter()
            .map(|reason| (reason, self.get(reason)))
    }

    pub fn total(&self) -> u64 {
        self.counts
            .iter()
            .fold(0u64, |total, count| total.saturating_add(*count))
    }

    pub(crate) fn record(&mut self, reason: IgnoreReason) {
        let count = &mut self.counts[IgnoreReasonKind::from(reason).index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineMetricsSnapshot {
    pub ingested_packets: u64,
    pub ingested_commands: u64,
    pub ignored_packets: IgnoreReasonCounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ignore_reason_has_one_stable_counter() {
        let reasons = [
            IgnoreReason::Consumed,
            IgnoreReason::Malformed,
            IgnoreReason::UnhandledContext,
            IgnoreReason::Duplicate,
            IgnoreReason::Superseded,
            IgnoreReason::NotForUs,
            IgnoreReason::NoRoute,
            IgnoreReason::HopLimitReached,
            IgnoreReason::LoopPrevented,
            IgnoreReason::RouteUnresponsive,
            IgnoreReason::OtherInstance,
            IgnoreReason::UnknownLink,
            IgnoreReason::LinkPhaseMismatch,
            IgnoreReason::LinkRttError(LinkRttError::Malformed),
            IgnoreReason::LinkRttError(LinkRttError::InvalidToken),
            IgnoreReason::LinkRttError(LinkRttError::BufferTooShort),
            IgnoreReason::DecryptFailed,
            IgnoreReason::ProofInvalid,
            IgnoreReason::UnknownIdentity,
            IgnoreReason::LinkRequestsRefused,
            IgnoreReason::PermissionDenied,
            IgnoreReason::RateLimited,
            IgnoreReason::CapacityExhausted,
            IgnoreReason::StrategyDeclined,
            IgnoreReason::UnmatchedResponse,
            IgnoreReason::IfacRefused,
        ];
        let mut counts = IgnoreReasonCounts::default();
        for reason in reasons {
            counts.record(reason);
        }
        let recorded = counts.iter().collect::<std::vec::Vec<_>>();
        let expected = IgnoreReasonKind::ALL
            .into_iter()
            .map(|reason| (reason, 1))
            .collect::<std::vec::Vec<_>>();
        assert_eq!(recorded, expected);
        assert_eq!(counts.total(), IgnoreReasonKind::ALL.len() as u64);
    }
}
