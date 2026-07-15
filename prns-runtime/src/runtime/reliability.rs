use crate::engine::{
    AllowRequesterFailure, AnnounceNowFailure, CloseLinkFailure, EstablishLinkFailure,
    IdentifyFailure, Journaled, LinkClosedReason, RequestPathFailure, RespondFailure,
    RouteRemovalCause, SendGroupFailure, SendRequestFailure, SendResourceFailure,
    SendSinglePacketFailure, SendToChannelFailure, SendToLinkFailure, SetResourceStrategyFailure,
    Settlement,
};
use crate::routing::links::resources::table::ApplyHashmapUpdateError;
use crate::routing::links::resources::ResourceFailureCause;

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
    RpcQuery,
}

impl RuntimeOperation {
    pub const ALL: [Self; 15] = [
        Self::AnnounceNow,
        Self::SendSinglePacket,
        Self::SendGroup,
        Self::RequestPath,
        Self::EstablishLink,
        Self::SendToLink,
        Self::Identify,
        Self::SendRequest,
        Self::Respond,
        Self::CloseLink,
        Self::SendResource,
        Self::SetResourceStrategy,
        Self::SendToChannel,
        Self::AllowRequester,
        Self::RpcQuery,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

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
}

impl RuntimeOperationOutcome {
    pub const ALL: [Self; 10] = [
        Self::Succeeded,
        Self::Rejected,
        Self::WriteFailed,
        Self::Timeout,
        Self::Culled,
        Self::PeerRejected,
        Self::Sequencing,
        Self::DependencyFailed,
        Self::Backpressure,
        Self::Untrackable,
    ];

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

impl RuntimeResourceFailure {
    pub const ALL: [Self; 14] = [
        Self::CancelledBySender,
        Self::HashmapBeyondPartCount,
        Self::HashmapSkipsAhead,
        Self::HashmapTooLong,
        Self::HashmapRagged,
        Self::RetriesExhausted,
        Self::LinkVanished,
        Self::TransferUnopenable,
        Self::TransferCorrupt,
        Self::ProofUnsendable,
        Self::DecompressionFailed,
        Self::DecompressionTimedOut,
        Self::OpenTimedOut,
        Self::MetadataOverrun,
    ];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RuntimeLinkClosure {
    Timeout,
    PeerClosed,
    MalformedRtt,
}

impl RuntimeLinkClosure {
    pub const ALL: [Self; 3] = [Self::Timeout, Self::PeerClosed, Self::MalformedRtt];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RuntimeRouteRemoval {
    Expired,
    Evicted,
    InterfaceGone,
}

impl RuntimeRouteRemoval {
    pub const ALL: [Self; 3] = [Self::Expired, Self::Evicted, Self::InterfaceGone];

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
    pub(crate) fn record_journaled(&mut self, journaled: &Journaled<'_>) {
        match journaled {
            Journaled::CommandSettled { settlement, .. } => {
                let (operation, outcome) = operation_outcome(settlement);
                self.operations.record(operation, outcome);
            }
            Journaled::LinkClosed { reason, .. } => {
                self.link_closures.record(link_closure(*reason));
            }
            Journaled::LinkInterfaceMismatch { .. } => {
                self.link_interface_mismatches = self.link_interface_mismatches.saturating_add(1);
            }
            Journaled::ResourceFailed { cause, .. } => {
                self.resource_failures.record(resource_failure(*cause));
            }
            Journaled::RouteRemoved { cause, .. } => {
                self.route_removals.record(route_removal(*cause));
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

fn operation_outcome(settlement: &Settlement) -> (RuntimeOperation, RuntimeOperationOutcome) {
    use RuntimeOperation as Operation;
    use RuntimeOperationOutcome as Outcome;

    match settlement {
        Settlement::AnnounceNow(result) => (
            Operation::AnnounceNow,
            result
                .as_ref()
                .map_or_else(announce_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendSinglePacket(result) => (
            Operation::SendSinglePacket,
            result
                .as_ref()
                .map_or_else(send_single_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendGroup(result) => (
            Operation::SendGroup,
            result
                .as_ref()
                .map_or_else(send_group_failure, |_| Outcome::Succeeded),
        ),
        Settlement::RequestPath(result) => (
            Operation::RequestPath,
            result
                .as_ref()
                .map_or_else(request_path_failure, |_| Outcome::Succeeded),
        ),
        Settlement::EstablishLink(result) => (
            Operation::EstablishLink,
            result
                .as_ref()
                .map_or_else(establish_link_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendToLink(result) => (
            Operation::SendToLink,
            result
                .as_ref()
                .map_or_else(send_to_link_failure, |_| Outcome::Succeeded),
        ),
        Settlement::Identify(result) => (
            Operation::Identify,
            result
                .as_ref()
                .map_or_else(identify_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendRequest(result) => (
            Operation::SendRequest,
            result
                .as_ref()
                .map_or_else(send_request_failure, |_| Outcome::Succeeded),
        ),
        Settlement::Respond(result) => (
            Operation::Respond,
            result
                .as_ref()
                .map_or_else(respond_failure, |_| Outcome::Succeeded),
        ),
        Settlement::CloseLink(result) => (
            Operation::CloseLink,
            result
                .as_ref()
                .map_or_else(close_link_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendResource(result) => (
            Operation::SendResource,
            result
                .as_ref()
                .map_or_else(send_resource_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SetResourceStrategy(result) => (
            Operation::SetResourceStrategy,
            result
                .as_ref()
                .map_or_else(set_resource_strategy_failure, |_| Outcome::Succeeded),
        ),
        Settlement::SendToChannel(result) => (
            Operation::SendToChannel,
            result
                .as_ref()
                .map_or_else(send_to_channel_failure, |_| Outcome::Succeeded),
        ),
        Settlement::AllowRequester(result) => (
            Operation::AllowRequester,
            result
                .as_ref()
                .map_or_else(allow_requester_failure, |_| Outcome::Succeeded),
        ),
        Settlement::RpcQuery(_) => (Operation::RpcQuery, Outcome::Succeeded),
    }
}

fn announce_failure(failure: &AnnounceNowFailure) -> RuntimeOperationOutcome {
    match failure {
        AnnounceNowFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        AnnounceNowFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
    }
}

fn send_single_failure(failure: &SendSinglePacketFailure) -> RuntimeOperationOutcome {
    match failure {
        SendSinglePacketFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendSinglePacketFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
        SendSinglePacketFailure::Culled => RuntimeOperationOutcome::Culled,
        SendSinglePacketFailure::Timeout => RuntimeOperationOutcome::Timeout,
    }
}

fn send_group_failure(failure: &SendGroupFailure) -> RuntimeOperationOutcome {
    match failure {
        SendGroupFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendGroupFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
    }
}

fn request_path_failure(failure: &RequestPathFailure) -> RuntimeOperationOutcome {
    match failure {
        RequestPathFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
        RequestPathFailure::Timeout => RuntimeOperationOutcome::Timeout,
        RequestPathFailure::Culled => RuntimeOperationOutcome::Culled,
    }
}

fn establish_link_failure(failure: &EstablishLinkFailure) -> RuntimeOperationOutcome {
    match failure {
        EstablishLinkFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        EstablishLinkFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
        EstablishLinkFailure::Timeout => RuntimeOperationOutcome::Timeout,
    }
}

fn send_to_link_failure(failure: &SendToLinkFailure) -> RuntimeOperationOutcome {
    match failure {
        SendToLinkFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendToLinkFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
        SendToLinkFailure::Culled => RuntimeOperationOutcome::Culled,
        SendToLinkFailure::Timeout => RuntimeOperationOutcome::Timeout,
    }
}

fn identify_failure(failure: &IdentifyFailure) -> RuntimeOperationOutcome {
    match failure {
        IdentifyFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        IdentifyFailure::WriteFailed => RuntimeOperationOutcome::WriteFailed,
    }
}

fn send_request_failure(failure: &SendRequestFailure) -> RuntimeOperationOutcome {
    match failure {
        SendRequestFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendRequestFailure::WriteFailed => RuntimeOperationOutcome::WriteFailed,
        SendRequestFailure::Culled => RuntimeOperationOutcome::Culled,
        SendRequestFailure::Timeout => RuntimeOperationOutcome::Timeout,
    }
}

fn respond_failure(failure: &RespondFailure) -> RuntimeOperationOutcome {
    match failure {
        RespondFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        RespondFailure::WriteFailed => RuntimeOperationOutcome::WriteFailed,
    }
}

fn close_link_failure(failure: &CloseLinkFailure) -> RuntimeOperationOutcome {
    match failure {
        CloseLinkFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        CloseLinkFailure::WriteFailed => RuntimeOperationOutcome::WriteFailed,
    }
}

fn send_resource_failure(failure: &SendResourceFailure) -> RuntimeOperationOutcome {
    match failure {
        SendResourceFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendResourceFailure::WriteFailed => RuntimeOperationOutcome::WriteFailed,
        SendResourceFailure::RejectedByPeer => RuntimeOperationOutcome::PeerRejected,
        SendResourceFailure::Sequencing => RuntimeOperationOutcome::Sequencing,
        SendResourceFailure::Timeout => RuntimeOperationOutcome::Timeout,
        SendResourceFailure::PredecessorFailed => RuntimeOperationOutcome::DependencyFailed,
    }
}

fn set_resource_strategy_failure(failure: &SetResourceStrategyFailure) -> RuntimeOperationOutcome {
    match failure {
        SetResourceStrategyFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
    }
}

fn send_to_channel_failure(failure: &SendToChannelFailure) -> RuntimeOperationOutcome {
    match failure {
        SendToChannelFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
        SendToChannelFailure::WriteFailed(_) => RuntimeOperationOutcome::WriteFailed,
        SendToChannelFailure::WindowFull => RuntimeOperationOutcome::Backpressure,
        SendToChannelFailure::Untrackable => RuntimeOperationOutcome::Untrackable,
        SendToChannelFailure::Timeout => RuntimeOperationOutcome::Timeout,
    }
}

fn allow_requester_failure(failure: &AllowRequesterFailure) -> RuntimeOperationOutcome {
    match failure {
        AllowRequesterFailure::Rejected(_) => RuntimeOperationOutcome::Rejected,
    }
}

fn resource_failure(cause: ResourceFailureCause) -> RuntimeResourceFailure {
    match cause {
        ResourceFailureCause::CancelledBySender => RuntimeResourceFailure::CancelledBySender,
        ResourceFailureCause::RefusedHashmapUpdate(refusal) => match refusal {
            ApplyHashmapUpdateError::BeyondPartCount => {
                RuntimeResourceFailure::HashmapBeyondPartCount
            }
            ApplyHashmapUpdateError::SkipsAhead => RuntimeResourceFailure::HashmapSkipsAhead,
            ApplyHashmapUpdateError::HashmapTooLong => RuntimeResourceFailure::HashmapTooLong,
            ApplyHashmapUpdateError::HashmapRagged => RuntimeResourceFailure::HashmapRagged,
        },
        ResourceFailureCause::RetriesExhausted => RuntimeResourceFailure::RetriesExhausted,
        ResourceFailureCause::LinkVanished => RuntimeResourceFailure::LinkVanished,
        ResourceFailureCause::TransferUnopenable => RuntimeResourceFailure::TransferUnopenable,
        ResourceFailureCause::TransferCorrupt => RuntimeResourceFailure::TransferCorrupt,
        ResourceFailureCause::ProofUnsendable => RuntimeResourceFailure::ProofUnsendable,
        ResourceFailureCause::DecompressionFailed => RuntimeResourceFailure::DecompressionFailed,
        ResourceFailureCause::DecompressionTimedOut => {
            RuntimeResourceFailure::DecompressionTimedOut
        }
        ResourceFailureCause::OpenTimedOut => RuntimeResourceFailure::OpenTimedOut,
        ResourceFailureCause::MetadataOverrun => RuntimeResourceFailure::MetadataOverrun,
    }
}

fn link_closure(reason: LinkClosedReason) -> RuntimeLinkClosure {
    match reason {
        LinkClosedReason::Timeout => RuntimeLinkClosure::Timeout,
        LinkClosedReason::PeerClosed => RuntimeLinkClosure::PeerClosed,
        LinkClosedReason::MalformedRtt => RuntimeLinkClosure::MalformedRtt,
    }
}

fn route_removal(cause: RouteRemovalCause) -> RuntimeRouteRemoval {
    match cause {
        RouteRemovalCause::Expired => RuntimeRouteRemoval::Expired,
        RouteRemovalCause::Evicted => RuntimeRouteRemoval::Evicted,
        RouteRemovalCause::InterfaceGone => RuntimeRouteRemoval::InterfaceGone,
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
            resource_failure(ResourceFailureCause::RefusedHashmapUpdate(
                ApplyHashmapUpdateError::SkipsAhead
            )),
            RuntimeResourceFailure::HashmapSkipsAhead
        );
        assert_eq!(
            link_closure(LinkClosedReason::MalformedRtt),
            RuntimeLinkClosure::MalformedRtt
        );
        assert_eq!(
            route_removal(RouteRemovalCause::InterfaceGone),
            RuntimeRouteRemoval::InterfaceGone
        );
    }
}
