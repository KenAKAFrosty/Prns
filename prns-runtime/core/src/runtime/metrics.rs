use alloc::vec::Vec;

use crate::engine::{AnnounceOrigin, EngineMetricsSnapshot};
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::runtime::ReliabilityMetricsSnapshot;
use crate::units::InstantMillis;

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum AnnounceEgressOutcome {
        Enqueued,
        InterfaceUnavailable,
        LaneFull,
        LaneMissing,
        IfacRejected,
        PacerRejected,
        PacerEvicted,
        PacerExpired,
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum AnnounceBackpressureEvent {
        Deferred,
        Retry,
        Recovered,
    }
}

impl AnnounceEgressOutcome {
    const fn index(self) -> usize {
        self as usize
    }
}

impl AnnounceBackpressureEvent {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceEgressCounts {
    counts: [[u64; AnnounceEgressOutcome::ALL.len()]; AnnounceOrigin::ALL.len()],
}

impl Default for AnnounceEgressCounts {
    fn default() -> Self {
        Self {
            counts: [[0; AnnounceEgressOutcome::ALL.len()]; AnnounceOrigin::ALL.len()],
        }
    }
}

impl AnnounceEgressCounts {
    pub const fn get(&self, origin: AnnounceOrigin, outcome: AnnounceEgressOutcome) -> u64 {
        self.counts[origin.index()][outcome.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = (AnnounceOrigin, AnnounceEgressOutcome, u64)> + '_ {
        AnnounceOrigin::ALL.into_iter().flat_map(move |origin| {
            AnnounceEgressOutcome::ALL
                .into_iter()
                .map(move |outcome| (origin, outcome, self.get(origin, outcome)))
        })
    }

    fn record(&mut self, origin: AnnounceOrigin, outcome: AnnounceEgressOutcome) {
        let count = &mut self.counts[origin.index()][outcome.index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceBackpressureCounts {
    counts: [[u64; AnnounceBackpressureEvent::ALL.len()]; AnnounceOrigin::ALL.len()],
}

impl Default for AnnounceBackpressureCounts {
    fn default() -> Self {
        Self {
            counts: [[0; AnnounceBackpressureEvent::ALL.len()]; AnnounceOrigin::ALL.len()],
        }
    }
}

impl AnnounceBackpressureCounts {
    pub const fn get(&self, origin: AnnounceOrigin, event: AnnounceBackpressureEvent) -> u64 {
        self.counts[origin.index()][event.index()]
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (AnnounceOrigin, AnnounceBackpressureEvent, u64)> + '_ {
        AnnounceOrigin::ALL.into_iter().flat_map(move |origin| {
            AnnounceBackpressureEvent::ALL
                .into_iter()
                .map(move |event| (origin, event, self.get(origin, event)))
        })
    }

    fn record(&mut self, origin: AnnounceOrigin, event: AnnounceBackpressureEvent) {
        let count = &mut self.counts[origin.index()][event.index()];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceOriginCounts {
    counts: [u64; AnnounceOrigin::ALL.len()],
}

impl Default for AnnounceOriginCounts {
    fn default() -> Self {
        Self {
            counts: [0; AnnounceOrigin::ALL.len()],
        }
    }
}

impl AnnounceOriginCounts {
    pub const fn get(&self, origin: AnnounceOrigin) -> u64 {
        self.counts[origin.index()]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (AnnounceOrigin, u64)> + '_ {
        AnnounceOrigin::ALL
            .into_iter()
            .map(|origin| (origin, self.get(origin)))
    }

    fn add(&mut self, origin: AnnounceOrigin, value: u64) {
        let count = &mut self.counts[origin.index()];
        *count = count.saturating_add(value);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressInterfaceKindCounts {
    counts: [u64; InterfaceKind::ALL.len()],
    unknown: u64,
}

impl Default for EgressInterfaceKindCounts {
    fn default() -> Self {
        Self {
            counts: [0; InterfaceKind::ALL.len()],
            unknown: 0,
        }
    }
}

impl EgressInterfaceKindCounts {
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

    fn record(&mut self, kind: Option<InterfaceKind>) {
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
pub struct InterfaceAnnounceEgressMetricsSnapshot {
    pub interface: InterfaceId,
    pub outcomes: AnnounceEgressCounts,
    pub backpressure: AnnounceBackpressureCounts,
    pub enqueued_bytes_by_origin: AnnounceOriginCounts,
    pub pacer_queue_depth: u32,
    pub pacer_deferred_depth: u32,
    pub pacer_oldest_deferred_age_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnounceEgressMetricsSnapshot {
    pub outcomes: AnnounceEgressCounts,
    pub backpressure: AnnounceBackpressureCounts,
    pub enqueued_by_interface_kind: EgressInterfaceKindCounts,
    pub enqueued_bytes_by_origin: AnnounceOriginCounts,
    pub pacer_queue_depth: u32,
    pub pacer_deferred_depth: u32,
    pub pacer_oldest_deferred_age_ms: u64,
    pub interfaces: Vec<InterfaceAnnounceEgressMetricsSnapshot>,
}

impl AnnounceEgressMetricsSnapshot {
    pub fn record(
        &mut self,
        origin: AnnounceOrigin,
        interface: InterfaceId,
        outcome: AnnounceEgressOutcome,
        bytes: usize,
    ) {
        self.outcomes.record(origin, outcome);
        if outcome == AnnounceEgressOutcome::Enqueued {
            self.enqueued_by_interface_kind.record(interface.kind());
            self.enqueued_bytes_by_origin
                .add(origin, u64::try_from(bytes).unwrap_or(u64::MAX));
        }
        let interface_metrics = self.interface_mut(interface);
        interface_metrics.outcomes.record(origin, outcome);
        if outcome == AnnounceEgressOutcome::Enqueued {
            interface_metrics
                .enqueued_bytes_by_origin
                .add(origin, u64::try_from(bytes).unwrap_or(u64::MAX));
        }
    }

    pub fn register_interface(&mut self, interface: InterfaceId) {
        let _ = self.interface_mut(interface);
    }

    pub fn record_backpressure(
        &mut self,
        origin: AnnounceOrigin,
        interface: InterfaceId,
        event: AnnounceBackpressureEvent,
    ) {
        self.backpressure.record(origin, event);
        self.interface_mut(interface)
            .backpressure
            .record(origin, event);
    }

    pub fn reset_pacer_gauges(&mut self) {
        self.pacer_queue_depth = 0;
        self.pacer_deferred_depth = 0;
        self.pacer_oldest_deferred_age_ms = 0;
        for metrics in &mut self.interfaces {
            metrics.pacer_queue_depth = 0;
            metrics.pacer_deferred_depth = 0;
            metrics.pacer_oldest_deferred_age_ms = 0;
        }
    }

    pub fn add_pacer_gauges(
        &mut self,
        interface: InterfaceId,
        depth: usize,
        deferred_depth: usize,
        oldest_deferred_age_ms: u64,
    ) {
        let depth = u32::try_from(depth).unwrap_or(u32::MAX);
        self.pacer_queue_depth = self.pacer_queue_depth.saturating_add(depth);
        let deferred_depth = u32::try_from(deferred_depth).unwrap_or(u32::MAX);
        self.pacer_deferred_depth = self.pacer_deferred_depth.saturating_add(deferred_depth);
        self.pacer_oldest_deferred_age_ms = self
            .pacer_oldest_deferred_age_ms
            .max(oldest_deferred_age_ms);
        let metrics = self.interface_mut(interface);
        metrics.pacer_queue_depth = metrics.pacer_queue_depth.saturating_add(depth);
        metrics.pacer_deferred_depth = metrics.pacer_deferred_depth.saturating_add(deferred_depth);
        metrics.pacer_oldest_deferred_age_ms = metrics
            .pacer_oldest_deferred_age_ms
            .max(oldest_deferred_age_ms);
    }

    fn interface_mut(
        &mut self,
        interface: InterfaceId,
    ) -> &mut InterfaceAnnounceEgressMetricsSnapshot {
        if let Some(position) = self
            .interfaces
            .iter()
            .position(|metrics| metrics.interface == interface)
        {
            return &mut self.interfaces[position];
        }
        self.interfaces
            .push(InterfaceAnnounceEgressMetricsSnapshot {
                interface,
                outcomes: AnnounceEgressCounts::default(),
                backpressure: AnnounceBackpressureCounts::default(),
                enqueued_bytes_by_origin: AnnounceOriginCounts::default(),
                pacer_queue_depth: 0,
                pacer_deferred_depth: 0,
                pacer_oldest_deferred_age_ms: 0,
            });
        let position = self.interfaces.len() - 1;
        &mut self.interfaces[position]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressLaneMetricsSnapshot {
    pub physical_interface: InterfaceId,
    pub logical_interface: InterfaceId,
    pub capacity: u32,
    pub occupancy: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressMetricsSnapshot {
    pub enqueued_frames: u64,
    pub unavailable_frame_skips: u64,
    pub full_lane_drops: u64,
    pub missing_lane_drops: u64,
    pub ifac_rejected_frames: u64,
    pub announces: AnnounceEgressMetricsSnapshot,
    pub lanes: Vec<EgressLaneMetricsSnapshot>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CryptoMetricsSnapshot {
    pub submitted_jobs: u64,
    pub completed_jobs: u64,
    pub queue_depth: u32,
    pub maximum_queue_depth: u32,
    pub backpressure_deferrals: u64,
    pub packet_verdicts_owed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMetricsSnapshot {
    pub taken_at: InstantMillis,
    pub engine: EngineMetricsSnapshot,
    pub egress: EgressMetricsSnapshot,
    pub crypto: Option<CryptoMetricsSnapshot>,
    pub reliability: ReliabilityMetricsSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_backpressure_counters_and_pacer_gauges_preserve_dimensions() {
        let interface = InterfaceId::new([0x31; 8]);
        let mut metrics = AnnounceEgressMetricsSnapshot::default();
        metrics.record_backpressure(
            AnnounceOrigin::Relay,
            interface,
            AnnounceBackpressureEvent::Deferred,
        );
        metrics.record_backpressure(
            AnnounceOrigin::Relay,
            interface,
            AnnounceBackpressureEvent::Retry,
        );
        metrics.add_pacer_gauges(interface, 3, 2, 1_250);

        assert_eq!(
            metrics
                .backpressure
                .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Deferred),
            1
        );
        assert_eq!(metrics.pacer_queue_depth, 3);
        assert_eq!(metrics.pacer_deferred_depth, 2);
        assert_eq!(metrics.pacer_oldest_deferred_age_ms, 1_250);
        assert_eq!(metrics.interfaces.len(), 1);
        assert_eq!(
            metrics.interfaces[0]
                .backpressure
                .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Retry),
            1
        );

        metrics.reset_pacer_gauges();
        assert_eq!(metrics.pacer_queue_depth, 0);
        assert_eq!(metrics.pacer_deferred_depth, 0);
        assert_eq!(metrics.pacer_oldest_deferred_age_ms, 0);
        assert_eq!(
            metrics
                .backpressure
                .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Deferred),
            1,
            "snapshot gauge refreshes must not reset cumulative lifecycle counters"
        );
    }
}
