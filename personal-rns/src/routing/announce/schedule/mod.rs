//! Pending rebroadcasts of accepted announces — the analog of RNS's `announce_table`
//! ([Transport.py:113](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L113)):
//! "a table for storing announces currently waiting to be retransmitted."
//!
//! One entry per destination whose announce we accepted and now owe the network a
//! re-emission of, keyed by destination so a fresher announce supersedes the one
//! already waiting. Entries are tiny — destination + due time only; the announce
//! bytes live in the routing table's app_data arena and are read back at emit time,
//! keeping the freshest accept the one rebroadcast with no second copy.

mod impls;

pub use impls::*;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRebroadcast {
    pub destination: DestinationHash,
    pub due_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub hops: u8,
    pub emission_count: u8,
    pub peer_rebroadcast_count: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoOutcome {
    NoPendingEntry,
    PeerRebroadcastCounted,
    RetransmitCancelled,
    HopsUnrelated,
}

pub trait RebroadcastQueue {
    fn pending_count(&self) -> usize;
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
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
    fn iter(&self) -> impl Iterator<Item = ScheduledRebroadcast> + '_;
}
