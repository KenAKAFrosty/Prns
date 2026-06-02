//! The interface drive contract: an [`Interface`] is started once and resolves
//! into exactly one [`DriverMode`] — self-driven (owns its own loop) or
//! runtime-driven (the loop ticks its `poll`). The enum makes the choice
//! mutually exclusive by construction: a value is one variant, never both or
//! neither, so no two-trait juggling can express an impossible interface.

use crate::engine::{InstantMillis, NextScheduledEngineWork};
use crate::interfaces::InterfaceDescriptor;

/// What an interface becomes the moment it starts: the runtime matches this
/// once, at bring-up, then drives it accordingly — no per-cycle dispatch, no
/// `dyn`, no allocator. The worker struct is the state; the variant only decides
/// *where* it lives.
pub enum DriverMode<Worker> {
    /// The interface launched its own loop (a thread or an executor task). The
    /// runtime never holds it again — they meet only through the seam built at
    /// construction (the byte lanes plus the control plane), so this variant
    /// carries nothing. To wind it down the runtime sends
    /// [`ControlCommand::Stop`](super::ControlCommand) over the control lane and
    /// watches for [`ControlReport::Stopped`](super::ControlReport).
    SelfDriven,

    /// The interface cannot run itself (no thread, no executor). The runtime owns
    /// `worker` and calls `poll` each cycle — a bare fn-pointer, so the poll-state
    /// travels with the data it acts on and needs no trait of its own.
    RuntimeDriven {
        worker: Worker,
        poll: fn(&mut Worker, InstantMillis) -> NextScheduledEngineWork,
    },
}

/// An autonomous interface. `start` consumes it into its running form: a
/// self-driven interface spawns its loop and returns [`DriverMode::SelfDriven`]; a
/// runtime-driven one does its synchronous setup and hands itself back to be
/// polled. The seam (inbound/outbound byte lanes plus the control plane) is
/// wired in at construction, so by `start` the interface already holds it; see
/// [`InterfaceWorkerContext`](super::InterfaceWorkerContext).
///
/// There is no teardown method. Graceful wind-down rides the control plane — the
/// worker awaits its own cleanup on [`Stop`](super::ControlCommand), then reports
/// [`Stopped`](super::ControlReport) — and synchronous resource release is just
/// the worker's own [`Drop`], the same as any owned handle.
pub trait Interface: Sized {
    /// The routing facts the engine registers and routes on — read before
    /// `start` consumes the interface.
    fn descriptor(&self) -> InterfaceDescriptor;

    /// Activate: open the device, decide the drive mode, return the [`DriverMode`].
    fn start(self) -> DriverMode<Self>;
}
