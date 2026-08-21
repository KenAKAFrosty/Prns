use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use prns_core::capabilities::positioning::gnss::{GnssReceiverCommand, GnssSnapshot};

/// The synchronization seam shared by nRF52840 GNSS board adapters.
///
/// Receiver power sequencing and UART ownership remain board-specific. This type only keeps the
/// application's command and observation contracts consistent across those adapters.
pub(crate) struct GnssShared {
    command: Signal<CriticalSectionRawMutex, GnssReceiverCommand>,
    snapshot: Mutex<CriticalSectionRawMutex, RefCell<GnssSnapshot>>,
}

impl GnssShared {
    pub(crate) const fn new() -> Self {
        Self {
            command: Signal::new(),
            snapshot: Mutex::new(RefCell::new(GnssSnapshot::Disabled)),
        }
    }

    pub(crate) fn control(&self, command: GnssReceiverCommand) {
        self.publish(match command {
            GnssReceiverCommand::Enable => GnssSnapshot::Starting,
            GnssReceiverCommand::Disable => GnssSnapshot::Disabled,
        });
        self.command.signal(command);
    }

    pub(crate) async fn wait(&self) -> GnssReceiverCommand {
        self.command.wait().await
    }

    pub(crate) fn snapshot(&self) -> GnssSnapshot {
        self.snapshot.lock(|snapshot| *snapshot.borrow())
    }

    pub(crate) fn publish(&self, snapshot: GnssSnapshot) {
        self.snapshot
            .lock(|current| *current.borrow_mut() = snapshot);
    }
}
