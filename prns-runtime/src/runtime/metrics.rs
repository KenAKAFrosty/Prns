use crate::engine::EngineMetricsSnapshot;
use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EgressMetricsSnapshot {
    pub enqueued_frames: u64,
    pub full_lane_drops: u64,
    pub missing_lane_drops: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMetricsSnapshot {
    pub taken_at: InstantMillis,
    pub engine: EngineMetricsSnapshot,
    pub egress: EgressMetricsSnapshot,
    pub crypto: Option<CryptoMetricsSnapshot>,
}
