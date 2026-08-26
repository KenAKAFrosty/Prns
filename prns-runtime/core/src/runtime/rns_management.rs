use alloc::string::String;
use alloc::vec::Vec;

use prns_core::interfaces::rns_management::{
    RnsAnnounceRateEntry, RnsAnnounceRateTable, RnsInterfaceAccessCode, RnsInterfaceStats,
    RnsInterfaceStatsEntry,
};

use super::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot, InterfaceInventoryEntry,
};

pub fn interface_stats(inventory: Vec<InterfaceInventoryEntry<String>>) -> RnsInterfaceStats {
    RnsInterfaceStats::new(
        logical_interface_inventory(inventory)
            .into_iter()
            .map(stats_entry_from_inventory)
            .collect(),
    )
}

fn stats_entry_from_inventory(entry: InterfaceInventoryEntry<String>) -> RnsInterfaceStatsEntry {
    let access_code = entry.ifac.map(|access_code| {
        RnsInterfaceAccessCode::new(
            access_code.signature,
            access_code.size,
            access_code.network_name,
        )
    });
    let fleet_peers = entry
        .members
        .into_iter()
        .map(|member| {
            RnsInterfaceStatsEntry::new(member.name, member.snapshot, None).with_rssi(member.rssi)
        })
        .collect();
    RnsInterfaceStatsEntry::new(entry.name, entry.snapshot, access_code)
        .with_rssi(entry.rssi)
        .with_fleet_peers(fleet_peers)
}

pub fn announce_rate_table(entries: Vec<AnnounceRateSnapshot>) -> RnsAnnounceRateTable {
    RnsAnnounceRateTable::new(
        entries
            .into_iter()
            .map(|entry| {
                RnsAnnounceRateEntry::new(
                    entry.destination,
                    entry.last_allowed_announce_at,
                    entry.blocked_until,
                    entry.rate_violations,
                    entry.observed_at,
                )
            })
            .collect(),
    )
}
