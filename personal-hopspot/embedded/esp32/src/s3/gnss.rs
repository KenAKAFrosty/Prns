use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;

use personal_hopspot_core::{GnssAvailability, GnssReceiverCommand, GnssSnapshot};

/// The board-neutral control and observation seam shared by ESP32 GNSS adapters.
///
/// Commands are edge-triggered through [`Signal`], while the latest coherent snapshot remains
/// readable by the display task without coupling it to a particular UART or receiver chipset.
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

    pub(crate) fn snapshot(&self) -> GnssSnapshot {
        self.snapshot.lock(|snapshot| *snapshot.borrow())
    }

    pub(crate) async fn wait_command(&self) -> GnssReceiverCommand {
        self.command.wait().await
    }

    pub(crate) fn publish(&self, snapshot: GnssSnapshot) {
        self.snapshot
            .lock(|current| *current.borrow_mut() = snapshot);
    }
}

/// A board-owned GNSS receiver behind the S3 runtime's common command/snapshot contract.
#[allow(async_fn_in_trait)]
pub(crate) trait GnssProvider {
    const AVAILABILITY: GnssAvailability;

    fn control(command: GnssReceiverCommand);
    fn snapshot() -> Option<GnssSnapshot>;

    async fn drive(self);
}

/// Capability implementation for boards that do not expose a GNSS receiver.
pub(crate) struct NoGnss;

impl GnssProvider for NoGnss {
    const AVAILABILITY: GnssAvailability = GnssAvailability::Unavailable;

    fn control(_command: GnssReceiverCommand) {}

    fn snapshot() -> Option<GnssSnapshot> {
        None
    }

    async fn drive(self) {
        core::future::pending::<()>().await;
    }
}
