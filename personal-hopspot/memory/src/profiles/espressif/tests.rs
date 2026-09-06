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
        &HELTEC_E290,
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

    for profile in [&HELTEC_V4, &HELTEC_V4_R8, &HELTEC_E290] {
        assert_partition_csv(profile, sixteen_mib);
    }
    assert_partition_csv(&T_BEAM_SUPREME, eight_mib);
    assert_partition_csv(&XIAO_ESP32_C6, four_mib);
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
