use core::cell::{Cell, RefCell};
use core::future::poll_fn;
use core::task::Poll;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use embassy_sync::waitqueue::MultiWakerRegistration;
use portable_atomic::{AtomicU8, Ordering};
use prns_core::interfaces::bluetooth_auto::Origin;

type Availability<M> = FairSemaphore<M, 1>;

pub struct ConnectionSlotPool<M: RawMutex + 'static, const SLOTS: usize> {
    availability: Availability<M>,
    slots: [ConnectionSlotState<M>; SLOTS],
}

struct ConnectionSlotState<M: RawMutex + 'static> {
    owners: AtomicU8,
    index: AtomicU8,
    availability: BlockingMutex<M, Cell<Option<&'static Availability<M>>>>,
    closed: BlockingMutex<M, RefCell<CloseState>>,
}

struct CloseState {
    requested: bool,
    // The worker, source, sink and retained control can all wait independently.
    waiters: MultiWakerRegistration<4>,
}

impl<M: RawMutex + 'static> ConnectionSlotState<M> {
    const fn new() -> Self {
        Self {
            owners: AtomicU8::new(0),
            index: AtomicU8::new(0),
            availability: BlockingMutex::new(Cell::new(None)),
            closed: BlockingMutex::new(RefCell::new(CloseState {
                requested: false,
                waiters: MultiWakerRegistration::new(),
            })),
        }
    }

    fn add_owner(&self) {
        self.owners.fetch_add(1, Ordering::AcqRel);
    }

    fn request_close(&self) {
        self.closed.lock(|closed| {
            let mut closed = closed.borrow_mut();
            closed.requested = true;
            closed.waiters.wake();
        });
    }

    fn release(&self) {
        self.request_close();
        if self.owners.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        if let Some(availability) = self.availability.lock(Cell::get) {
            availability.release(1);
        }
    }
}

impl<M: RawMutex + 'static, const SLOTS: usize> ConnectionSlotPool<M, SLOTS> {
    #[must_use]
    pub const fn new() -> Self {
        assert!(SLOTS <= u8::MAX as usize + 1);
        Self {
            availability: FairSemaphore::new(SLOTS),
            slots: [const { ConnectionSlotState::new() }; SLOTS],
        }
    }

    pub async fn acquire(
        &'static self,
    ) -> Result<ConnectionSlotLease<M>, ConnectionSlotAcquireError> {
        let permit = self
            .availability
            .acquire(1)
            .await
            .map_err(|_| ConnectionSlotAcquireError::WaitQueueFull)?;
        self.claim(permit)
            .ok_or(ConnectionSlotAcquireError::PermitWithoutAvailableSlot)
    }

    pub fn try_acquire(
        &'static self,
    ) -> Result<Option<ConnectionSlotLease<M>>, ConnectionSlotAcquireError> {
        let Some(permit) = self.availability.try_acquire(1) else {
            return Ok(None);
        };
        self.claim(permit)
            .map(Some)
            .ok_or(ConnectionSlotAcquireError::PermitWithoutAvailableSlot)
    }

    pub fn request_close(&self, index: usize) {
        if let Some(slot) = self.slots.get(index) {
            slot.request_close();
        }
    }

    fn claim(
        &'static self,
        permit: SemaphoreReleaser<'static, Availability<M>>,
    ) -> Option<ConnectionSlotLease<M>> {
        for index in 0..SLOTS {
            let slot = &self.slots[index];
            if slot
                .owners
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.index.store(index as u8, Ordering::Release);
                slot.availability
                    .lock(|availability| availability.set(Some(&self.availability)));
                slot.closed.lock(|closed| {
                    *closed.borrow_mut() = CloseState {
                        requested: false,
                        waiters: MultiWakerRegistration::new(),
                    };
                });
                permit.disarm();
                return Some(ConnectionSlotLease {
                    owner: ConnectionSlotOwner { slot },
                });
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConnectionSlotAcquireError {
    WaitQueueFull,
    PermitWithoutAvailableSlot,
}

impl<M: RawMutex + 'static, const SLOTS: usize> Default for ConnectionSlotPool<M, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub struct ConnectionSlotLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

struct ConnectionSlotOwner<M: RawMutex + 'static> {
    slot: &'static ConnectionSlotState<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotOwner<M> {
    fn index(&self) -> usize {
        usize::from(self.slot.index.load(Ordering::Acquire))
    }

    fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        poll_fn(|cx| {
            self.slot.closed.lock(|closed| {
                let mut closed = closed.borrow_mut();
                if closed.requested {
                    Poll::Ready(())
                } else {
                    closed.waiters.register(cx.waker());
                    Poll::Pending
                }
            })
        })
    }

    fn split(self) -> (Self, Self) {
        self.slot.add_owner();
        let slot = self.slot;
        core::mem::forget(self);
        (Self { slot }, Self { slot })
    }
}

