#[cfg(target_has_atomic = "64")]
use core::sync::atomic::Ordering;

#[cfg(not(target_has_atomic = "64"))]
use core::cell::Cell;
#[cfg(not(target_has_atomic = "64"))]
use critical_section::Mutex;

pub struct AtomicU64 {
    #[cfg(target_has_atomic = "64")]
    value: core::sync::atomic::AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    value: Mutex<Cell<u64>>,
}

impl AtomicU64 {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            value: core::sync::atomic::AtomicU64::new(value),
            #[cfg(not(target_has_atomic = "64"))]
            value: Mutex::new(Cell::new(value)),
        }
    }

    #[must_use]
    pub fn load_relaxed(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        return self.value.load(Ordering::Relaxed);

        #[cfg(not(target_has_atomic = "64"))]
        critical_section::with(|cs| self.value.borrow(cs).get())
    }

    #[must_use]
    pub fn load_acquire(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        return self.value.load(Ordering::Acquire);

        #[cfg(not(target_has_atomic = "64"))]
        critical_section::with(|cs| self.value.borrow(cs).get())
    }

    pub fn store_relaxed(&self, value: u64) {
        #[cfg(target_has_atomic = "64")]
        return self.value.store(value, Ordering::Relaxed);

        #[cfg(not(target_has_atomic = "64"))]
        critical_section::with(|cs| self.value.borrow(cs).set(value));
    }

    pub fn store_release(&self, value: u64) {
        #[cfg(target_has_atomic = "64")]
        return self.value.store(value, Ordering::Release);

        #[cfg(not(target_has_atomic = "64"))]
        critical_section::with(|cs| self.value.borrow(cs).set(value));
    }

    pub fn fetch_add_relaxed(&self, value: u64) -> u64 {
        #[cfg(target_has_atomic = "64")]
        return self.value.fetch_add(value, Ordering::Relaxed);

        #[cfg(not(target_has_atomic = "64"))]
        critical_section::with(|cs| {
            let previous = self.value.borrow(cs).get();
            self.value.borrow(cs).set(previous.wrapping_add(value));
            previous
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AtomicU64;

    #[test]
    fn operations_preserve_atomic_u64_semantics() {
        let value = AtomicU64::new(u64::MAX);

        assert_eq!(value.load_relaxed(), u64::MAX);
        assert_eq!(value.fetch_add_relaxed(1), u64::MAX);
        assert_eq!(value.load_acquire(), 0);

        value.store_relaxed(41);
        assert_eq!(value.fetch_add_relaxed(1), 41);
        value.store_release(7);
        assert_eq!(value.load_acquire(), 7);
    }
}
