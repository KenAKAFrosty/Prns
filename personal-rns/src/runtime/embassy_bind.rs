//! The embassy platform binding — the second concrete [`Bind`], distilled from the hand-roll in
//! the S3 firmware's `engine_task`. Where [`TokioBind`](super::TokioBind) *owns* heap channels and
//! lanes, embassy's are `static` (const-sized) and the reactor takes them by borrow — so this
//! binding *holds the borrow bundle* the firmware sets up (the `Host`, the interface descriptors,
//! the inbound-notify and command `Receiver`s over `static` channels, the inbound grant-lane
//! consumers, and the egress) and hands them to the reactor in [`Bind::drive`].
//!
//! There is no handle to hand back: the command channel is a `static` the app already owns, so the
//! app keeps `COMMANDS.sender()` directly and passes `COMMANDS.receiver()` in here — the same
//! dual-side surface as `TokioBind`, with the channel living in static storage instead of the heap.

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;

use crate::engine::{EngineState, IssuedCommand};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::grant::AnyGrantConsumer;
use crate::reactor::impls::embassy_reactor::{run, EmbassyEgress, EmbassyHost};
use crate::storage::StorageLayout;

use super::{Bind, PrnsEvent};

pub struct EmbassyBind<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize>
where
    S: StorageLayout,
    E: FnMut(&mut [u8]),
    M: RawMutex,
{
    host: EmbassyHost<E>,
    interfaces: &'a [InterfaceConfig],
    ifacs: &'a [InterfaceIfac],
    notify: Receiver<'a, M, InterfaceId, NOTIFY>,
    inbound_lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantConsumer)],
    commands: Receiver<'a, M, IssuedCommand, COMMANDS>,
    egress: EmbassyEgress<'a>,
    _storage: core::marker::PhantomData<S>,
}

impl<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize>
    EmbassyBind<'a, S, E, M, NOTIFY, COMMANDS>
where
    S: StorageLayout,
    E: FnMut(&mut [u8]),
    M: RawMutex,
{
    /// Bundle the firmware's already-wired reactor inputs into a binding. The app owns the `static`
    /// notify/command channels and the lane buffers; it keeps the command channel's `Sender` and
    /// passes the matching `Receiver`s here.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: EmbassyHost<E>,
        interfaces: &'a [InterfaceConfig],
        ifacs: &'a [InterfaceIfac],
        notify: Receiver<'a, M, InterfaceId, NOTIFY>,
        inbound_lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantConsumer)],
        commands: Receiver<'a, M, IssuedCommand, COMMANDS>,
        egress: EmbassyEgress<'a>,
    ) -> Self {
        Self {
            host,
            interfaces,
            ifacs,
            notify,
            inbound_lanes,
            commands,
            egress,
            _storage: core::marker::PhantomData,
        }
    }
}

impl<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize> Bind
    for EmbassyBind<'a, S, E, M, NOTIFY, COMMANDS>
where
    S: StorageLayout,
    E: FnMut(&mut [u8]),
    M: RawMutex,
{
    type Storage = S;

    async fn drive(self, engine: EngineState<S>, mut on_event: impl FnMut(PrnsEvent<'_>)) {
        run(
            engine,
            self.interfaces,
            self.ifacs,
            self.host,
            self.notify,
            self.inbound_lanes,
            self.commands,
            self.egress,
            |journaled| on_event(PrnsEvent::from(journaled)),
        )
        .await
    }
}
