use core::cell::Cell;
use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use heapless::Vec as FrameBytes;
use portable_atomic::{AtomicBool, Ordering};

type Availability<M, const WAITERS: usize> = FairSemaphore<M, WAITERS>;

pub(super) struct SharedFramePool<
    M: RawMutex + 'static,
    const FRAME: usize,
    const CAPACITY: usize,
    const WAITERS: usize,
> {
    availability: Availability<M, WAITERS>,
    slots: [FrameSlot<M, FRAME>; CAPACITY],
}

struct FrameSlot<M: RawMutex + 'static, const FRAME: usize> {
    claimed: AtomicBool,
    frame: Mutex<M, FrameBytes<u8, FRAME>>,
}

impl<M: RawMutex + 'static, const FRAME: usize> FrameSlot<M, FRAME> {
    const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            frame: Mutex::new(FrameBytes::new()),
        }
    }
}

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize, const WAITERS: usize>
    SharedFramePool<M, FRAME, CAPACITY, WAITERS>
{
    #[must_use]
    pub const fn new() -> Self {
        assert!(CAPACITY > 0);
        assert!(CAPACITY <= u8::MAX as usize + 1);
        assert!(WAITERS > 0);
        Self {
            availability: FairSemaphore::new(CAPACITY),
            slots: [const { FrameSlot::new() }; CAPACITY],
        }
    }

    pub async fn lease(
        &'static self,
    ) -> Result<FrameLease<M, FRAME, CAPACITY, WAITERS>, FramePoolError> {
        let permit = self
            .availability
            .acquire(1)
            .await
            .map_err(|_| FramePoolError::WaitQueueFull)?;
        let lease = self
            .claim(permit)
            .ok_or(FramePoolError::PermitWithoutAvailableSlot)?;
        lease.lock().await.clear();
        Ok(lease)
    }

    pub fn try_lease(
        &'static self,
    ) -> Result<Option<FrameLease<M, FRAME, CAPACITY, WAITERS>>, FramePoolError> {
        let Some(permit) = self.availability.try_acquire(1) else {
            return Ok(None);
        };
        let lease = self
            .claim(permit)
            .ok_or(FramePoolError::PermitWithoutAvailableSlot)?;
        lease
            .slot()
            .frame
            .try_lock()
            .map_err(|_| FramePoolError::SlotBusy)?
            .clear();
        Ok(Some(lease))
    }

    fn claim(
        &'static self,
        permit: SemaphoreReleaser<'static, Availability<M, WAITERS>>,
    ) -> Option<FrameLease<M, FRAME, CAPACITY, WAITERS>> {
        for index in 0..CAPACITY {
            if self.slots[index]
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                permit.disarm();
                return Some(FrameLease {
                    pool: self,
                    index: index as u8,
                    not_sync: PhantomData,
                });
            }
        }
        None
    }
}

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize, const WAITERS: usize> Default
    for SharedFramePool<M, FRAME, CAPACITY, WAITERS>
{
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub(super) struct FrameLease<
    M: RawMutex + 'static,
    const FRAME: usize,
    const CAPACITY: usize,
    const WAITERS: usize,
> {
    pool: &'static SharedFramePool<M, FRAME, CAPACITY, WAITERS>,
    index: u8,
    not_sync: PhantomData<Cell<()>>,
}

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize, const WAITERS: usize>
    FrameLease<M, FRAME, CAPACITY, WAITERS>
{
    fn slot(&self) -> &FrameSlot<M, FRAME> {
        &self.pool.slots[usize::from(self.index)]
    }

    pub async fn lock(&self) -> MutexGuard<'_, M, FrameBytes<u8, FRAME>> {
        self.slot().frame.lock().await
    }

    pub async fn fill(&self, bytes: &[u8]) -> Result<(), FramePoolError> {
        if bytes.len() > FRAME {
            return Err(FramePoolError::FrameTooLarge {
                len: bytes.len(),
                capacity: FRAME,
            });
        }
        let mut frame = self.lock().await;
        frame.clear();
        frame
            .extend_from_slice(bytes)
            .map_err(|_| FramePoolError::FrameTooLarge {
                len: bytes.len(),
                capacity: FRAME,
            })
    }

    pub fn try_fill(&self, bytes: &[u8]) -> Result<(), FramePoolError> {
        if bytes.len() > FRAME {
            return Err(FramePoolError::FrameTooLarge {
                len: bytes.len(),
                capacity: FRAME,
            });
        }
        let mut frame = self
            .slot()
            .frame
            .try_lock()
            .map_err(|_| FramePoolError::SlotBusy)?;
        frame.clear();
        frame
            .extend_from_slice(bytes)
            .map_err(|_| FramePoolError::FrameTooLarge {
                len: bytes.len(),
                capacity: FRAME,
            })
    }
}

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize, const WAITERS: usize> Drop
    for FrameLease<M, FRAME, CAPACITY, WAITERS>
{
    fn drop(&mut self) {
        self.slot().claimed.store(false, Ordering::Release);
        self.pool.availability.release(1);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FramePoolError {
    FrameTooLarge { len: usize, capacity: usize },
    SlotBusy,
    WaitQueueFull,
    PermitWithoutAvailableSlot,
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use embassy_futures::block_on;
    use embassy_futures::select::{select, select3, Either, Either3};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    use super::{FramePoolError, SharedFramePool};

    #[test]
    fn leases_are_exclusive_and_release_on_drop() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 8, 2, 2> = SharedFramePool::new();

        let first = POOL.try_lease().ok().flatten();
        let second = POOL.try_lease().ok().flatten();
        assert!(first.is_some());
        assert!(second.is_some());
        assert_eq!(POOL.try_lease().map(|lease| lease.is_none()), Ok(true));

        drop(first);
        let replacement = POOL.try_lease().ok().flatten();
        assert!(replacement.is_some());
        assert_eq!(POOL.try_lease().map(|lease| lease.is_none()), Ok(true));
    }

    #[test]
    fn frames_are_bounded_and_reused() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 4, 1, 1> = SharedFramePool::new();

        block_on(async {
            let lease = POOL.lease().await;
            assert!(lease.is_ok());
            if let Ok(lease) = lease {
                assert_eq!(lease.fill(b"prns").await, Ok(()));
                {
                    let frame = lease.lock().await;
                    assert_eq!(frame.as_slice(), b"prns");
                    assert_eq!(lease.try_fill(b"rns"), Err(FramePoolError::SlotBusy));
                }
                assert_eq!(
                    lease.fill(b"large").await,
                    Err(FramePoolError::FrameTooLarge {
                        len: 5,
                        capacity: 4,
                    })
                );
                drop(lease);

                let reused = POOL.lease().await;
                assert!(reused.is_ok());
                if let Ok(reused) = reused {
                    assert!(reused.lock().await.is_empty());
                    assert_eq!(reused.fill(b"rns").await, Ok(()));
                    let frame = reused.lock().await;
                    assert_eq!(frame.as_slice(), b"rns");
                }
            }
        });
    }

    #[test]
    fn cancelled_lease_leaves_capacity_available() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 4, 1, 2> = SharedFramePool::new();

        let held = POOL.try_lease().ok().flatten();
        assert!(held.is_some());
        block_on(async {
            assert!(matches!(
                select(POOL.lease(), ready(())).await,
                Either::Second(())
            ));
        });
        drop(held);
        assert_eq!(POOL.try_lease().map(|lease| lease.is_some()), Ok(true));
    }

    #[test]
    fn wait_queue_saturation_is_explicit() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 4, 1, 2> = SharedFramePool::new();

        let held = POOL.try_lease().ok().flatten();
        assert!(held.is_some());
        block_on(async {
            assert!(matches!(
                select3(POOL.lease(), POOL.lease(), POOL.lease()).await,
                Either3::Third(Err(FramePoolError::WaitQueueFull))
            ));
        });
        drop(held);
        assert_eq!(POOL.try_lease().map(|lease| lease.is_some()), Ok(true));
    }
}
