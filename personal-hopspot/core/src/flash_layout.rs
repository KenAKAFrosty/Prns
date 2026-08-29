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

pub const NRF52840_RADIO_PROFILE_PAGES: [u32; 2] = [0xE9000, 0xEA000];
pub const NRF52840_BLE_IDENTITY_FLASH_OFFSET: u32 = 0xE8000;
pub const NRF52840_NODE_IDENTITY_FLASH_OFFSET: u32 = 0xEB000;
pub const T114_RECOVERY_BOOTLOADER_FLASH_OFFSET: u32 = 0xEC000;
pub const T_ECHO_BLE_IDENTITY_FLASH_OFFSET: u32 = 0xEC000;
pub const T_ECHO_RESERVED_FLASH_END: u32 = 0xED000;
pub const MESH_TOWER_V2_RADIO_PROFILE_FLASH_OFFSET: u32 = 0xE9000;
pub const MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET: u32 = 0xEA000;
pub const MESH_TOWER_V2_RECOVERY_BOOTLOADER_FLASH_OFFSET: u32 = 0xEC000;
pub const T1000E_NODE_IDENTITY_FLASH_OFFSET: u32 = 0xF0000;
pub const T1000E_RECOVERY_BOOTLOADER_FLASH_OFFSET: u32 = 0xF4000;
pub const T096_APPLICATION_DATA_END: u32 = 0xEC000;
pub const T096_FACTORY_RESERVED_FLASH_OFFSET: u32 = 0xED000;
pub const T096_RECOVERY_BOOTLOADER_FLASH_OFFSET: u32 = 0xF4000;
pub const T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET: u32 = 0xBF000;
pub const HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET: u32 = 0xE1000;
pub const MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET: u32 = 0xE2000;
pub const T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET: u32 = 0xE9000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nrf52840FirmwareMemory {
    pub application_flash: FirmwareAddressRange,
    pub application_ram: FirmwareAddressRange,
    pub minimum_runtime_stack_bytes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareAddressRange {
    pub start: u32,
    pub end: u32,
}

impl FirmwareAddressRange {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        assert!(start < end);
        Self { start, end }
    }

    #[must_use]
    pub const fn byte_len(self) -> u32 {
        self.end - self.start
    }
}

const NRF52840_APPLICATION_FLASH_ORIGIN: u32 = 0x26000;
const NRF52840_S140_V7_APPLICATION_FLASH_ORIGIN: u32 = 0x27000;
const NRF52840_APPLICATION_RAM_ORIGIN: u32 = 0x2000E000;
const T1000E_APPLICATION_RAM_ORIGIN: u32 = 0x20010000;
const NRF52840_RAM_END: u32 = 0x20040000;
const NRF52840_MINIMUM_RUNTIME_STACK_BYTES: u32 = 68 * 1024;
pub const T_ECHO_MIN_ARENA_BYTES: usize = 19 * HOPSPOT_FLASH_PAGE_BYTES;
pub const T_ECHO_JOURNAL_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xC0000, 0xC1000],
    [
        FlashArenaRange::new(0xC2000, 0xD6000),
        FlashArenaRange::new(0xD6000, 0xE9000),
    ],
);
pub const T_ECHO_S140_V6_FIRMWARE_MEMORY: Nrf52840FirmwareMemory = Nrf52840FirmwareMemory {
    application_flash: FirmwareAddressRange::new(
        NRF52840_APPLICATION_FLASH_ORIGIN,
        T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    ),
    application_ram: FirmwareAddressRange::new(NRF52840_APPLICATION_RAM_ORIGIN, NRF52840_RAM_END),
    minimum_runtime_stack_bytes: NRF52840_MINIMUM_RUNTIME_STACK_BYTES,
};
pub const T_ECHO_S140_V7_FIRMWARE_MEMORY: Nrf52840FirmwareMemory = Nrf52840FirmwareMemory {
    application_flash: FirmwareAddressRange::new(
        NRF52840_S140_V7_APPLICATION_FLASH_ORIGIN,
        T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    ),
    application_ram: FirmwareAddressRange::new(NRF52840_APPLICATION_RAM_ORIGIN, NRF52840_RAM_END),
    minimum_runtime_stack_bytes: NRF52840_MINIMUM_RUNTIME_STACK_BYTES,
};

