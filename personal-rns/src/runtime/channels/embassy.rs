//! The embassy snapshot channel: the runtime publishes each cycle's
//! [`RuntimeSnapshot`] on a `Watch` an app subscribes to, such as a display render
//! task. Interfaces meet the runtime through the per-interface seam in
//! [`embassy_seam`](super::embassy_seam).

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::{Receiver as WatchReceiver, Sender as WatchSender, Watch};

use super::super::RuntimeSnapshot;

/// How many subscribers can read the runtime snapshot. One display task today; bump
/// if more consumers (a metrics exporter, a control UI) subscribe.
pub const RUNTIME_SNAPSHOT_RECEIVERS: usize = 1;

/// The latest-wins channel the runtime fires its post-cycle [`RuntimeSnapshot`] out on
/// — `Watch`, not a queue, so a burst of cycles coalesces to the newest value and a
/// subscriber that wakes late never replays stale snapshots. An app awaits
/// [`changed`](WatchReceiver::changed) and so sleeps until engine state actually moves
/// — no polling.
pub type RuntimeSnapshotWatch =
    Watch<CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The publishing end the runtime loop holds.
pub type RuntimeSnapshotSender =
    WatchSender<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
/// The subscribing end an app holds.
pub type RuntimeSnapshotReceiver =
    WatchReceiver<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
