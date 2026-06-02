//! The interface contract: how an [`Interface`] starts, and what the runtime
//! holds to drive it afterward.
//!
//! Starting an interface answers two separate questions, kept in two separate
//! shapes so neither bleeds into the other:
//!
//! - **Who drives the loop?** — [`DriverMode`]. Either the interface launched its
//!   own loop ([`SelfDriven`](DriverMode::SelfDriven)) or it cannot run itself and
//!   the runtime ticks its `poll` ([`RuntimeDriven`](DriverMode::RuntimeDriven)).
//!   Purely a scheduling question.
//! - **How does the runtime reach it?** — [`InterfaceHandle`]. Every started
//!   interface, self-driven or not, leaves the runtime a handle to drain its
//!   inbound, queue its outbound, and trade control signals. The runtime's
//!   relationship to a self-driven interface is not "nothing" — it is this handle.
//!
//! [`StartedInterface`] bundles both with the descriptor, so a started interface
//! cannot exist without a runtime handle, and a `SelfDriven` value can never mean
//! "the runtime has no way to reach it."

use crate::engine::{InboundPacket, InstantMillis, NextScheduledEngineWork, OutboundPacket};
use crate::interfaces::{ControlReport, InterfaceDescriptor, InterfaceWorkerContext, Substrate};

/// Which side runs an interface's loop — the runtime's *scheduling* decision,
/// nothing more. The runtime-side data lanes and control plane live in the
/// interface's [`InterfaceHandle`], not here, so this enum stays narrowly about
/// "do I poll this worker or not."
pub enum DriverMode<Worker> {
    /// The interface launched its own loop (a thread or an executor task), so the
    /// runtime never polls it. The runtime still reaches it through its
    /// [`InterfaceHandle`]; to wind it down it sends
    /// [`ControlCommand::Stop`](super::ControlCommand) over that handle's control
    /// lane and watches for [`ControlReport::Stopped`].
    SelfDriven,

    /// The interface cannot run itself (no thread, no executor). The runtime owns
    /// `worker` and calls `poll` each cycle — a bare fn-pointer, so the poll-state
    /// travels with the data it acts on and needs no trait of its own.
    RuntimeDriven {
        worker: Worker,
        poll: fn(&mut Worker, InstantMillis) -> NextScheduledEngineWork,
    },
}

/// The runtime's end of one started interface's seam — the mirror image of the
/// [`InterfaceWorkerContext`] the interface's loop holds. The runtime drains the
/// inbound the interface stamped, queues outbound for it to transmit, and trades
/// lifecycle signals, all non-blocking. A platform supplies the concrete type (its
/// lanes' runtime ends); the runtime stores it and never learns what backs it.
pub trait InterfaceHandle {
    /// Lend each inbound packet the interface has stamped to `f`, in order, tagged
    /// with the interface's id and arrival stamp. Returns how many were drained.
    fn drain_inbound(&mut self, f: impl FnMut(InboundPacket<'_>)) -> usize;

    /// Queue a packet for the interface to transmit. `false` if it does not fit or
    /// the interface's queue is full — a drop self-heals, since the engine re-emits
    /// announces on its own cadence.
    fn send(&mut self, packet: OutboundPacket<'_>) -> bool;

    /// Ask the interface to wind down; it reports [`ControlReport::Stopped`] when
    /// done.
    fn request_stop(&mut self);

    /// The next report the interface has sent, if any. Never blocks.
    fn next_report(&mut self) -> Option<ControlReport>;
}

/// A started interface, as the runtime holds it: the routing facts it registered,
/// the [`InterfaceHandle`] it reaches the running interface through, and the
/// [`DriverMode`] saying whether it must also be polled. Coupling the three is
/// what makes "started but unreachable" unrepresentable.
pub struct StartedInterface<H: InterfaceHandle, Worker> {
    pub descriptor: InterfaceDescriptor,
    pub handle: H,
    pub drive: DriverMode<Worker>,
}

/// An autonomous interface, parameterized by the [`Substrate`] it runs on so this
/// generic contract never names a platform's concrete lane types. `start` consumes
/// the interface into its running form: it receives the worker side of the seam
/// (the lanes the platform already built) and returns its [`DriverMode`] — a
/// self-driven interface spawns its loop and returns
/// [`SelfDriven`](DriverMode::SelfDriven); a runtime-driven one does its
/// synchronous setup and hands itself back to be polled. The runtime side of the
/// seam (the [`InterfaceHandle`]) is the platform's other half, kept by the
/// runtime — it does not flow back through `start`.
///
/// There is no teardown method. Graceful wind-down rides the control plane — the
/// interface awaits its own cleanup on [`Stop`](super::ControlCommand), then
/// reports [`Stopped`](ControlReport) — and synchronous resource release is just
/// the interface's own [`Drop`].
pub trait Interface<S: Substrate>: Sized {
    /// The routing facts the engine registers and routes on — read before `start`
    /// consumes the interface.
    fn descriptor(&self) -> InterfaceDescriptor;

    /// Activate: open the device, take the worker side of the seam, decide the
    /// drive mode, return the [`DriverMode`].
    fn start(self, context: InterfaceWorkerContext<S>) -> DriverMode<Self>;
}

/// The ready-made [`Interface`] for the common case: one that drives its own loop.
/// It carries its [`InterfaceDescriptor`] and a `launch` closure that, handed the
/// worker side of the seam, starts that loop on whatever the platform runs
/// concurrent work with — a std thread, an embassy task — and returns. Every
/// self-driven interface, on every platform, has exactly this shape, so the launch
/// mechanism is the *only* thing that varies, and it lives where it must: with the
/// platform that owns concurrency. [`start`](Interface::start) just fires the
/// launcher and reports [`SelfDriven`](DriverMode::SelfDriven).
///
/// The launcher is `FnOnce(InterfaceWorkerContext<S>)` for *every* interface — the
/// device it transmits over is captured *inside* the closure (built beside the
/// platform's spawn primitive, e.g. at an embassy board next to its `#[task]`), so
/// it never surfaces in this generic signature. That is what lets a single wrapper
/// serve serial, LoRa, ESP-NOW, AutoInterface, … across both std and embassy.
pub struct SelfDrivenInterface<Launch> {
    descriptor: InterfaceDescriptor,
    launch: Launch,
}

impl<Launch> SelfDrivenInterface<Launch> {
    /// `descriptor` is the routing facts the engine registers (read before `start`
    /// consumes the interface); `launch`, handed the worker side of the seam, must
    /// start the interface's loop and return promptly — spawn a thread or task, do
    /// not block.
    pub fn new(descriptor: InterfaceDescriptor, launch: Launch) -> Self {
        Self { descriptor, launch }
    }
}

impl<S, Launch> Interface<S> for SelfDrivenInterface<Launch>
where
    S: Substrate,
    Launch: FnOnce(InterfaceWorkerContext<S>),
{
    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn start(self, context: InterfaceWorkerContext<S>) -> DriverMode<Self> {
        (self.launch)(context);
        DriverMode::SelfDriven
    }
}
