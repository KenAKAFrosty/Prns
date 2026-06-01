//! The embassy inbound mailbox + snapshot channel. A worker task stamps
//! [`InboxEntry`]s into the `Channel` the runtime drains, and the runtime fires
//! each cycle's [`RuntimeSnapshot`] out on the `Watch` an app subscribes to.
//!
//! The mailbox lives here, with its draining end — a worker is *handed* an
//! [`InboundSender`] and stamps into it; the host (e.g. `EmbassyHost`) holds the
//! [`InboundReceiver`]. (The outbound queue drains the other way — into the
//! worker — so it lives with the worker shell.)

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::watch::{Receiver as WatchReceiver, Sender as WatchSender, Watch};
use heapless::Vec as HVec;

use super::super::RuntimeSnapshot;
use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

/// How many inbound packets the shared mailbox holds before a worker's stamp is
/// dropped. A drop self-heals — the engine re-emits announces on its own cadence
/// and Reticulum tolerates loss — so a shallow queue is the right trade on a
/// constrained node.
pub const INBOX_DEPTH: usize = 4;

/// One inbound packet a worker has stamped into the shared mailbox: the wire
/// bytes plus the provenance the runtime needs (which interface heard it, and
/// when). An *owned* [`InboundPacket`](crate::engine::InboundPacket) — it must
/// outlive the socket read that produced it to ride into the next cycle's batch.
///
/// `PACKET_BUFFER_SIZE` is the producing worker's
/// [`InterfaceWorker::PACKET_BUFFER_SIZE`](crate::interfaces::InterfaceWorker::PACKET_BUFFER_SIZE);
/// the host sizes the mailbox off that one well-known number so a stamped packet
/// always fits.
pub struct InboxEntry<const PACKET_BUFFER_SIZE: usize> {
    pub arrived_at: InstantMillis,
    pub source: InterfaceId,
    pub bytes: HVec<u8, PACKET_BUFFER_SIZE>,
}

/// The shared inbound mailbox: workers stamp [`InboxEntry`]s in, the runtime
/// drains them each cycle. Sized to the worker's `PACKET_BUFFER_SIZE`.
pub type InboundChannel<const PACKET_BUFFER_SIZE: usize> =
    Channel<CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;
/// The stamping end a worker holds.
pub type InboundSender<const PACKET_BUFFER_SIZE: usize> =
    Sender<'static, CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;
/// The draining end the host holds.
pub type InboundReceiver<const PACKET_BUFFER_SIZE: usize> =
    Receiver<'static, CriticalSectionRawMutex, InboxEntry<PACKET_BUFFER_SIZE>, INBOX_DEPTH>;

/// How many subscribers can read the runtime snapshot. One display task today;
/// bump if more consumers (a metrics exporter, a control UI) subscribe.
pub const RUNTIME_SNAPSHOT_RECEIVERS: usize = 1;

/// The latest-wins channel the runtime fires its post-cycle [`RuntimeSnapshot`]
/// out on — `Watch`, not a queue, so a burst of cycles coalesces to the newest
/// value and a subscriber that wakes late never replays stale snapshots. An app
/// awaits [`changed`](WatchReceiver::changed) and so sleeps until engine state
/// actually moves — no polling.
pub type RuntimeSnapshotWatch =
    Watch<CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The publishing end the runtime loop holds.
pub type RuntimeSnapshotSender =
    WatchSender<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The subscribing end an app holds.
pub type RuntimeSnapshotReceiver =
    WatchReceiver<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
