//! The embassy platform binding — the second concrete [`Bind`], distilled from the hand-roll in
//! the S3 firmware's `engine_task`. Where [`TokioBind`](super::TokioBind) *owns* heap channels and
//! lanes, embassy's are `static` (const-sized) and the reactor takes them by borrow — so this
//! binding *holds the borrow bundle* the firmware sets up (the `Host`, the interface descriptors,
//! the inbound-notify and command `Receiver`s over `static` channels, the inbound grant-lane
//! consumers, and the egress) and hands them to the reactor in [`Bind::drive`].
//!
//! The handle is [`EmbassyCommands`] — the embedded twin of `TokioCommands`, built over the command
//! channel's `Sender` and a [`CompletionPool`] the app provides as a `static` (the embedded stand-in
//! for tokio's per-command oneshot, since no_std has no ownable completion to ride the command). The
//! app keeps the `Sender` side wrapped in the handle and passes the `Receiver` and the same pool
//! borrow in here — the dual-side surface of `TokioBind`, with the channel and pool living in static
//! storage instead of the heap.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU64, Ordering};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Receiver, Sender};
use embassy_sync::signal::Signal;

use crate::engine::{
    CloseLink, CommandId, Delivered, EngineCommand, EngineState, IssuedCommand, Journaled, Respond,
    RespondData, SendSingle, SendSingleFailure, SendSinglePayload, Settlement,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::reactor::grant::AnyGrantConsumer;
use crate::reactor::impls::embassy_reactor::{run, EmbassyEgress, EmbassyHost};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

use super::{Bind, PrnsEvent, Responder, SendError};

/// The free-slot sentinel — no real [`CommandId`] reaches `u64::MAX` (the handle mints from zero).
const NO_AWAITER: u64 = u64::MAX;

/// A fixed pool of completion slots an embassy app provides as a `static`, alongside its command
/// channel — the embedded twin of tokio's per-command oneshot. An awaited send claims a slot, parks
/// on its [`Signal`], and the binding fires that slot by command id when the engine settles; the
/// send future releases its slot on drop, so a cancelled send can never wake a later claimant. `N`
/// bounds the awaited sends in flight at once. All bookkeeping is serialized under one blocking
/// mutex, so claim, release, and settle never race even across cores — and `settle` signals while
/// holding it, closing the window where a freed slot could be reused mid-fire.
pub struct CompletionPool<M: RawMutex, const N: usize> {
    next_id: AtomicU64,
    awaited: BlockingMutex<M, RefCell<[u64; N]>>,
    slots: [Signal<M, Settlement>; N],
}

impl<M: RawMutex, const N: usize> Default for CompletionPool<M, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const N: usize> CompletionPool<M, N> {
    /// A pool with every slot free — `const`, so it lives in a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            awaited: BlockingMutex::new(RefCell::new([NO_AWAITER; N])),
            slots: [const { Signal::new() }; N],
        }
    }

    fn mint(&self) -> CommandId {
        CommandId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve a free slot for `id`, clearing any stale signal first. `None` when the pool is full —
    /// the caller already has more awaited sends in flight than `N`.
    fn claim(&self, id: CommandId) -> Option<usize> {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            let slot = awaited.iter().position(|&a| a == NO_AWAITER)?;
            self.slots[slot].reset();
            awaited[slot] = id.0;
            Some(slot)
        })
    }

    /// Free `slot` only if it still belongs to `id` — the send future's drop path. After a settle
    /// has cleared the slot (and another send may have claimed it), this is a no-op, so a late drop
    /// can't clobber a newer claimant.
    fn release(&self, slot: usize, id: CommandId) {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            if awaited[slot] == id.0 {
                awaited[slot] = NO_AWAITER;
                self.slots[slot].reset();
            }
        });
    }

    /// Hand `settlement` to the slot awaiting `id`, if any, and report whether it fired — the
    /// binding drops a fired settlement from the event stream. Signals under the lock so a
    /// concurrent release/claim can't slip the slot out from under the wakeup.
    fn settle(&self, id: CommandId, settlement: Settlement) -> bool {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            match awaited.iter().position(|&a| a == id.0) {
                Some(slot) => {
                    awaited[slot] = NO_AWAITER;
                    self.slots[slot].signal(settlement);
                    true
                }
                None => false,
            }
        })
    }

    async fn parked(&self, slot: usize) -> Settlement {
        self.slots[slot].wait().await
    }
}

