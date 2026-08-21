use crate::engine::{
    AllowRequesterFailure, AnnounceNowFailure, CloseLinkFailure, EstablishLinkFailure,
    IdentifyFailure, Journaled, LinkClosedReason, RequestPathFailure, RespondFailure,
    RouteRemovalCause, SendGroupFailure, SendRequestFailure, SendResourceFailure,
    SendSinglePacketFailure, SendToChannelFailure, SendToLinkFailure, SetResourceStrategyFailure,
    Settlement,
};
use crate::routing::links::resources::table::ApplyHashmapUpdateError;
use crate::routing::links::resources::ResourceFailureCause;

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RuntimeOperation {
        AnnounceNow,
        SendSinglePacket,
        SendGroup,
        RequestPath,
        EstablishLink,
        SendToLink,
        Identify,
        SendRequest,
        Respond,
        CloseLink,
        SendResource,
        SetResourceStrategy,
        SendToChannel,
        AllowRequester,
    }
}

impl RuntimeOperation {
    const fn index(self) -> usize {
        self as usize
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RuntimeOperationOutcome {
        Succeeded,
        Rejected,
        WriteFailed,
        Timeout,
        Culled,
        PeerRejected,
        Sequencing,
        DependencyFailed,
        Backpressure,
        Untrackable,
        ResponseTooLarge,
    }
}

impl RuntimeOperationOutcome {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOperationCounts {
    counts: [[u64; RuntimeOperationOutcome::ALL.len()]; RuntimeOperation::ALL.len()],
}

impl Default for RuntimeOperationCounts {
    fn default() -> Self {
        Self {
            counts: [[0; RuntimeOperationOutcome::ALL.len()]; RuntimeOperation::ALL.len()],
        }
    }
}

impl RuntimeOperationCounts {
    pub const fn get(&self, operation: RuntimeOperation, outcome: RuntimeOperationOutcome) -> u64 {
        self.counts[operation.index()][outcome.index()]
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (RuntimeOperation, RuntimeOperationOutcome, u64)> + '_ {
        RuntimeOperation::ALL
            .into_iter()
            .flat_map(move |operation| {
                RuntimeOperationOutcome::ALL
                    .into_iter()
                    .map(move |outcome| (operation, outcome, self.get(operation, outcome)))
            })
    }

