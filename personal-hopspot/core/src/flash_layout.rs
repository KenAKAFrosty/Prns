use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};

pub const HOPSPOT_FLASH_PAGE_BYTES: usize = 4096;

/// The durable regions coupled to one ESP32-S3 partition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HopspotS3FlashLayout {
    pub flash_capacity: usize,
    pub factory_end: u32,
    pub radio_profile_pages: [u32; 2],
    pub journal: FlashJournalLayout,
}

pub const S3_8_MIB_FLASH_LAYOUT: HopspotS3FlashLayout = HopspotS3FlashLayout {
    flash_capacity: 8 * 1024 * 1024,
    factory_end: 0x67E000,
    radio_profile_pages: [0x67E000, 0x67F000],
    journal: FlashJournalLayout::new(
        [0x680000, 0x681000],
        [
            FlashArenaRange::new(0x682000, 0x741000),
            FlashArenaRange::new(0x741000, 0x800000),
        ],
    ),
};

pub const S3_16_MIB_FLASH_LAYOUT: HopspotS3FlashLayout = HopspotS3FlashLayout {
    flash_capacity: 16 * 1024 * 1024,
    factory_end: 0xE7E000,
    radio_profile_pages: [0xE7E000, 0xE7F000],
    journal: FlashJournalLayout::new(
        [0xE80000, 0xE81000],
        [
            FlashArenaRange::new(0xE82000, 0xF41000),
            FlashArenaRange::new(0xF41000, 0x1000000),
        ],
    ),
};

pub const T_ECHO_RADIO_PROFILE_PAGES: [u32; 2] = [0xE9000, 0xEA000];
pub const T_ECHO_MIN_ARENA_BYTES: usize = 19 * HOPSPOT_FLASH_PAGE_BYTES;
pub const T_ECHO_JOURNAL_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xC0000, 0xC1000],
    [
        FlashArenaRange::new(0xC2000, 0xD6000),
        FlashArenaRange::new(0xD6000, 0xE9000),
    ],
);

// Seeed SenseCAP Wio Tracker T1000-E: same nRF52840 + S140 SoftDevice + memory.x
// as the T-Echo, so the on-chip flash partitioning is byte-identical. The board
// references these aliases so the intent ("T1000-E layout") reads clearly at the
// call site without implying a separate layout was laid down.
#[allow(dead_code)] // consumed by the t1000e board (personal-hopspot nrf52840 face)
pub const T1000E_RADIO_PROFILE_PAGES: [u32; 2] = T_ECHO_RADIO_PROFILE_PAGES;
#[allow(dead_code)] // consumed by the t1000e board (personal-hopspot nrf52840 face)
pub const T1000E_MIN_ARENA_BYTES: usize = T_ECHO_MIN_ARENA_BYTES;
#[allow(dead_code)] // consumed by the t1000e board (personal-hopspot nrf52840 face)
pub const T1000E_JOURNAL_LAYOUT: FlashJournalLayout = T_ECHO_JOURNAL_LAYOUT;

const _: () = {
    const PAGE: u32 = HOPSPOT_FLASH_PAGE_BYTES as u32;
    assert!(S3_8_MIB_FLASH_LAYOUT.factory_end == S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[0]);
    assert!(
        S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[0] + PAGE
            == S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[1]
    );
    assert!(
        S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[1] + PAGE
            == S3_8_MIB_FLASH_LAYOUT.journal.timebase_regions[0]
    );
    assert!(
        S3_8_MIB_FLASH_LAYOUT.journal.arenas[1].end as usize
            == S3_8_MIB_FLASH_LAYOUT.flash_capacity
    );

    assert!(S3_16_MIB_FLASH_LAYOUT.factory_end == S3_16_MIB_FLASH_LAYOUT.radio_profile_pages[0]);
    assert!(
        S3_16_MIB_FLASH_LAYOUT.radio_profile_pages[0] + PAGE
            == S3_16_MIB_FLASH_LAYOUT.radio_profile_pages[1]
    );
    assert!(
        S3_16_MIB_FLASH_LAYOUT.radio_profile_pages[1] + PAGE
            == S3_16_MIB_FLASH_LAYOUT.journal.timebase_regions[0]
    );
    assert!(
        S3_16_MIB_FLASH_LAYOUT.journal.arenas[1].end as usize
            == S3_16_MIB_FLASH_LAYOUT.flash_capacity
    );

    assert!(T_ECHO_JOURNAL_LAYOUT.arenas[1].end == T_ECHO_RADIO_PROFILE_PAGES[0]);
    assert!(T_ECHO_RADIO_PROFILE_PAGES[0] + PAGE == T_ECHO_RADIO_PROFILE_PAGES[1]);
    assert!(T_ECHO_RADIO_PROFILE_PAGES[1] + PAGE == 0xEB000);
};

#[cfg(test)]
mod tests {
    use super::*;

    fn partition(csv: &str, name: &str) -> (u32, u32) {
        csv.lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .find_map(|line| {
                let fields: std::vec::Vec<_> = line.split(',').map(str::trim).collect();
                (fields.first().copied() == Some(name)).then(|| {
                    let offset = u32::from_str_radix(fields[3].trim_start_matches("0x"), 16)
                        .expect("partition offset is hexadecimal");
                    let size = u32::from_str_radix(fields[4].trim_start_matches("0x"), 16)
                        .expect("partition size is hexadecimal");
                    (offset, size)
                })
            })
            .expect("named partition exists")
    }

    fn assert_s3_csv(csv: &str, layout: HopspotS3FlashLayout) {
        let (factory_offset, factory_size) = partition(csv, "factory");
        assert_eq!(factory_offset, 0x10000);
        assert_eq!(factory_offset + factory_size, layout.factory_end);

        let (profile_offset, profile_size) = partition(csv, "radio_cfg");
        assert_eq!(profile_offset, layout.radio_profile_pages[0]);
        assert_eq!(profile_size, 2 * HOPSPOT_FLASH_PAGE_BYTES as u32);

        let (journal_offset, journal_size) = partition(csv, "prns_state");
        assert_eq!(journal_offset, layout.journal.timebase_regions[0]);
        assert_eq!(journal_offset + journal_size, layout.flash_capacity as u32);
    }

    #[test]
    fn esp_partition_tables_match_the_firmware_layout_contract() {
        assert_s3_csv(
            include_str!("../../embedded/esp32/partitions-hopspot-8mb.csv"),
            S3_8_MIB_FLASH_LAYOUT,
        );
        assert_s3_csv(
            include_str!("../../embedded/esp32/partitions-hopspot-16mb.csv"),
            S3_16_MIB_FLASH_LAYOUT,
        );
    }

    #[test]
    fn techo_profile_pages_bridge_the_journal_and_identity_vaults() {
        assert_eq!(
            T_ECHO_JOURNAL_LAYOUT.arenas[1].len() as usize,
            T_ECHO_MIN_ARENA_BYTES
        );
        assert_eq!(T_ECHO_RADIO_PROFILE_PAGES, [0xE9000, 0xEA000]);
        assert_eq!(
            T_ECHO_RADIO_PROFILE_PAGES[1] + HOPSPOT_FLASH_PAGE_BYTES as u32,
            0xEB000
        );
        assert_eq!(0xEB000 + HOPSPOT_FLASH_PAGE_BYTES as u32, 0xEC000);
        assert_eq!(0xEC000 + HOPSPOT_FLASH_PAGE_BYTES as u32, 0xED000);
    }
}