/// The embassy command handle — the embedded twin of [`TokioCommands`](super::TokioCommands). It
/// holds the command channel's [`Sender`] and a borrow of the app's [`CompletionPool`], and is
/// `Copy`, so any task can drive the node through it. Every [`CommandId`] is minted from the pool's
/// one counter, so the app never picks ids and a fire-and-forget [`issue`](Self::issue) can't
/// collide with an awaited [`send_single`](Self::send_single).
pub struct EmbassyCommands<'a, M: RawMutex, const COMMANDS: usize, const N: usize> {
    commands: Sender<'a, M, IssuedCommand, COMMANDS>,
    pool: &'a CompletionPool<M, N>,
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Clone
    for EmbassyCommands<'_, M, COMMANDS, N>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> Copy
    for EmbassyCommands<'_, M, COMMANDS, N>
{
}

impl<'a, M: RawMutex, const COMMANDS: usize, const N: usize> EmbassyCommands<'a, M, COMMANDS, N> {
    /// Pair the command channel's sender with the completion pool — the app holds both as `static`s
    /// and passes the matching [`CompletionPool`] reference into [`EmbassyBind::new`] too.
    #[must_use]
    pub fn new(
        commands: Sender<'a, M, IssuedCommand, COMMANDS>,
        pool: &'a CompletionPool<M, N>,
    ) -> Self {
        Self { commands, pool }
    }

    /// Queue an engine command and return the [`CommandId`] it was minted under — watch the event
    /// stream for the settlement tagged with it. `None` if the bounded command lane is full. The
    /// fire-and-forget escape hatch; to await the outcome, prefer [`send_single`](Self::send_single).
    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        let id = self.pool.mint();
        self.commands.try_send(IssuedCommand { id, command }).ok()?;
        Some(id)
    }

    /// Send one Single data packet to `destination` and await its delivery proof — the embedded peer
    /// of [`TokioCommands::send_single`](super::TokioCommands::send_single). Claims a pool slot,
    /// parks on it until the engine settles, and frees the slot on every exit, cancellation
    /// included. `Err(SendError::Busy)` when more awaited sends are in flight than the pool's `N`.
    pub async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        let payload =
            SendSinglePayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim(id).ok_or(SendError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: EngineCommand::SendSingle(SendSingle {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendSingle(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    /// Answer a request with `body` as a single RESPONSE packet — the request runner's path. Embedded
    /// responds inline, so a `body` past the link MDU is refused here (returns `false`); the host
    /// auto-upgrades to a resource instead.
    pub fn respond(&self, responder: Responder, body: &[u8]) -> bool {
        match RespondData::from_slice(body) {
            Ok(data) => self
                .issue(EngineCommand::Respond(Respond {
                    link_id: responder.link_id,
                    request_id: responder.request_id,
                    data,
                }))
                .is_some(),
            Err(_) => false,
        }
    }

    /// Sever an active link. Returns `false` if the command lane is full.
    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(EngineCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }
}

/// Frees a claimed completion slot when its awaited send finishes or is cancelled. Release is
/// guarded by the awaited id, so a late drop after the settle already reused the slot is a no-op.
struct SlotGuard<'a, M: RawMutex, const N: usize> {
    pool: &'a CompletionPool<M, N>,
    slot: usize,
    id: CommandId,
}

impl<M: RawMutex, const N: usize> Drop for SlotGuard<'_, M, N> {
    fn drop(&mut self) {
        self.pool.release(self.slot, self.id);
    }
}

impl<M: RawMutex, const COMMANDS: usize, const N: usize> super::Commands
    for EmbassyCommands<'_, M, COMMANDS, N>
{
    fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn send_single(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<Delivered, SendError<SendSingleFailure>> {
        self.send_single(destination, data).await
    }

    fn respond(&self, responder: Responder, body: &[u8]) -> bool {
        self.respond(responder, body)
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

pub struct EmbassyBind<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize, const N: usize>
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
    pool: &'a CompletionPool<M, N>,
    _storage: core::marker::PhantomData<S>,
}

impl<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize, const N: usize>
    EmbassyBind<'a, S, E, M, NOTIFY, COMMANDS, N>
where
    S: StorageLayout,
    E: FnMut(&mut [u8]),
    M: RawMutex,
{
    /// Bundle the firmware's already-wired reactor inputs into a binding. The app owns the `static`
    /// notify/command channels, the lane buffers, and the [`CompletionPool`]; it keeps the command
    /// channel's `Sender` (wrapped in an [`EmbassyCommands`] over the same pool) and passes the
    /// matching `Receiver`s and a `pool` borrow here.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: EmbassyHost<E>,
        interfaces: &'a [InterfaceConfig],
        ifacs: &'a [InterfaceIfac],
        notify: Receiver<'a, M, InterfaceId, NOTIFY>,
        inbound_lanes: &'a mut [(InterfaceId, &'a mut dyn AnyGrantConsumer)],
        commands: Receiver<'a, M, IssuedCommand, COMMANDS>,
        egress: EmbassyEgress<'a>,
        pool: &'a CompletionPool<M, N>,
    ) -> Self {
        Self {
            host,
            interfaces,
            ifacs,
            notify,
            inbound_lanes,
            commands,
            egress,
            pool,
            _storage: core::marker::PhantomData,
        }
    }
}

