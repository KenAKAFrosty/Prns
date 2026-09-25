use super::*;
use crate::ReservationTotals;
use std::string::String;

fn assert_partition_csv(profile: &MemoryProfile, csv: &str) {
    let table = esp_partition_table(profile.id).expect("profile has a partition table");
    let mut generated = String::new();
    table
        .write_csv(profile, &mut generated)
        .expect("canonical partition table renders");
    assert_eq!(csv, generated);
}

#[test]
fn partition_tables_bind_to_generic_regions() {
    for profile in [
        &HELTEC_V4,
        &HELTEC_V4_R8,
        &HELTEC_V4_R8_AB,
        &HELTEC_E290,
        &HELTEC_WIRELESS_STICK_LITE_V3,
        &T_BEAM_SUPREME,
        &XIAO_ESP32_C6,
    ] {
        let table = esp_partition_table(profile.id);
        assert!(table.is_some());
        if let Some(table) = table {
            assert_eq!(table.validate(profile), Ok(()));
        }
    }
}

#[test]
fn checked_partition_csvs_match_the_canonical_profiles() {
    let sixteen_mib = include_str!("../../../../embedded/esp32/partitions-hopspot-16mb.csv");
    let eight_mib = include_str!("../../../../embedded/esp32/partitions-hopspot-8mb.csv");
    let four_mib = include_str!("../../../../embedded/esp32/partitions-hopspot-4mb.csv");
    let sixteen_mib_ab = include_str!("../../../../embedded/esp32/partitions-hopspot-16mb-ab.csv");

    for profile in [&HELTEC_V4, &HELTEC_V4_R8, &HELTEC_E290] {
        assert_partition_csv(profile, sixteen_mib);
    }
    assert_partition_csv(&T_BEAM_SUPREME, eight_mib);
    assert_partition_csv(&HELTEC_WIRELESS_STICK_LITE_V3, eight_mib);
    assert_partition_csv(&XIAO_ESP32_C6, four_mib);
    assert_partition_csv(&HELTEC_V4_R8_AB, sixteen_mib_ab);
}

#[test]
fn wireless_stick_lite_does_not_claim_external_psram() {
    assert!(HELTEC_WIRELESS_STICK_LITE_V3
        .address_spaces
        .iter()
        .all(|space| space.kind != crate::AddressSpaceKind::ExternalPsram));
}

#[test]
fn partition_table_lookup_rejects_non_espressif_profiles() {
    assert_eq!(
        esp_partition_table(crate::profiles::T_ECHO_S140_V6.id),
        None
    );
}

#[test]
fn linker_counted_and_additional_reservations_stay_separate() {
    assert_eq!(
        HELTEC_V4.reservation_totals(RECLAIMED_RAM),
        Ok(ReservationTotals {
            additional_bytes: 0,
            linker_counted_bytes: 72 * KIB,
            external_bytes: 0,
        })
    );
    assert_eq!(
        HELTEC_V4.reservation_totals(DCACHE_RAM),
        Ok(ReservationTotals {
            additional_bytes: 32 * KIB,
            linker_counted_bytes: 0,
            external_bytes: 0,
        })
    );
    assert_eq!(
        XIAO_ESP32_C6.reservation_totals(DRAM),
        Ok(ReservationTotals {
            additional_bytes: 0,
            linker_counted_bytes: 88 * KIB,
            external_bytes: 0,
        })
    );
}

#[test]
fn ab_profile_keeps_every_shipped_region_except_firmware() {
    for shipped in HELTEC_V4_R8.regions {
        if shipped.id == MemoryRegionId("firmware") {
            continue;
        }
        let migrated = HELTEC_V4_R8_AB
            .region(shipped.id)
            .unwrap_or_else(|| panic!("{} survives the A/B carve", shipped.id.0));
        assert_eq!(migrated, shipped, "{} moved", shipped.id.0);
    }
    assert_eq!(HELTEC_V4_R8_AB.journals, HELTEC_V4_R8.journals);
    assert_eq!(
        HELTEC_V4_R8_AB.runtime_reservations,
        HELTEC_V4_R8.runtime_reservations
    );
    assert_eq!(HELTEC_V4_R8_AB.address_spaces, HELTEC_V4_R8.address_spaces);
}

#[test]
fn ab_slots_are_equal_aligned_and_clear_of_every_preserved_region() {
    let slot_a = HELTEC_V4_R8_AB
        .unique_region_for_role(RegionRole::FirmwareImage)
        .expect("one firmware-owned slot");
    let slot_b = HELTEC_V4_R8_AB
        .unique_region_for_role(RegionRole::FirmwareUpdateSlot)
        .expect("one update slot");
    let boot_selection = HELTEC_V4_R8_AB
        .unique_region_for_role(RegionRole::BootSelection)
        .expect("one boot selection");

    assert_eq!(slot_a.range.byte_len(), slot_b.range.byte_len());
    assert_eq!(slot_a.range.byte_len(), 0x730000);
    assert!(slot_a.range.start().is_multiple_of(0x10000));
    assert!(slot_b.range.start().is_multiple_of(0x10000));
    assert_eq!(boot_selection.range.byte_len(), 0x2000);
    assert_eq!(
        HELTEC_V4_R8_AB.firmware.transport_envelope.range,
        slot_a.range
    );
    for persistent in HELTEC_V4_R8_AB
        .regions
        .iter()
        .filter(|region| region.retention == RegionRetention::PreserveAcrossFirmwareUpdate)
    {
        assert!(
            !persistent.range.overlaps(slot_a.range),
            "{}",
            persistent.id.0
        );
        assert!(
            !persistent.range.overlaps(slot_b.range),
            "{}",
            persistent.id.0
        );
    }
}

#[test]
fn shipped_16_mib_table_does_not_know_the_ab_profile() {
    assert!(!ESP_16_MIB_PARTITION_TABLE.supports(HELTEC_V4_R8_AB.id));
    assert!(!ESP_16_MIB_AB_PARTITION_TABLE.supports(HELTEC_V4_R8.id));
    assert_eq!(
        esp_partition_table(HELTEC_V4_R8_AB.id),
        Some(&ESP_16_MIB_AB_PARTITION_TABLE)
    );
    assert_eq!(
        esp_partition_table(HELTEC_V4_R8.id),
        Some(&ESP_16_MIB_PARTITION_TABLE)
    );
}
