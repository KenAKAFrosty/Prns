use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::{Receiver as WatchReceiver, Sender as WatchSender, Watch};

use super::super::RuntimeSnapshot;

pub const RUNTIME_SNAPSHOT_RECEIVERS: usize = 1;

pub type RuntimeSnapshotWatch =
    Watch<CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
pub type RuntimeSnapshotSender =
    WatchSender<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
pub type RuntimeSnapshotReceiver =
    WatchReceiver<'static, CriticalSectionRawMutex, RuntimeSnapshot, RUNTIME_SNAPSHOT_RECEIVERS>;