impl<'a, S, E, M, const NOTIFY: usize, const COMMANDS: usize, const N: usize> Bind
    for EmbassyBind<'a, S, E, M, NOTIFY, COMMANDS, N>
where
    S: StorageLayout,
    E: FnMut(&mut [u8]),
    M: RawMutex,
{
    type Storage = S;

    async fn drive(self, engine: EngineState<S>, mut on_event: impl FnMut(PrnsEvent<'_>)) {
        let pool = self.pool;
        run(
            engine,
            self.interfaces,
            self.ifacs,
            self.host,
            self.notify,
            self.inbound_lanes,
            self.commands,
            self.egress,
            |journaled| {
                if let Journaled::CommandSettled { id, settlement } = &journaled {
                    if pool.settle(*id, *settlement) {
                        return;
                    }
                }
                on_event(PrnsEvent::from(journaled));
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Rtt;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    type Pool<const N: usize> = CompletionPool<CriticalSectionRawMutex, N>;

    fn delivered(ms: u64) -> Settlement {
        Settlement::SendSingle(Ok(Delivered {
            rtt: Rtt::from_millis(ms),
        }))
    }

    #[test]
    fn the_pool_mints_a_distinct_id_each_time() {
        let pool: Pool<2> = CompletionPool::new();
        assert_eq!(pool.mint(), CommandId(0));
        assert_eq!(pool.mint(), CommandId(1));
        assert_eq!(pool.mint(), CommandId(2));
    }

    #[test]
    fn the_pool_bounds_concurrent_awaited_sends() {
        let pool: Pool<2> = CompletionPool::new();
        let first = pool.claim(CommandId(0));
        let second = pool.claim(CommandId(1));
        assert!(first.is_some() && second.is_some());
        assert_ne!(first, second);
        assert_eq!(
            pool.claim(CommandId(2)),
            None,
            "a full pool refuses a claim"
        );
    }

    #[test]
    fn settle_wakes_only_the_slot_awaiting_that_id() {
        let pool: Pool<3> = CompletionPool::new();
        pool.claim(CommandId(10));
        pool.claim(CommandId(11));
        pool.claim(CommandId(12));
        assert!(
            !pool.settle(CommandId(99), delivered(1)),
            "no slot awaits 99"
        );
        assert!(pool.settle(CommandId(11), delivered(1)));
        assert!(pool.settle(CommandId(10), delivered(1)));
        assert!(pool.settle(CommandId(12), delivered(1)));
    }

    #[test]
    fn a_settled_slot_frees_for_reuse() {
        let pool: Pool<1> = CompletionPool::new();
        let id = CommandId(0);
        assert!(pool.claim(id).is_some());
        assert_eq!(pool.claim(CommandId(1)), None, "full while id awaits");
        assert!(pool.settle(id, delivered(1)));
        assert!(
            pool.claim(CommandId(1)).is_some(),
            "the slot frees once settled"
        );
    }

    #[test]
    fn a_cancelled_await_releases_its_slot_and_ignores_a_late_settlement() {
        let pool: Pool<1> = CompletionPool::new();
        let id = CommandId(0);
        let slot = pool.claim(id).expect("a slot");
        pool.release(slot, id);
        assert!(
            !pool.settle(id, delivered(1)),
            "a settlement for a released await fires nothing"
        );
        assert!(
            pool.claim(CommandId(1)).is_some(),
            "the released slot is reusable"
        );
    }

    #[test]
    fn a_late_release_never_clobbers_a_newer_claimant() {
        let pool: Pool<1> = CompletionPool::new();
        let first = CommandId(0);
        let slot = pool.claim(first).expect("a slot");
        assert!(pool.settle(first, delivered(1)));

        let second = CommandId(1);
        assert_eq!(pool.claim(second), Some(slot), "the same slot is reused");
        pool.release(slot, first);
        assert!(
            pool.settle(second, delivered(2)),
            "the stale release left the new claimant intact"
        );
    }
}
