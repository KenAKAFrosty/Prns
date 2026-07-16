use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::FnvIndexMap;

use crate::engine::InterfaceCounts;
use crate::interfaces::InterfaceId;

pub(crate) trait InterfaceCountStore: Sync {
    const RETAINS_COUNTS: bool;

    fn set_interface_counts(&self, interface: InterfaceId, counts: InterfaceCounts);
    fn forget_interface(&self, interface: InterfaceId);
    fn signal_interface_counts_changed(&self);
}

pub(crate) struct NoInterfaceCountStore;

impl InterfaceCountStore for NoInterfaceCountStore {
    const RETAINS_COUNTS: bool = false;

    fn set_interface_counts(&self, _interface: InterfaceId, _counts: InterfaceCounts) {}

    fn forget_interface(&self, _interface: InterfaceId) {}

    fn signal_interface_counts_changed(&self) {}
}

pub struct EmbassyInterfaceStore<M: RawMutex, const N: usize> {
    counts: Mutex<M, RefCell<FnvIndexMap<InterfaceId, InterfaceCounts, N>>>,
    signal: Signal<M, ()>,
}

impl<M: RawMutex, const N: usize> Default for EmbassyInterfaceStore<M, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const N: usize> EmbassyInterfaceStore<M, N> {
    #[must_use]
    pub const fn new() -> Self {
        const {
            assert!(
                N.is_power_of_two(),
                "EmbassyInterfaceStore N must be a power of two: heapless::FnvIndexMap requires it"
            )
        };
        Self {
            counts: Mutex::new(RefCell::new(FnvIndexMap::new())),
            signal: Signal::new(),
        }
    }

    #[must_use]
    pub fn counts(&self, interface: InterfaceId) -> InterfaceCounts {
        self.counts
            .lock(|cell| cell.borrow().get(&interface).copied().unwrap_or_default())
    }

    pub async fn changed(&self) {
        self.signal.wait().await;
    }
}

impl<M: RawMutex + Sync, const N: usize> InterfaceCountStore for EmbassyInterfaceStore<M, N> {
    const RETAINS_COUNTS: bool = true;

    fn set_interface_counts(&self, interface: InterfaceId, counts: InterfaceCounts) {
        self.counts.lock(|cell| {
            let stored = cell.borrow_mut().insert(interface, counts);
            assert!(
                stored.is_ok(),
                "EmbassyInterfaceStore capacity N is smaller than the live interface count"
            );
        });
    }

    fn forget_interface(&self, interface: InterfaceId) {
        self.counts.lock(|cell| {
            let _ = cell.borrow_mut().remove(&interface);
        });
    }

    fn signal_interface_counts_changed(&self) {
        self.signal.signal(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::INTERFACE_ID_LEN;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    #[test]
    fn set_reads_back_per_interface_in_fixed_capacity() {
        let store = EmbassyInterfaceStore::<CriticalSectionRawMutex, 8>::new();
        let interface = InterfaceId::new([5; INTERFACE_ID_LEN]);

        assert_eq!(store.counts(interface), InterfaceCounts::default());

        store.set_interface_counts(
            interface,
            InterfaceCounts {
                destinations: 2,
                links: 1,
                transported_links: 4,
            },
        );

        assert_eq!(store.counts(interface).transported_links, 4);
    }
}
