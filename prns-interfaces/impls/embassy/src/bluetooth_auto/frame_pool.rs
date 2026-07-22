use core::cell::Cell;
use core::marker::PhantomData;

use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use heapless::Vec as FrameBytes;
use portable_atomic::{AtomicBool, Ordering};

type Availability<M, const CAPACITY: usize> = FairSemaphore<M, CAPACITY>;

pub(super) struct SharedFramePool<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize>
{
    availability: Availability<M, CAPACITY>,
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

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize>
    SharedFramePool<M, FRAME, CAPACITY>
{
    #[must_use]
    pub const fn new() -> Self {
        assert!(CAPACITY > 0);
        assert!(CAPACITY <= u8::MAX as usize + 1);
        Self {
            availability: FairSemaphore::new(CAPACITY),
            slots: [const { FrameSlot::new() }; CAPACITY],
        }
    }

    pub async fn lease(&'static self) -> FrameLease<M, FRAME, CAPACITY> {
        loop {
            let permit = match self.availability.acquire(1).await {
                Ok(permit) => permit,
                Err(_) => {
                    yield_now().await;
                    continue;
                }
            };
            if let Some(lease) = self.claim(permit) {
                return lease;
            }
            yield_now().await;
        }
    }

    pub fn try_lease(&'static self) -> Option<FrameLease<M, FRAME, CAPACITY>> {
        let permit = self.availability.try_acquire(1)?;
        self.claim(permit)
    }

    fn claim(
        &'static self,
        permit: SemaphoreReleaser<'static, Availability<M, CAPACITY>>,
    ) -> Option<FrameLease<M, FRAME, CAPACITY>> {
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

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize> Default
    for SharedFramePool<M, FRAME, CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub(super) struct FrameLease<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize> {
    pool: &'static SharedFramePool<M, FRAME, CAPACITY>,
    index: u8,
    not_sync: PhantomData<Cell<()>>,
}

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize>
    FrameLease<M, FRAME, CAPACITY>
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

impl<M: RawMutex + 'static, const FRAME: usize, const CAPACITY: usize> Drop
    for FrameLease<M, FRAME, CAPACITY>
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
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    use super::{FramePoolError, SharedFramePool};

    #[test]
    fn leases_are_exclusive_and_release_on_drop() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 8, 2> = SharedFramePool::new();

        let first = POOL.try_lease();
        let second = POOL.try_lease();
        assert!(first.is_some());
        assert!(second.is_some());
        assert!(POOL.try_lease().is_none());

        drop(first);
        assert!(POOL.try_lease().is_some());
    }

    #[test]
    fn frames_are_bounded_and_reused() {
        static POOL: SharedFramePool<CriticalSectionRawMutex, 4, 1> = SharedFramePool::new();

        block_on(async {
            let lease = POOL.lease().await;
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
            assert_eq!(reused.fill(b"rns").await, Ok(()));
            let frame = reused.lock().await;
            assert_eq!(frame.as_slice(), b"rns");
        });
    }
}
