use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};

pub const HOPSPOT_FLASH_PAGE_BYTES: usize = 4096;

/// The durable regions coupled to one ESP32-S3 partition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HopspotS3FlashLayout {
    pub flash_capacity: usize,
    pub remote_control_identity_flash_offset: u32,
    pub radio_profile_pages: [u32; 2],
    pub journal: FlashJournalLayout,
}

pub const ESP32_4_MIB_FLASH_CAPACITY: usize = 4 * 1024 * 1024;
pub const ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET: u32 = 0x3DF000;

pub const S3_8_MIB_FLASH_LAYOUT: HopspotS3FlashLayout = HopspotS3FlashLayout {
    flash_capacity: 8 * 1024 * 1024,
    remote_control_identity_flash_offset: 0x67D000,
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
    remote_control_identity_flash_offset: 0xE7D000,
    radio_profile_pages: [0xE7E000, 0xE7F000],
    journal: FlashJournalLayout::new(
        [0xE80000, 0xE81000],
        [
            FlashArenaRange::new(0xE82000, 0xF41000),
            FlashArenaRange::new(0xF41000, 0x1000000),
        ],
    ),
};

const _: () = {
    const PAGE: u32 = HOPSPOT_FLASH_PAGE_BYTES as u32;
    assert!(
        S3_8_MIB_FLASH_LAYOUT.remote_control_identity_flash_offset + PAGE
            == S3_8_MIB_FLASH_LAYOUT.radio_profile_pages[0]
    );
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

    assert!(
        S3_16_MIB_FLASH_LAYOUT.remote_control_identity_flash_offset + PAGE
            == S3_16_MIB_FLASH_LAYOUT.radio_profile_pages[0]
    );
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
        assert_eq!(
            factory_offset + factory_size,
            layout.remote_control_identity_flash_offset
        );

        let remote_control_identity = partition(csv, "remote_ctl_id");
        assert_eq!(
            remote_control_identity,
            (
                layout.remote_control_identity_flash_offset,
                HOPSPOT_FLASH_PAGE_BYTES as u32,
            )
        );

        let (profile_offset, profile_size) = partition(csv, "radio_cfg");
        assert_eq!(profile_offset, layout.radio_profile_pages[0]);
        assert_eq!(profile_size, 2 * HOPSPOT_FLASH_PAGE_BYTES as u32);

        let (journal_offset, journal_size) = partition(csv, "prns_state");
        assert_eq!(journal_offset, layout.journal.timebase_regions[0]);
        assert_eq!(journal_offset + journal_size, layout.flash_capacity as u32);
    }

    #[test]
    fn esp_partition_tables_match_the_firmware_layout_contract() {
        let four_mib = include_str!("../../embedded/esp32/partitions-hopspot-4mb.csv");
        let (factory_offset, factory_size) = partition(four_mib, "factory");
        assert_eq!(factory_offset, 0x10000);
        assert_eq!(
            factory_offset + factory_size,
            ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
        );
        assert_eq!(
            partition(four_mib, "remote_ctl_id"),
            (
                ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
                HOPSPOT_FLASH_PAGE_BYTES as u32,
            )
        );
        let journal = partition(four_mib, "prns_state");
        assert_eq!(
            ESP32_4_MIB_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + HOPSPOT_FLASH_PAGE_BYTES as u32,
            journal.0
        );
        assert_eq!(journal.0 + journal.1, ESP32_4_MIB_FLASH_CAPACITY as u32);

        assert_s3_csv(
            include_str!("../../embedded/esp32/partitions-hopspot-8mb.csv"),
            S3_8_MIB_FLASH_LAYOUT,
        );
        assert_s3_csv(
            include_str!("../../embedded/esp32/partitions-hopspot-16mb.csv"),
            S3_16_MIB_FLASH_LAYOUT,
        );
    }
}
