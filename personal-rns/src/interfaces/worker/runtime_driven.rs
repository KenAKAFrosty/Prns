use crate::engine::{InstantMillis, NextScheduledEngineWork};
use crate::interfaces::InterfaceWorker;

/// OPTIONAL refinement of [`InterfaceWorker`] for workers that cannot run
/// themselves (no OS thread, no async executor) or that want the runtime to
/// own their wake cadence to participate in cooperative only-wake-when-needed power
/// savings. The runtime's loop calls [`poll`](RuntimeDriven::poll); the worker
/// does one non-blocking pass of its own work and reports when it next needs
/// driving. A worker that runs on its own thread or task never implements this
/// (it just needs its channels).
pub trait RuntimeDriven: InterfaceWorker {
    /// Advance one non-blocking step — pump sockets, run discovery, expire
    /// peers, deliver queued fan-out — and return when the worker next needs to
    /// be polled, folded the same way the engine folds its own
    /// [`next_wakeup`](crate::engine::EngineState::next_wakeup): `Immediate` if
    /// there is work due now, `At` for a deadline, `Idle` if nothing is pending.
    fn poll(&mut self, now: InstantMillis) -> NextScheduledEngineWork;
}
