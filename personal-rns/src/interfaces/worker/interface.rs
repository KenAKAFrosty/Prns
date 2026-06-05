use crate::engine::InstantMillis;
use crate::interfaces::{
    ConnectionState, ControlReport, InboundPacket, InterfaceDescriptor, InterfaceWorkerContext,
    SendError, Substrate,
};

#[derive(Debug, Clone, Copy)]
pub enum NextScheduledInterfaceWake {
    Immediate,
    At(InstantMillis),
    Idle,
}

impl NextScheduledInterfaceWake {
    pub fn sooner(self, other: Self) -> Self {
        use NextScheduledInterfaceWake::{At, Idle, Immediate};
        match (self, other) {
            (Immediate, _) | (_, Immediate) => Immediate,
            (Idle, other) | (other, Idle) => other,
            (At(x), At(y)) => At(InstantMillis(x.0.min(y.0))),
        }
    }
}

pub enum DriverMode<Worker> {
    SelfDriven,

    /// The interface cannot run itself (no thread, no executor). The runtime owns
    /// `worker` and calls `poll` each cycle — a bare fn-pointer, so the poll-state
    /// travels with the data it acts on and needs no trait of its own.
    RuntimeDriven {
        worker: Worker,
        poll: fn(&mut Worker, InstantMillis) -> NextScheduledInterfaceWake,
    },
}

pub trait InterfaceHandle {
    fn drain_inbound(&mut self, f: impl FnMut(InboundPacket<'_>)) -> usize;

    /// The outbound mirror of [`InboundSink::submit`](crate::interfaces::InboundSink::submit):
    /// the handle grants a queue slot first, `fill` writes the packet straight
    /// into it, and the written length is what got queued. `fill` only runs
    /// once a grant is in hand — a full queue costs no serialization.
    fn acquire_send_grant(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<usize, SendError>;

    fn request_stop(&mut self);

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

    fn send_with(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<usize, SendError>;

    fn next_report(&mut self) -> Option<ControlReport>;

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

    fn send_with(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<usize, SendError> {
        self.handle.acquire_send_grant(fill)
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

/// There is no teardown method. Graceful wind-down uses the control plane: the
/// interface awaits its own cleanup on [`Stop`](crate::interfaces::ControlCommand), then
/// reports [`Stopped`](ControlReport), and synchronous resource release is just
/// the interface's own [`Drop`].
pub trait Interface<S: Substrate>: Sized {
    /// The worker the runtime polls if this interface can't run itself —
    /// [`Infallible`](core::convert::Infallible) for a self-driven one, so its
    /// `RuntimeDriven` branch is unconstructable and `start` hands the host a
    /// `DriverMode<Infallible>` with no coercion.
    type Worker;

    fn descriptor(&self) -> InterfaceDescriptor;

    fn start(self, context: InterfaceWorkerContext<S>) -> DriverMode<Self::Worker>;
}

pub struct SelfDrivenInterface<Launch> {
    descriptor: InterfaceDescriptor,
    launch: Launch,
}

impl<Launch> SelfDrivenInterface<Launch> {
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
