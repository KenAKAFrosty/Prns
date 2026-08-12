use alloc::vec::Vec;

use crate::interfaces::InterfaceId;
use crate::interfaces::InterfaceKind;
use crate::routing::announce::held::HeldDropCause;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::ingress::{AnnounceIngest, IgnoreReason};
use crate::routing::links::handshake::LinkRttError;
use crate::storage::StorageLayout;

use super::state::EngineState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceSourceKind {
    Network,
    SharedClient,
}

impl AnnounceSourceKind {
    pub const ALL: [Self; 2] = [Self::Network, Self::SharedClient];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceIngressOutcome {
    Accepted,
    Held,
    Ignored,
    HeldDroppedInterfaceAtCap,
    HeldDroppedPoolFull,
    HeldDroppedArenaFull,
    Blackholed,
    AcceptedScheduleRejectedQueueFull,
}

impl AnnounceIngressOutcome {
    pub const ALL: [Self; 8] = [
        Self::Accepted,
        Self::AcceptedScheduleRejectedQueueFull,
        Self::Held,
        Self::Ignored,
        Self::HeldDroppedInterfaceAtCap,
        Self::HeldDroppedPoolFull,
        Self::HeldDroppedArenaFull,
        Self::Blackholed,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceCommandOutcome {
    Succeeded,
    Rejected,
    WriteFailed,
}

impl AnnounceCommandOutcome {
    pub const ALL: [Self; 3] = [Self::Succeeded, Self::Rejected, Self::WriteFailed];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceOrigin {
    Local,
    SharedClient,
    Relay,
}

impl AnnounceOrigin {
    pub const ALL: [Self; 3] = [Self::Local, Self::SharedClient, Self::Relay];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceIngressCounts {
    counts: [[u64; AnnounceIngressOutcome::ALL.len()]; AnnounceSourceKind::ALL.len()],
}

impl Default for AnnounceIngressCounts {
    fn default() -> Self {
        Self {
            counts: [[0; AnnounceIngressOutcome::ALL.len()]; AnnounceSourceKind::ALL.len()],
        }
    }
}

impl AnnounceIngressCounts {
    pub const fn get(&self, source: AnnounceSourceKind, outcome: AnnounceIngressOutcome) -> u64 {
        self.counts[source.index()][outcome.index()]
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (AnnounceSourceKind, AnnounceIngressOutcome, u64)> + '_ {
        AnnounceSourceKind::ALL.into_iter().flat_map(move |source| {
            AnnounceIngressOutcome::ALL
                .into_iter()
                .map(move |outcome| (source, outcome, self.get(source, outcome)))
        })
    }

    pub(crate) fn record(&mut self, source: AnnounceSourceKind, outcome: AnnounceIngressOutcome) {
        let count = &mut self.counts[source.index()][outcome.index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceCommandCounts {
    counts: [u64; AnnounceCommandOutcome::ALL.len()],
}

impl Default for AnnounceCommandCounts {
    fn default() -> Self {
        Self {
            counts: [0; AnnounceCommandOutcome::ALL.len()],
        }
    }
}

impl AnnounceCommandCounts {
    pub const fn get(&self, outcome: AnnounceCommandOutcome) -> u64 {
        self.counts[outcome.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (AnnounceCommandOutcome, u64)> + '_ {
        AnnounceCommandOutcome::ALL
            .into_iter()
            .map(|outcome| (outcome, self.get(outcome)))
    }

    pub(crate) fn record(&mut self, outcome: AnnounceCommandOutcome) {
        let count = &mut self.counts[outcome.index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceKindCounts {
    counts: [u64; InterfaceKind::ALL.len()],
    unknown: u64,
}

impl Default for InterfaceKindCounts {
    fn default() -> Self {
        Self {
            counts: [0; InterfaceKind::ALL.len()],
            unknown: 0,
        }
    }
}

impl InterfaceKindCounts {
    pub const fn get(&self, kind: InterfaceKind) -> u64 {
        self.counts[kind as usize]
    }

    pub const fn unknown(&self) -> u64 {
        self.unknown
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (InterfaceKind, u64)> + '_ {
        InterfaceKind::ALL
            .into_iter()
            .map(|kind| (kind, self.get(kind)))
    }

    pub(crate) fn record(&mut self, kind: Option<InterfaceKind>) {
        match kind {
            Some(kind) => {
                let count = &mut self.counts[kind as usize];
                *count = count.saturating_add(1);
            }
            None => self.unknown = self.unknown.saturating_add(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAnnounceMetricsSnapshot {
    pub interface: InterfaceId,
    pub ingress: AnnounceIngressCounts,
    pub held_depth: u32,
    pub scheduled_depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InterfaceMetricGroup {
    pub interface: InterfaceId,
    pub logical_interface: InterfaceId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineAnnounceMetricsSnapshot {
    pub ingress: AnnounceIngressCounts,
    pub accepted_by_interface_kind: InterfaceKindCounts,
    pub commands: AnnounceCommandCounts,
    pub held_depth: u32,
    pub scheduled_depth: u32,
    pub interfaces: Vec<InterfaceAnnounceMetricsSnapshot>,
}

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
    RequestTooLarge,
}

impl IgnoreReasonKind {
    pub const ALL: [Self; 27] = [
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
        Self::RequestTooLarge,
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
            IgnoreReason::RequestTooLarge => Self::RequestTooLarge,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineMetricsSnapshot {
    pub ingested_packets: u64,
    pub ingested_commands: u64,
    pub ignored_packets: IgnoreReasonCounts,
    pub announces: EngineAnnounceMetricsSnapshot,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn attach_metrics_interface(
        &mut self,
        interface: InterfaceId,
        logical_interface: InterfaceId,
    ) {
        match self
            .interface_metric_groups
            .binary_search_by_key(&interface, |group| group.interface)
        {
            Ok(position) => {
                self.interface_metric_groups[position].logical_interface = logical_interface;
            }
            Err(position) => self.interface_metric_groups.insert(
                position,
                InterfaceMetricGroup {
                    interface,
                    logical_interface,
                },
            ),
        }
        if self
            .announce_interface_metrics
            .iter()
            .all(|metrics| metrics.interface != logical_interface)
        {
            self.announce_interface_metrics
                .push(InterfaceAnnounceMetricsSnapshot {
                    interface: logical_interface,
                    ingress: AnnounceIngressCounts::default(),
                    held_depth: 0,
                    scheduled_depth: 0,
                });
        }
    }

    fn logical_metrics_interface(&self, interface: InterfaceId) -> InterfaceId {
        self.interface_metric_groups
            .binary_search_by_key(&interface, |group| group.interface)
            .map_or(interface, |position| {
                self.interface_metric_groups[position].logical_interface
            })
    }

    pub(crate) fn detach_metrics_interface_if_idle(&mut self, interface: InterfaceId) {
        let has_held = self.held_announces.len_for(interface) != 0;
        let has_scheduled = self
            .scheduled_announces
            .iter()
            .any(|scheduled| scheduled.source_interface == interface);
        if has_held || has_scheduled {
            return;
        }
        if let Ok(position) = self
            .interface_metric_groups
            .binary_search_by_key(&interface, |group| group.interface)
        {
            self.interface_metric_groups.remove(position);
        }
    }

    fn record_interface_announce_ingress(
        &mut self,
        interface: InterfaceId,
        source: AnnounceSourceKind,
        outcome: AnnounceIngressOutcome,
    ) {
        let logical_interface = self.logical_metrics_interface(interface);
        let position = self
            .announce_interface_metrics
            .iter()
            .position(|metrics| metrics.interface == logical_interface);
        let metrics = match position {
            Some(position) => &mut self.announce_interface_metrics[position],
            None => {
                self.announce_interface_metrics
                    .push(InterfaceAnnounceMetricsSnapshot {
                        interface: logical_interface,
                        ingress: AnnounceIngressCounts::default(),
                        held_depth: 0,
                        scheduled_depth: 0,
                    });
                let Some(metrics) = self.announce_interface_metrics.last_mut() else {
                    return;
                };
                metrics
            }
        };
        metrics.ingress.record(source, outcome);
    }

    pub(crate) fn interface_announce_metrics_snapshot(
        &self,
    ) -> Vec<InterfaceAnnounceMetricsSnapshot> {
        let mut snapshots = self.announce_interface_metrics.clone();
        for snapshot in &mut snapshots {
            snapshot.held_depth = 0;
            snapshot.scheduled_depth = 0;
        }
        for group in &self.interface_metric_groups {
            let held =
                u32::try_from(self.held_announces.len_for(group.interface)).unwrap_or(u32::MAX);
            if let Some(snapshot) = snapshots
                .iter_mut()
                .find(|snapshot| snapshot.interface == group.logical_interface)
            {
                snapshot.held_depth = snapshot.held_depth.saturating_add(held);
            }
        }
        for scheduled in self.scheduled_announces.iter() {
            let logical_interface = self.logical_metrics_interface(scheduled.source_interface);
            if let Some(snapshot) = snapshots
                .iter_mut()
                .find(|snapshot| snapshot.interface == logical_interface)
            {
                snapshot.scheduled_depth = snapshot.scheduled_depth.saturating_add(1);
            }
        }
        snapshots
    }

    pub(crate) fn record_announce_ingress(&mut self, source: InterfaceId, ingest: AnnounceIngest) {
        let source_kind = if source.kind() == Some(InterfaceKind::LocalClient) {
            AnnounceSourceKind::SharedClient
        } else {
            AnnounceSourceKind::Network
        };
        let outcome = match ingest {
            AnnounceIngest::Accepted(accepted)
                if matches!(
                    accepted.rebroadcast,
                    crate::routing::ingress::RebroadcastDecision::ScheduleRejected(
                        crate::routing::announce::schedule::ScheduleRejection::QueueFull
                    )
                ) =>
            {
                AnnounceIngressOutcome::AcceptedScheduleRejectedQueueFull
            }
            AnnounceIngest::Accepted(_) => AnnounceIngressOutcome::Accepted,
            AnnounceIngest::Held => AnnounceIngressOutcome::Held,
            AnnounceIngest::Ignored => AnnounceIngressOutcome::Ignored,
            AnnounceIngest::Blackholed => AnnounceIngressOutcome::Blackholed,
            AnnounceIngest::HeldDropped {
                cause: HeldDropCause::InterfaceAtCap,
                ..
            } => AnnounceIngressOutcome::HeldDroppedInterfaceAtCap,
            AnnounceIngest::HeldDropped {
                cause: HeldDropCause::PoolFull,
                ..
            } => AnnounceIngressOutcome::HeldDroppedPoolFull,
            AnnounceIngest::HeldDropped {
                cause: HeldDropCause::ArenaFull,
                ..
            } => AnnounceIngressOutcome::HeldDroppedArenaFull,
        };
        self.announce_ingress_counts.record(source_kind, outcome);
        self.record_interface_announce_ingress(source, source_kind, outcome);
        if matches!(ingest, AnnounceIngest::Accepted(_)) {
            let interface_kind = self.logical_metrics_interface(source).kind();
            self.announce_accepted_interface_counts
                .record(interface_kind);
        }
    }

    pub(crate) fn record_announce_command(&mut self, outcome: AnnounceCommandOutcome) {
        self.announce_command_counts.record(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_schedule_capacity_rejections_have_their_own_metric_outcome() {
        let mut state = EngineState::<crate::engine::test_support::TestStorageLayout>::default();
        let source = InterfaceId::new([0xA1; 8]);
        state.record_announce_ingress(
            source,
            AnnounceIngest::Accepted(crate::routing::ingress::AcceptedAnnounce {
                destination: crate::wire::DestinationHash::new([0x44; 16]),
                hops: 3,
                rebroadcast: crate::routing::ingress::RebroadcastDecision::ScheduleRejected(
                    crate::routing::announce::schedule::ScheduleRejection::QueueFull,
                ),
            }),
        );

        assert_eq!(
            state.announce_ingress_counts.get(
                AnnounceSourceKind::Network,
                AnnounceIngressOutcome::AcceptedScheduleRejectedQueueFull,
            ),
            1,
        );
        assert_eq!(
            state.announce_ingress_counts.get(
                AnnounceSourceKind::Network,
                AnnounceIngressOutcome::Accepted
            ),
            0,
        );
    }

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
            IgnoreReason::RequestTooLarge,
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

    #[test]
    fn announce_counters_cover_every_bounded_dimension() {
        let mut ingress = AnnounceIngressCounts::default();
        for source in AnnounceSourceKind::ALL {
            for outcome in AnnounceIngressOutcome::ALL {
                ingress.record(source, outcome);
            }
        }
        assert!(ingress.iter().all(|(_, _, count)| count == 1));

        let mut commands = AnnounceCommandCounts::default();
        for outcome in AnnounceCommandOutcome::ALL {
            commands.record(outcome);
        }
        assert!(commands.iter().all(|(_, count)| count == 1));

        let mut interfaces = InterfaceKindCounts::default();
        for kind in InterfaceKind::ALL {
            interfaces.record(Some(kind));
        }
        interfaces.record(None);
        assert!(interfaces.iter().all(|(_, count)| count == 1));
        assert_eq!(interfaces.unknown(), 1);
    }

    #[test]
    fn interface_announce_metrics_roll_members_into_their_logical_interface() {
        use crate::engine::test_support::TestStorageLayout;

        let logical = InterfaceId::new([0x10; 8]);
        let first = InterfaceId::new([0x11; 8]);
        let second = InterfaceId::new([0x12; 8]);
        let mut state = EngineState::<TestStorageLayout>::default();
        state.attach_metrics_interface(first, logical);
        state.attach_metrics_interface(second, logical);

        state.record_announce_ingress(first, AnnounceIngest::Ignored);
        state.record_announce_ingress(second, AnnounceIngest::Ignored);
        state.record_announce_ingress(first, AnnounceIngest::Blackholed);
        let _ = state.scheduled_announces.schedule(
            crate::wire::DestinationHash::new([0x21; 16]),
            crate::units::InstantMillis(100),
            first,
            1,
        );

        let snapshot = state.metrics_snapshot();
        assert_eq!(snapshot.announces.interfaces.len(), 1);
        assert_eq!(
            snapshot.announces.interfaces[0]
                .ingress
                .get(AnnounceSourceKind::Network, AnnounceIngressOutcome::Ignored),
            2
        );
        assert_eq!(
            snapshot.announces.interfaces[0].ingress.get(
                AnnounceSourceKind::Network,
                AnnounceIngressOutcome::Blackholed,
            ),
            1,
        );
        assert_eq!(snapshot.announces.interfaces[0].scheduled_depth, 1);

        state.interface_departed(
            second,
            crate::routing::warmth::Departure::Forgotten,
            crate::units::InstantMillis(200),
        );
        assert_eq!(state.logical_metrics_interface(second), second);
        assert_eq!(state.logical_metrics_interface(first), logical);
    }
}