impl<M: RawMutex + 'static> Drop for ConnectionSlotOwner<M> {
    fn drop(&mut self) {
        self.slot.release();
    }
}

impl<M: RawMutex + 'static> ConnectionSlotLease<M> {
    #[must_use]
    pub fn index(&self) -> usize {
        self.owner.index()
    }

    pub fn activate(self) -> ConnectionSlotOwners<M> {
        let (worker, link) = self.owner.split();
        ConnectionSlotOwners {
            worker: ConnectionSlotWorkerLease { owner: worker },
            link: ConnectionSlotLinkLease { owner: link },
        }
    }
}

#[must_use]
pub struct ConnectionSlotOwners<M: RawMutex + 'static> {
    pub worker: ConnectionSlotWorkerLease<M>,
    pub link: ConnectionSlotLinkLease<M>,
}

#[must_use]
pub struct ConnectionSlotWorkerLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotWorkerLease<M> {
    pub fn is_closed(&self) -> bool {
        self.owner
            .slot
            .closed
            .lock(|closed| closed.borrow().requested)
    }

    pub fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        self.owner.wait_for_close()
    }

    pub fn request_close(&self) {
        self.owner.slot.request_close();
    }
}

#[must_use]
pub struct ConnectionSlotLinkLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotLinkLease<M> {
    pub fn request_close(&self) {
        self.owner.slot.request_close();
    }

    #[must_use]
    pub fn index(&self) -> usize {
        self.owner.index()
    }

    pub fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        self.owner.wait_for_close()
    }

    pub fn into_ready(self, origin: Origin) -> ReadyConnectionSlot<M> {
        ReadyConnectionSlot { link: self, origin }
    }

    pub fn into_data(self) -> ConnectionSlotDataOwners<M> {
        let (source, sink) = self.owner.split();
        ConnectionSlotDataOwners {
            source: ConnectionSlotSourceLease { owner: source },
            sink: ConnectionSlotSinkLease { owner: sink },
        }
    }

    /// Native sessions retain the original link owner as their control lease.
    /// Columba must use `into_data`, without creating and dropping a control owner.
    pub fn into_parts(self) -> ConnectionSlotParts<M> {
        let (control, data) = self.owner.split();
        let ConnectionSlotDataOwners { source, sink } =
            ConnectionSlotLinkLease { owner: data }.into_data();
        ConnectionSlotParts {
            control: ConnectionSlotLinkLease { owner: control },
            source,
            sink,
        }
    }
}

#[must_use]
pub struct ConnectionSlotParts<M: RawMutex + 'static> {
    pub control: ConnectionSlotLinkLease<M>,
    pub source: ConnectionSlotSourceLease<M>,
    pub sink: ConnectionSlotSinkLease<M>,
}

#[must_use]
pub struct ConnectionSlotDataOwners<M: RawMutex + 'static> {
    pub source: ConnectionSlotSourceLease<M>,
    pub sink: ConnectionSlotSinkLease<M>,
}

#[must_use]
pub struct ConnectionSlotSourceLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotSourceLease<M> {
    pub fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        self.owner.wait_for_close()
    }
}

#[must_use]
pub struct ConnectionSlotSinkLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotSinkLease<M> {
    pub fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        self.owner.wait_for_close()
    }
}

#[must_use]
pub struct ReadyConnectionSlot<M: RawMutex + 'static> {
    link: ConnectionSlotLinkLease<M>,
    origin: Origin,
}

impl<M: RawMutex + 'static> ReadyConnectionSlot<M> {
    pub fn into_parts(self) -> ReadyConnectionSlotParts<M> {
        ReadyConnectionSlotParts {
            origin: self.origin,
            link: self.link,
        }
    }
}

#[must_use]
pub struct ReadyConnectionSlotParts<M: RawMutex + 'static> {
    pub origin: Origin,
    pub link: ConnectionSlotLinkLease<M>,
}

