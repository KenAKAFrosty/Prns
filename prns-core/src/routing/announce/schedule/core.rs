use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledAnnounce {
    pub destination: DestinationHash,
    pub due_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub hops: u8,
    pub our_emission_count: u8,
    pub peer_emission_count: u8,
    pub directed_to: Option<InterfaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleRejection {
    /// A bounded queue is full. Existing entries are left unchanged.
    QueueFull,
}

/// The result of scheduling one destination without changing queue admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ScheduleOutcome {
    Inserted,
    Updated,
    Rejected(ScheduleRejection),
}

/// Work removed for one destination from the active queue and its parked flood store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct ScheduleCancellation {
    pub active_removed: bool,
    pub parked_removed: bool,
}

impl ScheduleCancellation {
    pub const NOT_FOUND: Self = Self {
        active_removed: false,
        parked_removed: false,
    };

    pub const fn removed_any(self) -> bool {
        self.active_removed || self.parked_removed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoOutcome {
    NoPendingEntry,
    PeerEmissionCounted,
    RetransmitCancelled,
    HopsUnrelated,
}

pub trait ScheduledAnnounceQueue {
    fn scheduled_count(&self) -> usize;

    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome;

    fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome;

    fn schedule_shared_client(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome;

    fn schedule_directed_shared_client(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome;

    fn cancel(&mut self, destination: &DestinationHash) -> ScheduleCancellation;

    fn drain_due(&mut self, now: InstantMillis) -> usize;

    fn advance_due_retransmits(
        &mut self,
        now: InstantMillis,
        interval_ms: u64,
        max_our_emission_count: u8,
    ) -> usize;

    fn absorb_echo(
        &mut self,
        destination: &DestinationHash,
        received_hops: u8,
        now: InstantMillis,
        max_peer_emission_count: u8,
    ) -> EchoOutcome;

    fn earliest_due_at(&self) -> Option<InstantMillis>;

    fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_;
}