pub const NRF52840_MIN_ARENA_BYTES: usize = 2 * HOPSPOT_FLASH_PAGE_BYTES;
pub const HELTEC_DISPLAY_NRF52840_JOURNAL_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xE2000, 0xE3000],
    [
        FlashArenaRange::new(0xE4000, 0xE6000),
        FlashArenaRange::new(0xE6000, 0xE8000),
    ],
);
pub const HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY: Nrf52840FirmwareMemory =
    Nrf52840FirmwareMemory {
        application_flash: FirmwareAddressRange::new(
            NRF52840_APPLICATION_FLASH_ORIGIN,
            HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
        ),
        application_ram: FirmwareAddressRange::new(
            NRF52840_APPLICATION_RAM_ORIGIN,
            NRF52840_RAM_END,
        ),
        minimum_runtime_stack_bytes: NRF52840_MINIMUM_RUNTIME_STACK_BYTES,
    };
pub const MESH_TOWER_V2_JOURNAL_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xE3000, 0xE4000],
    [
        FlashArenaRange::new(0xE5000, 0xE7000),
        FlashArenaRange::new(0xE7000, 0xE9000),
    ],
);
pub const MESH_TOWER_V2_FIRMWARE_MEMORY: Nrf52840FirmwareMemory = Nrf52840FirmwareMemory {
    application_flash: FirmwareAddressRange::new(
        NRF52840_APPLICATION_FLASH_ORIGIN,
        MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    ),
    application_ram: FirmwareAddressRange::new(NRF52840_APPLICATION_RAM_ORIGIN, NRF52840_RAM_END),
    minimum_runtime_stack_bytes: NRF52840_MINIMUM_RUNTIME_STACK_BYTES,
};
pub const T1000E_JOURNAL_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xEA000, 0xEB000],
    [
        FlashArenaRange::new(0xEC000, 0xEE000),
        FlashArenaRange::new(0xEE000, 0xF0000),
    ],
);
pub const T1000E_FIRMWARE_MEMORY: Nrf52840FirmwareMemory = Nrf52840FirmwareMemory {
    application_flash: FirmwareAddressRange::new(
        NRF52840_S140_V7_APPLICATION_FLASH_ORIGIN,
        T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    ),
    application_ram: FirmwareAddressRange::new(T1000E_APPLICATION_RAM_ORIGIN, NRF52840_RAM_END),
    minimum_runtime_stack_bytes: NRF52840_MINIMUM_RUNTIME_STACK_BYTES,
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

    assert!(
        T_ECHO_S140_V6_FIRMWARE_MEMORY.application_flash.end
            == T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
    );
    assert!(
        T_ECHO_S140_V7_FIRMWARE_MEMORY.application_flash.end
            == T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
    );
    assert!(
        T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            == T_ECHO_JOURNAL_LAYOUT.timebase_regions[0]
    );
    assert!(T_ECHO_JOURNAL_LAYOUT.arenas[1].end == NRF52840_RADIO_PROFILE_PAGES[0]);
    assert!(
        HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY
            .application_flash
            .end
            == HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
    );
    assert!(
        HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            == HELTEC_DISPLAY_NRF52840_JOURNAL_LAYOUT.timebase_regions[0]
    );
    assert!(
        HELTEC_DISPLAY_NRF52840_JOURNAL_LAYOUT.arenas[1].end == NRF52840_BLE_IDENTITY_FLASH_OFFSET
    );
    assert!(
        MESH_TOWER_V2_FIRMWARE_MEMORY.application_flash.end
            == MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
    );
    assert!(
        MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            == MESH_TOWER_V2_JOURNAL_LAYOUT.timebase_regions[0]
    );
    assert!(MESH_TOWER_V2_JOURNAL_LAYOUT.arenas[1].end == NRF52840_RADIO_PROFILE_PAGES[0]);
    assert!(
        T1000E_FIRMWARE_MEMORY.application_flash.end == T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET
    );
    assert!(
        T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            == T1000E_JOURNAL_LAYOUT.timebase_regions[0]
    );
    assert!(T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET.is_multiple_of(PAGE));
    assert!(HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET.is_multiple_of(PAGE));
    assert!(MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET.is_multiple_of(PAGE));
    assert!(T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET.is_multiple_of(PAGE));
    assert!(
        T_ECHO_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE <= NRF52840_NODE_IDENTITY_FLASH_OFFSET
    );
    assert!(
        HELTEC_DISPLAY_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            <= NRF52840_NODE_IDENTITY_FLASH_OFFSET
    );
    assert!(
        MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE
            <= NRF52840_NODE_IDENTITY_FLASH_OFFSET
    );
    assert!(
        T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET + PAGE <= T1000E_NODE_IDENTITY_FLASH_OFFSET
    );
    assert!(NRF52840_BLE_IDENTITY_FLASH_OFFSET + PAGE == NRF52840_RADIO_PROFILE_PAGES[0]);
    assert!(NRF52840_RADIO_PROFILE_PAGES[0] + PAGE == NRF52840_RADIO_PROFILE_PAGES[1]);
    assert!(NRF52840_RADIO_PROFILE_PAGES[1] + PAGE == NRF52840_NODE_IDENTITY_FLASH_OFFSET);
    assert!(NRF52840_NODE_IDENTITY_FLASH_OFFSET + PAGE == T114_RECOVERY_BOOTLOADER_FLASH_OFFSET);
    assert!(NRF52840_NODE_IDENTITY_FLASH_OFFSET + PAGE == T_ECHO_BLE_IDENTITY_FLASH_OFFSET);
    assert!(T_ECHO_BLE_IDENTITY_FLASH_OFFSET + PAGE == T_ECHO_RESERVED_FLASH_END);
    assert!(MESH_TOWER_V2_JOURNAL_LAYOUT.arenas[1].end == MESH_TOWER_V2_RADIO_PROFILE_FLASH_OFFSET);
    assert!(
        MESH_TOWER_V2_RADIO_PROFILE_FLASH_OFFSET + PAGE == MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET
    );
    assert!(MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET + PAGE == NRF52840_NODE_IDENTITY_FLASH_OFFSET);
    assert!(
        NRF52840_NODE_IDENTITY_FLASH_OFFSET + PAGE
            == MESH_TOWER_V2_RECOVERY_BOOTLOADER_FLASH_OFFSET
    );
    assert!(T1000E_JOURNAL_LAYOUT.arenas[1].end == T1000E_NODE_IDENTITY_FLASH_OFFSET);
    assert!(
        T1000E_NODE_IDENTITY_FLASH_OFFSET + 4 * PAGE == T1000E_RECOVERY_BOOTLOADER_FLASH_OFFSET
    );
    assert!(NRF52840_NODE_IDENTITY_FLASH_OFFSET + PAGE == T096_APPLICATION_DATA_END);
    assert!(T096_APPLICATION_DATA_END + PAGE == T096_FACTORY_RESERVED_FLASH_OFFSET);
    assert!(T096_FACTORY_RESERVED_FLASH_OFFSET < T096_RECOVERY_BOOTLOADER_FLASH_OFFSET);
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

    #[test]
    fn nrf52840_profile_pages_bridge_the_techo_journal_and_identity_vaults() {
        assert_eq!(
            T_ECHO_JOURNAL_LAYOUT.arenas[1].len() as usize,
            T_ECHO_MIN_ARENA_BYTES
        );
        assert_eq!(NRF52840_RADIO_PROFILE_PAGES, [0xE9000, 0xEA000]);
        assert_eq!(
            NRF52840_RADIO_PROFILE_PAGES[1] + HOPSPOT_FLASH_PAGE_BYTES as u32,
            NRF52840_NODE_IDENTITY_FLASH_OFFSET
        );
        assert_eq!(
            NRF52840_NODE_IDENTITY_FLASH_OFFSET + HOPSPOT_FLASH_PAGE_BYTES as u32,
            T096_APPLICATION_DATA_END
        );
        assert_eq!(
            T096_APPLICATION_DATA_END + HOPSPOT_FLASH_PAGE_BYTES as u32,
            T096_FACTORY_RESERVED_FLASH_OFFSET
        );
    }

    #[test]
    fn nrf52840_firmware_ends_at_its_remote_control_identity_vault() {
        assert_eq!(
            T_ECHO_S140_V6_FIRMWARE_MEMORY,
            Nrf52840FirmwareMemory {
                application_flash: FirmwareAddressRange::new(0x26000, 0xBF000),
                application_ram: FirmwareAddressRange::new(0x2000E000, 0x20040000),
                minimum_runtime_stack_bytes: 68 * 1024,
            }
        );
        assert_eq!(
            T_ECHO_S140_V7_FIRMWARE_MEMORY,
            Nrf52840FirmwareMemory {
                application_flash: FirmwareAddressRange::new(0x27000, 0xBF000),
                application_ram: FirmwareAddressRange::new(0x2000E000, 0x20040000),
                minimum_runtime_stack_bytes: 68 * 1024,
            }
        );
        assert_eq!(
            HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY,
            Nrf52840FirmwareMemory {
                application_flash: FirmwareAddressRange::new(0x26000, 0xE1000),
                application_ram: FirmwareAddressRange::new(0x2000E000, 0x20040000),
                minimum_runtime_stack_bytes: 68 * 1024,
            }
        );
        assert_eq!(
            MESH_TOWER_V2_FIRMWARE_MEMORY,
            Nrf52840FirmwareMemory {
                application_flash: FirmwareAddressRange::new(0x26000, 0xE2000),
                application_ram: FirmwareAddressRange::new(0x2000E000, 0x20040000),
                minimum_runtime_stack_bytes: 68 * 1024,
            }
        );
        assert_eq!(
            T1000E_FIRMWARE_MEMORY,
            Nrf52840FirmwareMemory {
                application_flash: FirmwareAddressRange::new(0x27000, 0xE9000),
                application_ram: FirmwareAddressRange::new(0x20010000, 0x20040000),
                minimum_runtime_stack_bytes: 68 * 1024,
            }
        );
    }

    #[test]
    fn nrf52840_journals_are_contiguous() {
        for (layout, expected_start, expected_end, expected_arena_lengths) in [
            (
                T_ECHO_JOURNAL_LAYOUT,
                0xC0000,
                0xE9000,
                [20 * HOPSPOT_FLASH_PAGE_BYTES, T_ECHO_MIN_ARENA_BYTES],
            ),
            (
                HELTEC_DISPLAY_NRF52840_JOURNAL_LAYOUT,
                0xE2000,
                0xE8000,
                [NRF52840_MIN_ARENA_BYTES; 2],
            ),
            (
                MESH_TOWER_V2_JOURNAL_LAYOUT,
                0xE3000,
                0xE9000,
                [NRF52840_MIN_ARENA_BYTES; 2],
            ),
            (
                T1000E_JOURNAL_LAYOUT,
                0xEA000,
                0xF0000,
                [NRF52840_MIN_ARENA_BYTES; 2],
            ),
        ] {
            assert_eq!(layout.timebase_regions[0], expected_start);
            assert_eq!(
                layout.timebase_regions[0] + HOPSPOT_FLASH_PAGE_BYTES as u32,
                layout.timebase_regions[1]
            );
            assert_eq!(
                layout.timebase_regions[1] + HOPSPOT_FLASH_PAGE_BYTES as u32,
                layout.arenas[0].start
            );
            assert_eq!(layout.arenas[0].end, layout.arenas[1].start);
            assert_eq!(layout.arenas[1].end, expected_end);
            assert_eq!(
                [
                    layout.arenas[0].len() as usize,
                    layout.arenas[1].len() as usize,
                ],
                expected_arena_lengths
            );
        }
    }
}
