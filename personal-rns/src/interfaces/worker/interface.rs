//! The interface contract: how an [`Interface`] starts, and what the runtime
//! holds to drive it afterward.
//!
//! Starting an interface produces two runtime-facing pieces:
//!
//! - **Who drives the loop?** — [`DriverMode`]. Either the interface launched its
//!   own loop ([`SelfDriven`](DriverMode::SelfDriven)) or it cannot run itself and
//!   the runtime ticks its `poll` ([`RuntimeDriven`](DriverMode::RuntimeDriven)).
//!   Purely a scheduling decision.
//! - **How does the runtime reach it?** — [`InterfaceHandle`]. Every started
//!   interface, self-driven or not, leaves the runtime a handle to drain its
//!   inbound, queue its outbound, and trade control signals. The runtime's
//!   relationship to a self-driven interface is this handle.
//!
//! [`StartedInterface`] bundles both with the descriptor, so a started interface
//! cannot exist without a runtime handle, and a `SelfDriven` value can never mean
//! "the runtime has no way to reach it."

use crate::engine::InstantMillis;
use crate::interfaces::{
    ConnectionState, ControlReport, InboundPacket, InterfaceDescriptor, InterfaceWorkerContext,
    OutboundPacket, SendError, Substrate,
};

#[derive(Debug, Clone, Copy)]
pub enum NextScheduledInterfaceWake {
    Immediate,
    At(InstantMillis),
    Idle,
}

impl NextScheduledInterfaceWake {
    /// The sooner of two interface poll deadlines: `Immediate` wins, `Idle` loses,
    /// two `At`s take the earlier instant.
    pub fn sooner(self, other: Self) -> Self {
        use NextScheduledInterfaceWake::{At, Idle, Immediate};
        match (self, other) {
            (Immediate, _) | (_, Immediate) => Immediate,
            (Idle, other) | (other, Idle) => other,
            (At(x), At(y)) => At(InstantMillis(x.0.min(y.0))),
        }
    }
}

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
        poll: fn(&mut Worker, InstantMillis) -> NextScheduledInterfaceWake,
    },
}

/// The runtime's end of one started interface. All methods are non-blocking; the
/// concrete handle type decides what queues, channels, or hardware back it.
pub trait InterfaceHandle {
    /// Lend each inbound packet the interface has stamped to `f`, in order, tagged
    /// with the interface's id and arrival stamp. Returns how many were drained.
    fn drain_inbound(&mut self, f: impl FnMut(InboundPacket<'_>)) -> usize;
    fn send(&mut self, packet: OutboundPacket<'_>) -> Result<(), SendError>;

    /// Ask the interface to wind down; it reports [`ControlReport::Stopped`] when
    /// done.
    fn request_stop(&mut self);

    /// The next report the interface has sent, if any. Never blocks.
    fn next_report(&mut self) -> Option<ControlReport>;
}

pub struct StartedInterface<H: InterfaceHandle, Worker> {
    pub descriptor: InterfaceDescriptor,
    pub handle: H,
    pub drive: DriverMode<Worker>,
}

pub trait RegisteredInterface {
    fn descriptor(&self) -> &InterfaceDescriptor;

    fn set_connection_state(&mut self, state: ConnectionState);

    fn drain_inbound(&mut self, on_packet: impl FnMut(InboundPacket<'_>)) -> usize;

    fn send(&mut self, packet: OutboundPacket<'_>) -> Result<(), SendError>;

    fn next_report(&mut self) -> Option<ControlReport>;

    /// Poll a runtime-driven interface's worker for its next deadline; a no-op
    /// returning [`Idle`](NextScheduledInterfaceWake::Idle) for a self-driven one.
    fn poll(&mut self, now: InstantMillis) -> NextScheduledInterfaceWake;

    fn drain_control_reports(&mut self) {
        while let Some(report) = self.next_report() {
            match report {
                ControlReport::ConnectionState(state) => self.set_connection_state(state),
                ControlReport::Stopped => self.set_connection_state(ConnectionState::Disconnected),
            }
        }
    }
}

impl<H: InterfaceHandle, Worker> RegisteredInterface for StartedInterface<H, Worker> {
    fn descriptor(&self) -> &InterfaceDescriptor {
        &self.descriptor
    }

    fn set_connection_state(&mut self, state: ConnectionState) {
        self.descriptor.state = state;
    }

    fn drain_inbound(&mut self, on_packet: impl FnMut(InboundPacket<'_>)) -> usize {
        self.handle.drain_inbound(on_packet)
    }

    fn send(&mut self, packet: OutboundPacket<'_>) -> Result<(), SendError> {
        self.handle.send(packet)
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.handle.next_report()
    }

    fn poll(&mut self, now: InstantMillis) -> NextScheduledInterfaceWake {
        match &mut self.drive {
            DriverMode::SelfDriven => NextScheduledInterfaceWake::Idle,
            DriverMode::RuntimeDriven { worker, poll } => poll(worker, now),
        }
    }
}

/// An autonomous interface, parameterized by the [`Substrate`] it runs on so the
/// generic contract never names a platform's concrete lane types. `start`
/// consumes the interface into its running form and returns its [`DriverMode`].
///
/// There is no teardown method. Graceful wind-down uses the control plane: the
/// interface awaits its own cleanup on [`Stop`](super::ControlCommand), then
/// reports [`Stopped`](ControlReport), and synchronous resource release is just
/// the interface's own [`Drop`].
pub trait Interface<S: Substrate>: Sized {
    /// The worker the runtime polls if this interface can't run itself —
    /// [`Infallible`](core::convert::Infallible) for a self-driven one, so its
    /// `RuntimeDriven` branch is unconstructable and `start` hands the host a
    /// `DriverMode<Infallible>` with no coercion.
    type Worker;

    /// The routing facts the engine registers and routes on — read before `start`
    /// consumes the interface.
    fn descriptor(&self) -> InterfaceDescriptor;

    /// Activate: open the device, take the worker side of the seam, decide the
    /// drive mode, return the [`DriverMode`].
    fn start(self, context: InterfaceWorkerContext<S>) -> DriverMode<Self::Worker>;
}

/// Ready-made [`Interface`] for the common case: one that starts its own thread
/// or task and leaves the runtime with only its handle. The launch closure
/// captures any device-specific state, so this wrapper stays platform-neutral.
pub struct SelfDrivenInterface<Launch> {
    descriptor: InterfaceDescriptor,
    launch: Launch,
}

impl<Launch> SelfDrivenInterface<Launch> {
    /// `launch` must start the interface loop and return promptly.
    pub fn new(descriptor: InterfaceDescriptor, launch: Launch) -> Self {
        Self { descriptor, launch }
    }
}

impl<S, Launch> Interface<S> for SelfDrivenInterface<Launch>
where
    S: Substrate,
    Launch: FnOnce(InterfaceWorkerContext<S>),
{
    type Worker = core::convert::Infallible;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn start(self, context: InterfaceWorkerContext<S>) -> DriverMode<Self::Worker> {
        (self.launch)(context);
        DriverMode::SelfDriven
    }
}
