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

pub use impls::FixedRebroadcastQueue;
#[cfg(feature = "alloc")]
pub use impls::HeapRebroadcastQueue;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRebroadcast {
    pub destination: DestinationHash,
    pub due_at: InstantMillis,
    pub source_interface: InterfaceId,
}

pub trait RebroadcastQueue {
    fn pending_count(&self) -> usize;
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
    );
    fn drain_due(&mut self, now: InstantMillis) -> usize;
    fn earliest_due_at(&self) -> Option<InstantMillis>;
}
