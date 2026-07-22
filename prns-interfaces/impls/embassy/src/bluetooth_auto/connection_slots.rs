use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use embassy_sync::signal::Signal;
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
    closed: Signal<M, ()>,
}

impl<M: RawMutex + 'static> ConnectionSlotState<M> {
    const fn new() -> Self {
        Self {
            owners: AtomicU8::new(0),
            index: AtomicU8::new(0),
            availability: BlockingMutex::new(Cell::new(None)),
            closed: Signal::new(),
        }
    }

    fn add_owner(&self) {
        self.owners.fetch_add(1, Ordering::AcqRel);
    }

    fn release(&self) {
        self.closed.signal(());
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
                slot.closed.reset();
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
        self.slot.closed.wait()
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
    pub fn wait_for_close(&self) -> impl core::future::Future<Output = ()> + '_ {
        self.owner.wait_for_close()
    }
}

#[must_use]
pub struct ConnectionSlotLinkLease<M: RawMutex + 'static> {
    owner: ConnectionSlotOwner<M>,
}

impl<M: RawMutex + 'static> ConnectionSlotLinkLease<M> {
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

    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use prns_core::interfaces::bluetooth_auto::Origin;

    use super::{
        ConnectionSlotAcquireError, ConnectionSlotDataOwners, ConnectionSlotOwners,
        ConnectionSlotPool, ReadyConnectionSlotParts,
    };

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
}