#[cfg(test)]
mod tests {
    use core::future::ready;
    use core::future::Future;
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Waker};
    use std::boxed::Box;
    use std::sync::Arc;
    use std::task::Wake;

    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use prns_core::interfaces::bluetooth_auto::Origin;

    use super::{
        ConnectionSlotAcquireError, ConnectionSlotDataOwners, ConnectionSlotOwners,
        ConnectionSlotParts, ConnectionSlotPool, ReadyConnectionSlotParts,
    };

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn native_split_retains_all_owners_until_control_and_worker_exit() {
        let pool = Box::leak(Box::new(
            ConnectionSlotPool::<CriticalSectionRawMutex, 1>::new(),
        ));
        let lease = pool.try_acquire().ok().flatten();
        assert!(lease.is_some(), "fresh slot must be available");
        let Some(lease) = lease else {
            return;
        };
        let ConnectionSlotOwners { worker, link } = lease.activate();
        let ConnectionSlotParts {
            source,
            sink,
            control,
        } = link.into_parts();
        assert!(matches!(
            block_on(select(worker.wait_for_close(), ready(()))),
            Either::Second(())
        ));
        drop(source);
        assert!(matches!(
            block_on(select(worker.wait_for_close(), ready(()))),
            Either::First(())
        ));
        drop(sink);
        drop(worker);
        assert_eq!(pool.try_acquire().map(|slot| slot.is_none()), Ok(true));
        assert!(matches!(
            block_on(select(control.wait_for_close(), ready(()))),
            Either::First(())
        ));
        drop(control);
        assert_eq!(pool.try_acquire().map(|slot| slot.is_some()), Ok(true));
    }

    #[test]
    fn close_is_broadcast_and_latched_for_every_native_owner() {
        let pool = Box::leak(Box::new(
            ConnectionSlotPool::<CriticalSectionRawMutex, 1>::new(),
        ));
        let lease = pool.try_acquire().ok().flatten();
        assert!(lease.is_some(), "fresh slot must be available");
        let Some(lease) = lease else {
            return;
        };
        let ConnectionSlotOwners { worker, link } = lease.activate();
        let ConnectionSlotParts {
            source,
            sink,
            control,
        } = link.into_parts();
        let wakes: [_; 4] = core::array::from_fn(|_| Arc::new(WakeCounter::default()));
        let wakers = wakes.each_ref().map(|wake| Waker::from(Arc::clone(wake)));
        {
            let mut worker_wait = pin!(worker.wait_for_close());
            let mut source_wait = pin!(source.wait_for_close());
            let mut sink_wait = pin!(sink.wait_for_close());
            let mut control_wait = pin!(control.wait_for_close());
            assert!(worker_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[0]))
                .is_pending());
            assert!(source_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[1]))
                .is_pending());
            assert!(sink_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[2]))
                .is_pending());
            assert!(control_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[3]))
                .is_pending());
            pool.request_close(0);
            assert_eq!(
                wakes.each_ref().map(|wake| wake.0.load(Ordering::Relaxed)),
                [1; 4]
            );
            assert!(worker_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[0]))
                .is_ready());
            assert!(source_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[1]))
                .is_ready());
            assert!(sink_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[2]))
                .is_ready());
            assert!(control_wait
                .as_mut()
                .poll(&mut Context::from_waker(&wakers[3]))
                .is_ready());
        }
        assert!(matches!(
            block_on(select(control.wait_for_close(), ready(()))),
            Either::First(())
        ));
        drop(control);
        drop(source);
        drop(sink);
        assert_eq!(pool.try_acquire().map(|slot| slot.is_none()), Ok(true));
        drop(worker);
        let reused = pool.try_acquire().ok().flatten();
        assert!(reused.is_some(), "last drop must release the slot");
        let Some(reused) = reused else {
            return;
        };
        let ConnectionSlotOwners { worker, link } = reused.activate();
        assert!(matches!(
            block_on(select(worker.wait_for_close(), ready(()))),
            Either::Second(())
        ));
        drop(link);
        assert!(matches!(
            block_on(select(worker.wait_for_close(), ready(()))),
            Either::First(())
        ));
    }

    #[test]
    fn cancelled_close_waiters_cannot_exhaust_or_consume_the_close_notification() {
        let pool = Box::leak(Box::new(
            ConnectionSlotPool::<CriticalSectionRawMutex, 1>::new(),
        ));
        let lease = pool.try_acquire().ok().flatten();
        assert!(lease.is_some());
        let Some(lease) = lease else { return };
        let ConnectionSlotOwners { worker, link } = lease.activate();
        let stale: [_; 4] = core::array::from_fn(|_| Arc::new(WakeCounter::default()));
        for wake in &stale {
            let waker = Waker::from(Arc::clone(wake));
            let mut waiting = pin!(link.wait_for_close());
            assert!(waiting
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending());
            // Leave four different canceled task registrations, filling the bounded list.
        }
        let live = Arc::new(WakeCounter::default());
        let live_waker = Waker::from(Arc::clone(&live));
        let mut waiting = pin!(link.wait_for_close());
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(&live_waker))
            .is_pending());
        assert_eq!(
            stale.each_ref().map(|wake| wake.0.load(Ordering::Relaxed)),
            [1; 4]
        );
        worker.request_close();
        assert_eq!(live.0.load(Ordering::Relaxed), 1);
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(&live_waker))
            .is_ready());
        assert!(matches!(
            block_on(select(worker.wait_for_close(), ready(()))),
            Either::First(())
        ));
    }

    #[test]
    fn leases_reserve_unique_slots_until_drop() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 3> = ConnectionSlotPool::new();

        let first = POOL.try_acquire().ok().flatten();
        let second = POOL.try_acquire().ok().flatten();
        let third = POOL.try_acquire().ok().flatten();

        assert_eq!(first.as_ref().map(|lease| lease.index()), Some(0));
        assert_eq!(second.as_ref().map(|lease| lease.index()), Some(1));
        assert_eq!(third.as_ref().map(|lease| lease.index()), Some(2));
        assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));

        drop(second);
        let replacement = POOL.try_acquire().ok().flatten();
        assert_eq!(replacement.as_ref().map(|lease| lease.index()), Some(1));
    }

    #[test]
    fn split_lease_releases_after_both_owners_drop() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 1> = ConnectionSlotPool::new();

        let lease = POOL.try_acquire().ok().flatten();
        assert!(lease.is_some());
        if let Some(lease) = lease {
            let ConnectionSlotOwners { worker, link } = lease.activate();
            let ready = link.into_ready(Origin::Dialed);
            let ReadyConnectionSlotParts { origin, link } = ready.into_parts();
            assert_eq!(origin, Origin::Dialed);

            drop(worker);
            assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));
            let ConnectionSlotDataOwners { source, sink } = link.into_data();
            drop(source);
            assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));
            drop(sink);
            let replacement = POOL.try_acquire().ok().flatten();
            assert!(replacement.is_some());
            assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));
        }
    }

    #[test]
    fn cancelled_acquisition_leaves_capacity_available() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 1> = ConnectionSlotPool::new();

        let held = POOL.try_acquire().ok().flatten();
        assert!(held.is_some());
        block_on(async {
            assert!(matches!(
                select(POOL.acquire(), ready(())).await,
                Either::Second(())
            ));
        });
        drop(held);
        assert_eq!(POOL.try_acquire().map(|lease| lease.is_some()), Ok(true));
    }

    #[test]
    fn wait_queue_saturation_is_explicit() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 1> = ConnectionSlotPool::new();

        let held = POOL.try_acquire().ok().flatten();
        assert!(held.is_some());
        block_on(async {
            assert!(matches!(
                select(POOL.acquire(), POOL.acquire()).await,
                Either::Second(Err(ConnectionSlotAcquireError::WaitQueueFull))
            ));
        });
        drop(held);
        assert_eq!(POOL.try_acquire().map(|lease| lease.is_some()), Ok(true));
    }

    #[test]
    fn owner_drop_closes_peer_and_reuse_resets_close_signal() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 1> = ConnectionSlotPool::new();

        let lease = POOL.try_acquire().ok().flatten();
        assert!(lease.is_some());
        if let Some(lease) = lease {
            let ConnectionSlotOwners { worker, link } = lease.activate();
            drop(link);
            block_on(async {
                assert!(matches!(
                    select(worker.wait_for_close(), ready(())).await,
                    Either::First(())
                ));
            });
            assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));
            drop(worker);

            let reused = POOL.try_acquire().ok().flatten();
            assert!(reused.is_some());
            if let Some(reused) = reused {
                let ConnectionSlotOwners { worker, link } = reused.activate();
                block_on(async {
                    assert!(matches!(
                        select(worker.wait_for_close(), ready(())).await,
                        Either::Second(())
                    ));
                });
                drop(link);
                drop(worker);
            }
        }
    }

    #[test]
    fn explicit_close_requests_wake_workers_without_releasing_capacity() {
        static POOL: ConnectionSlotPool<CriticalSectionRawMutex, 1> = ConnectionSlotPool::new();

        let lease = POOL.try_acquire().ok().flatten();
        assert!(lease.is_some());
        if let Some(lease) = lease {
            let ConnectionSlotOwners { worker, link } = lease.activate();
            POOL.request_close(0);
            block_on(async {
                assert!(matches!(
                    select(worker.wait_for_close(), ready(())).await,
                    Either::First(())
                ));
            });
            assert_eq!(POOL.try_acquire().map(|lease| lease.is_none()), Ok(true));
            drop(link);
            drop(worker);

            let reused = POOL.try_acquire().ok().flatten();
            assert!(reused.is_some());
            if let Some(reused) = reused {
                let ConnectionSlotOwners { worker, link } = reused.activate();
                worker.request_close();
                block_on(async {
                    assert!(matches!(
                        select(worker.wait_for_close(), ready(())).await,
                        Either::First(())
                    ));
                });
                drop(link);
                drop(worker);
            }
        }
    }
}
