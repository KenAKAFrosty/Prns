//! Announces waiting to be retransmitted: RNS's `announce_table`, keyed by destination so a fresher announce supersedes the one already waiting.
//! Entries are destination + due time only; the announce bytes live in the routing table's app_data arena and are read back at emit time, so the freshest accept is the one re-emission with no second copy.

mod impls;

pub use impls::*;

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
pub enum EchoOutcome {
    NoPendingEntry,
    PeerRebroadcastCounted,
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
    );

    fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    );

    fn drain_due(&mut self, now: InstantMillis) -> usize;

    fn advance_due_retransmits(
        &mut self,
        now: InstantMillis,
        interval_ms: u64,
        max_emission_count: u8,
    ) -> usize;

    fn absorb_echo(
        &mut self,
        destination: &DestinationHash,
        received_hops: u8,
        now: InstantMillis,
        max_peer_rebroadcast_count: u8,
    ) -> EchoOutcome;

    fn earliest_due_at(&self) -> Option<InstantMillis>;

    fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_;
}
