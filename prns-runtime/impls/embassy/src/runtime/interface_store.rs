use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::FnvIndexMap;

use crate::engine::InterfaceCounts;
use crate::interfaces::{InterfaceId, PacketPhyStats};
use crate::routing::dedup::PacketHash;

use prns_runtime::runtime::packet_phy_retention::{
    fixed_packet_phy_retention, FixedPacketPhyRetention,
};

#[must_use]
pub const fn minimum_interface_store_capacity(interface_capacity: usize) -> usize {
    assert!(interface_capacity > 0);
    interface_capacity.next_power_of_two()
}

pub(crate) trait InterfaceInspectionStore: Sync {
    const RETAINS_COUNTS: bool;
    const RETAINS_PACKET_PHY: bool;

    fn set_interface_counts(&self, interface: InterfaceId, counts: InterfaceCounts);
    fn forget_interface(&self, interface: InterfaceId);
    fn signal_interface_counts_changed(&self);
    fn remember_packet_phy(&self, packet_hash: PacketHash, stats: PacketPhyStats);
}

pub(crate) struct NoInterfaceInspectionStore;

impl InterfaceInspectionStore for NoInterfaceInspectionStore {
    const RETAINS_COUNTS: bool = false;
    const RETAINS_PACKET_PHY: bool = false;

    fn set_interface_counts(&self, _interface: InterfaceId, _counts: InterfaceCounts) {}

    fn forget_interface(&self, _interface: InterfaceId) {}

    fn signal_interface_counts_changed(&self) {}

    fn remember_packet_phy(&self, _packet_hash: PacketHash, _stats: PacketPhyStats) {}
}

pub struct EmbassyInterfaceStore<
    M: RawMutex,
    const INTERFACES: usize,
    const PACKET_PHY_CAPACITY: usize,
    const PACKET_PHY_INDEX_BUCKETS: usize,
> {
    counts: Mutex<M, RefCell<FnvIndexMap<InterfaceId, InterfaceCounts, INTERFACES>>>,
    packet_phy:
        Mutex<M, RefCell<FixedPacketPhyRetention<PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>>>,
    signal: Signal<M, ()>,
}

impl<
        M: RawMutex,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    > Default
    for EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        M: RawMutex,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    > EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>
{
    #[must_use]
    pub const fn new() -> Self {
        const {
            assert!(
                INTERFACES.is_power_of_two(),
                "EmbassyInterfaceStore INTERFACES must be a power of two: heapless::FnvIndexMap requires it"
            )
        };
        Self {
            counts: Mutex::new(RefCell::new(FnvIndexMap::new())),
            packet_phy: Mutex::new(RefCell::new(fixed_packet_phy_retention())),
            signal: Signal::new(),
        }
    }

    #[must_use]
    pub fn counts(&self, interface: InterfaceId) -> InterfaceCounts {
        self.counts
            .lock(|cell| cell.borrow().get(&interface).copied().unwrap_or_default())
    }

    #[must_use]
    pub fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats> {
        self.packet_phy.lock(|cell| cell.borrow().get(packet_hash))
    }

    pub async fn changed(&self) {
        self.signal.wait().await;
    }
}

impl<
        M: RawMutex + Sync,
        const INTERFACES: usize,
        const PACKET_PHY_CAPACITY: usize,
        const PACKET_PHY_INDEX_BUCKETS: usize,
    > InterfaceInspectionStore
    for EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>
{
    const RETAINS_COUNTS: bool = true;
    const RETAINS_PACKET_PHY: bool = true;

    fn set_interface_counts(&self, interface: InterfaceId, counts: InterfaceCounts) {
        self.counts.lock(|cell| {
            let stored = cell.borrow_mut().insert(interface, counts);
            assert!(
                stored.is_ok(),
                "EmbassyInterfaceStore INTERFACES is smaller than the live interface count"
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

    fn remember_packet_phy(&self, packet_hash: PacketHash, stats: PacketPhyStats) {
        if stats.is_empty() {
            return;
        }
        self.packet_phy
            .lock(|cell| cell.borrow_mut().remember(packet_hash, stats));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{RssiDbm, INTERFACE_ID_LEN};
    use crate::routing::dedup::dedup_index_buckets;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    #[test]
    fn interface_store_capacity_covers_the_interface_ceiling_with_a_power_of_two() {
        assert_eq!(minimum_interface_store_capacity(1), 1);
        assert_eq!(minimum_interface_store_capacity(7), 8);
        assert_eq!(minimum_interface_store_capacity(24), 32);
    }

    #[test]
    fn fixed_store_reads_interface_counts_and_packet_phy() {
        const PACKET_PHY_CAPACITY: usize = 8;
        const PACKET_PHY_INDEX_BUCKETS: usize = dedup_index_buckets(PACKET_PHY_CAPACITY);

        let store = EmbassyInterfaceStore::<
            CriticalSectionRawMutex,
            8,
            PACKET_PHY_CAPACITY,
            PACKET_PHY_INDEX_BUCKETS,
        >::new();
        let interface = InterfaceId::new([5; INTERFACE_ID_LEN]);
        let packet_hash = PacketHash::new([7; 32]);
        let packet_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-87)),
            snr: None,
            quality: None,
        };

        assert_eq!(store.counts(interface), InterfaceCounts::default());
        assert_eq!(store.packet_phy(packet_hash), None);

        store.set_interface_counts(
            interface,
            InterfaceCounts {
                destinations: 2,
                links: 1,
                transported_links: 4,
            },
        );
        store.remember_packet_phy(packet_hash, packet_phy);

        assert_eq!(store.counts(interface).transported_links, 4);
        assert_eq!(store.packet_phy(packet_hash), Some(packet_phy));
    }
}
