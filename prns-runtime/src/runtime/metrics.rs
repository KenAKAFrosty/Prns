use crate::engine::{AnnounceOrigin, EngineMetricsSnapshot};
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceEgressOutcome {
    Enqueued,
    InterfaceUnavailable,
    LaneFull,
    LaneMissing,
    IfacRejected,
    PacerRejected,
}

impl AnnounceEgressOutcome {
    pub const ALL: [Self; 6] = [
        Self::Enqueued,
        Self::InterfaceUnavailable,
        Self::LaneFull,
        Self::LaneMissing,
        Self::IfacRejected,
        Self::PacerRejected,
    ];

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
    pub enqueued_bytes_by_origin: AnnounceOriginCounts,
    pub pacer_queue_depth: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnounceEgressMetricsSnapshot {
    pub outcomes: AnnounceEgressCounts,
    pub enqueued_by_interface_kind: EgressInterfaceKindCounts,
    pub enqueued_bytes_by_origin: AnnounceOriginCounts,
    pub pacer_queue_depth: u32,
    pub interfaces: std::vec::Vec<InterfaceAnnounceEgressMetricsSnapshot>,
}

impl AnnounceEgressMetricsSnapshot {
    pub(crate) fn record(
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

    pub(crate) fn register_interface(&mut self, interface: InterfaceId) {
        let _ = self.interface_mut(interface);
    }

    pub(crate) fn reset_pacer_depths(&mut self) {
        self.pacer_queue_depth = 0;
        for metrics in &mut self.interfaces {
            metrics.pacer_queue_depth = 0;
        }
    }

    pub(crate) fn add_pacer_depth(&mut self, interface: InterfaceId, depth: usize) {
        let depth = u32::try_from(depth).unwrap_or(u32::MAX);
        self.pacer_queue_depth = self.pacer_queue_depth.saturating_add(depth);
        let metrics = self.interface_mut(interface);
        metrics.pacer_queue_depth = metrics.pacer_queue_depth.saturating_add(depth);
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
                enqueued_bytes_by_origin: AnnounceOriginCounts::default(),
                pacer_queue_depth: 0,
            });
        let position = self.interfaces.len() - 1;
        &mut self.interfaces[position]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressMetricsSnapshot {
    pub enqueued_frames: u64,
    pub unavailable_frame_skips: u64,
    pub full_lane_drops: u64,
    pub missing_lane_drops: u64,
    pub announces: AnnounceEgressMetricsSnapshot,
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
}