    fn record(&mut self, operation: RuntimeOperation, outcome: RuntimeOperationOutcome) {
        let count = &mut self.counts[operation.index()][outcome.index()];
        *count = count.saturating_add(1);
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RuntimeResourceFailure {
        CancelledBySender,
        HashmapBeyondPartCount,
        HashmapSkipsAhead,
        HashmapTooLong,
        HashmapRagged,
        RetriesExhausted,
        LinkVanished,
        TransferUnopenable,
        TransferCorrupt,
        ProofUnsendable,
        DecompressionFailed,
        DecompressionTimedOut,
        OpenTimedOut,
        MetadataOverrun,
    }
}

impl RuntimeResourceFailure {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeResourceFailureCounts {
    counts: [u64; RuntimeResourceFailure::ALL.len()],
}

impl Default for RuntimeResourceFailureCounts {
    fn default() -> Self {
        Self {
            counts: [0; RuntimeResourceFailure::ALL.len()],
        }
    }
}

impl RuntimeResourceFailureCounts {
    pub const fn get(&self, failure: RuntimeResourceFailure) -> u64 {
        self.counts[failure.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (RuntimeResourceFailure, u64)> + '_ {
        RuntimeResourceFailure::ALL
            .into_iter()
            .map(|failure| (failure, self.get(failure)))
    }

    fn record(&mut self, failure: RuntimeResourceFailure) {
        let count = &mut self.counts[failure.index()];
        *count = count.saturating_add(1);
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RuntimeLinkClosure {
        Timeout,
        PeerClosed,
        MalformedRtt,
    }
}

impl RuntimeLinkClosure {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeLinkClosureCounts {
    counts: [u64; RuntimeLinkClosure::ALL.len()],
}

impl RuntimeLinkClosureCounts {
    pub const fn get(&self, reason: RuntimeLinkClosure) -> u64 {
        self.counts[reason.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (RuntimeLinkClosure, u64)> + '_ {
        RuntimeLinkClosure::ALL
            .into_iter()
            .map(|reason| (reason, self.get(reason)))
    }

    fn record(&mut self, reason: RuntimeLinkClosure) {
        let count = &mut self.counts[reason.index()];
        *count = count.saturating_add(1);
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RuntimeRouteRemoval {
        Expired,
        Evicted,
        InterfaceGone,
        Dropped,
    }
}

impl RuntimeRouteRemoval {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeRouteRemovalCounts {
    counts: [u64; RuntimeRouteRemoval::ALL.len()],
}

impl RuntimeRouteRemovalCounts {
    pub const fn get(&self, cause: RuntimeRouteRemoval) -> u64 {
        self.counts[cause.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (RuntimeRouteRemoval, u64)> + '_ {
        RuntimeRouteRemoval::ALL
            .into_iter()
            .map(|cause| (cause, self.get(cause)))
    }

    fn record(&mut self, cause: RuntimeRouteRemoval) {
        let count = &mut self.counts[cause.index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReliabilityMetricsSnapshot {
    pub operations: RuntimeOperationCounts,
    pub resource_failures: RuntimeResourceFailureCounts,
    pub link_closures: RuntimeLinkClosureCounts,
    pub link_interface_mismatches: u64,
    pub route_removals: RuntimeRouteRemovalCounts,
}

impl ReliabilityMetricsSnapshot {
    pub fn record_journaled(&mut self, journaled: &Journaled<'_>) {
        match journaled {
            Journaled::PersistenceFlushed { .. } | Journaled::PersistenceFlushFailed { .. } => {}
            Journaled::CommandSettled { settlement, .. } => {
                let settled = SettledOperation::from(settlement);
                self.operations.record(settled.operation, settled.outcome);
            }
            Journaled::LinkClosed { reason, .. } => {
                self.link_closures.record((*reason).into());
            }
            Journaled::LinkInterfaceMismatch { .. } => {
                self.link_interface_mismatches = self.link_interface_mismatches.saturating_add(1);
            }
            Journaled::ResourceFailed { cause, .. } => {
                self.resource_failures.record((*cause).into());
            }
            Journaled::RouteRemoved { cause, .. } => {
                self.route_removals.record((*cause).into());
            }
            Journaled::AnnounceHeard { .. }
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::AnnounceHeldDropped { .. }
            | Journaled::Delivered(_)
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceNeedsDecompression { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. } => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettledOperation {
    operation: RuntimeOperation,
    outcome: RuntimeOperationOutcome,
}

trait RuntimeOutcome {
    fn runtime_outcome(&self) -> RuntimeOperationOutcome;
}

impl<Success, Failure> RuntimeOutcome for Result<Success, Failure>
where
    for<'failure> RuntimeOperationOutcome: From<&'failure Failure>,
{
    fn runtime_outcome(&self) -> RuntimeOperationOutcome {
        match self {
            Ok(_) => RuntimeOperationOutcome::Succeeded,
            Err(failure) => RuntimeOperationOutcome::from(failure),
        }
    }
}

impl From<&Settlement> for SettledOperation {
    fn from(settlement: &Settlement) -> Self {
        use RuntimeOperation as Operation;

        match settlement {
            Settlement::AnnounceNow(result) => Self {
                operation: Operation::AnnounceNow,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendSinglePacket(result) => Self {
                operation: Operation::SendSinglePacket,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendGroup(result) => Self {
                operation: Operation::SendGroup,
                outcome: result.runtime_outcome(),
            },
            Settlement::RequestPath(result) => Self {
                operation: Operation::RequestPath,
                outcome: result.runtime_outcome(),
            },
            Settlement::EstablishLink(result) => Self {
                operation: Operation::EstablishLink,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendToLink(result) => Self {
                operation: Operation::SendToLink,
                outcome: result.runtime_outcome(),
            },
            Settlement::Identify(result) => Self {
                operation: Operation::Identify,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendRequest(result) => Self {
                operation: Operation::SendRequest,
                outcome: result.runtime_outcome(),
            },
            Settlement::Respond(result) => Self {
                operation: Operation::Respond,
                outcome: result.runtime_outcome(),
            },
            Settlement::CloseLink(result) => Self {
                operation: Operation::CloseLink,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendResource(result) => Self {
                operation: Operation::SendResource,
                outcome: result.runtime_outcome(),
            },
            Settlement::SetResourceStrategy(result) => Self {
                operation: Operation::SetResourceStrategy,
                outcome: result.runtime_outcome(),
            },
            Settlement::SendToChannel(result) => Self {
                operation: Operation::SendToChannel,
                outcome: result.runtime_outcome(),
            },
            Settlement::AllowRequester(result) => Self {
                operation: Operation::AllowRequester,
                outcome: result.runtime_outcome(),
            },
        }
    }
}

impl From<&AnnounceNowFailure> for RuntimeOperationOutcome {
    fn from(failure: &AnnounceNowFailure) -> Self {
        match failure {
            AnnounceNowFailure::Rejected(_) => Self::Rejected,
            AnnounceNowFailure::WriteFailed(_) => Self::WriteFailed,
        }
    }
}

impl From<&SendSinglePacketFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendSinglePacketFailure) -> Self {
        match failure {
            SendSinglePacketFailure::Rejected(_) => Self::Rejected,
            SendSinglePacketFailure::WriteFailed(_) => Self::WriteFailed,
            SendSinglePacketFailure::Culled => Self::Culled,
            SendSinglePacketFailure::Timeout => Self::Timeout,
        }
    }
}

impl From<&SendGroupFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendGroupFailure) -> Self {
        match failure {
            SendGroupFailure::Rejected(_) => Self::Rejected,
            SendGroupFailure::WriteFailed(_) => Self::WriteFailed,
        }
    }
}

impl From<&RequestPathFailure> for RuntimeOperationOutcome {
    fn from(failure: &RequestPathFailure) -> Self {
        match failure {
            RequestPathFailure::WriteFailed(_) => Self::WriteFailed,
            RequestPathFailure::Timeout => Self::Timeout,
            RequestPathFailure::Culled => Self::Culled,
        }
    }
}

impl From<&EstablishLinkFailure> for RuntimeOperationOutcome {
    fn from(failure: &EstablishLinkFailure) -> Self {
        match failure {
            EstablishLinkFailure::Rejected(_) => Self::Rejected,
            EstablishLinkFailure::WriteFailed(_) => Self::WriteFailed,
            EstablishLinkFailure::Timeout => Self::Timeout,
        }
    }
}

impl From<&SendToLinkFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendToLinkFailure) -> Self {
        match failure {
            SendToLinkFailure::Rejected(_) => Self::Rejected,
            SendToLinkFailure::WriteFailed(_) => Self::WriteFailed,
            SendToLinkFailure::Culled => Self::Culled,
            SendToLinkFailure::Timeout => Self::Timeout,
        }
    }
}

impl From<&IdentifyFailure> for RuntimeOperationOutcome {
    fn from(failure: &IdentifyFailure) -> Self {
        match failure {
            IdentifyFailure::Rejected(_) => Self::Rejected,
            IdentifyFailure::WriteFailed => Self::WriteFailed,
        }
    }
}

impl From<&SendRequestFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendRequestFailure) -> Self {
        match failure {
            SendRequestFailure::Rejected(_) => Self::Rejected,
            SendRequestFailure::WriteFailed => Self::WriteFailed,
            SendRequestFailure::Culled => Self::Culled,
            SendRequestFailure::Timeout => Self::Timeout,
            SendRequestFailure::ResponseTooLarge => Self::ResponseTooLarge,
            SendRequestFailure::ResourceCapacity => Self::Backpressure,
        }
    }
}

impl From<&RespondFailure> for RuntimeOperationOutcome {
    fn from(failure: &RespondFailure) -> Self {
        match failure {
            RespondFailure::Rejected(_) => Self::Rejected,
            RespondFailure::WriteFailed => Self::WriteFailed,
            RespondFailure::Resource(inner) => Self::from(inner),
        }
    }
}

impl From<&CloseLinkFailure> for RuntimeOperationOutcome {
    fn from(failure: &CloseLinkFailure) -> Self {
        match failure {
            CloseLinkFailure::Rejected(_) => Self::Rejected,
            CloseLinkFailure::WriteFailed => Self::WriteFailed,
        }
    }
}

impl From<&SendResourceFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendResourceFailure) -> Self {
        match failure {
            SendResourceFailure::Rejected(_) => Self::Rejected,
            SendResourceFailure::WriteFailed => Self::WriteFailed,
            SendResourceFailure::RejectedByPeer => Self::PeerRejected,
            SendResourceFailure::Sequencing => Self::Sequencing,
            SendResourceFailure::Timeout => Self::Timeout,
            SendResourceFailure::PredecessorFailed => Self::DependencyFailed,
        }
    }
}

impl From<&SetResourceStrategyFailure> for RuntimeOperationOutcome {
    fn from(failure: &SetResourceStrategyFailure) -> Self {
        match failure {
            SetResourceStrategyFailure::Rejected(_) => Self::Rejected,
        }
    }
}

impl From<&SendToChannelFailure> for RuntimeOperationOutcome {
    fn from(failure: &SendToChannelFailure) -> Self {
        match failure {
            SendToChannelFailure::Rejected(_) => Self::Rejected,
            SendToChannelFailure::WriteFailed(_) => Self::WriteFailed,
            SendToChannelFailure::WindowFull => Self::Backpressure,
            SendToChannelFailure::Untrackable => Self::Untrackable,
            SendToChannelFailure::Timeout => Self::Timeout,
        }
    }
}

impl From<&AllowRequesterFailure> for RuntimeOperationOutcome {
    fn from(failure: &AllowRequesterFailure) -> Self {
        match failure {
            AllowRequesterFailure::Rejected(_) => Self::Rejected,
        }
    }
}

impl From<ResourceFailureCause> for RuntimeResourceFailure {
    fn from(cause: ResourceFailureCause) -> Self {
        match cause {
            ResourceFailureCause::CancelledBySender => Self::CancelledBySender,
            ResourceFailureCause::RefusedHashmapUpdate(refusal) => match refusal {
                ApplyHashmapUpdateError::BeyondPartCount => Self::HashmapBeyondPartCount,
                ApplyHashmapUpdateError::SkipsAhead => Self::HashmapSkipsAhead,
                ApplyHashmapUpdateError::HashmapTooLong => Self::HashmapTooLong,
                ApplyHashmapUpdateError::HashmapRagged => Self::HashmapRagged,
            },
            ResourceFailureCause::RetriesExhausted => Self::RetriesExhausted,
            ResourceFailureCause::LinkVanished => Self::LinkVanished,
            ResourceFailureCause::TransferUnopenable => Self::TransferUnopenable,
            ResourceFailureCause::TransferCorrupt => Self::TransferCorrupt,
            ResourceFailureCause::ProofUnsendable => Self::ProofUnsendable,
            ResourceFailureCause::DecompressionFailed => Self::DecompressionFailed,
            ResourceFailureCause::DecompressionTimedOut => Self::DecompressionTimedOut,
            ResourceFailureCause::OpenTimedOut => Self::OpenTimedOut,
            ResourceFailureCause::MetadataOverrun => Self::MetadataOverrun,
        }
    }
}

impl From<LinkClosedReason> for RuntimeLinkClosure {
    fn from(reason: LinkClosedReason) -> Self {
        match reason {
            LinkClosedReason::Timeout => Self::Timeout,
            LinkClosedReason::PeerClosed => Self::PeerClosed,
            LinkClosedReason::MalformedRtt => Self::MalformedRtt,
        }
    }
}

impl From<RouteRemovalCause> for RuntimeRouteRemoval {
    fn from(cause: RouteRemovalCause) -> Self {
        match cause {
            RouteRemovalCause::Expired => Self::Expired,
            RouteRemovalCause::Evicted => Self::Evicted,
            RouteRemovalCause::InterfaceGone => Self::InterfaceGone,
            RouteRemovalCause::Dropped => Self::Dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{CommandId, SendRequestFailure, SendResourceFailure};

    #[test]
    fn journaled_command_settlements_are_counted_before_delivery() {
        let mut snapshot = ReliabilityMetricsSnapshot::default();
        snapshot.record_journaled(&Journaled::CommandSettled {
            id: CommandId(1),
            settlement: Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
        });
        snapshot.record_journaled(&Journaled::CommandSettled {
            id: CommandId(2),
            settlement: Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
        });

        assert_eq!(
            snapshot.operations.get(
                RuntimeOperation::SendRequest,
                RuntimeOperationOutcome::Timeout
            ),
            1
        );
        assert_eq!(
            snapshot.operations.get(
                RuntimeOperation::SendResource,
                RuntimeOperationOutcome::PeerRejected
            ),
            1
        );
    }

    #[test]
    fn response_resource_capacity_is_reported_as_backpressure() {
        assert_eq!(
            RuntimeOperationOutcome::from(&SendRequestFailure::ResourceCapacity),
            RuntimeOperationOutcome::Backpressure,
        );
    }

    #[test]
    fn bounded_reliability_dimensions_cover_every_named_value() {
        assert_eq!(
            RuntimeOperation::ALL.len() * RuntimeOperationOutcome::ALL.len(),
            RuntimeOperationCounts::default().iter().count()
        );
        assert_eq!(
            RuntimeResourceFailure::ALL.len(),
            RuntimeResourceFailureCounts::default().iter().count()
        );
        assert_eq!(
            RuntimeLinkClosure::ALL.len(),
            RuntimeLinkClosureCounts::default().iter().count()
        );
        assert_eq!(
            RuntimeRouteRemoval::ALL.len(),
            RuntimeRouteRemovalCounts::default().iter().count()
        );
    }

    #[test]
    fn nested_resource_and_maintenance_causes_keep_their_diagnostic_shape() {
        assert_eq!(
            RuntimeResourceFailure::from(ResourceFailureCause::RefusedHashmapUpdate(
                ApplyHashmapUpdateError::SkipsAhead
            )),
            RuntimeResourceFailure::HashmapSkipsAhead
        );
        assert_eq!(
            RuntimeLinkClosure::from(LinkClosedReason::MalformedRtt),
            RuntimeLinkClosure::MalformedRtt
        );
        assert_eq!(
            RuntimeRouteRemoval::from(RouteRemovalCause::InterfaceGone),
            RuntimeRouteRemoval::InterfaceGone
        );
        assert_eq!(
            RuntimeRouteRemoval::from(RouteRemovalCause::Dropped),
            RuntimeRouteRemoval::Dropped
        );
    }
}
