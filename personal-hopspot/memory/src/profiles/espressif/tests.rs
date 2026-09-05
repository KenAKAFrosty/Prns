use super::*;
use crate::ReservationTotals;

fn csv_partition(csv: &str, name: &str) -> (u64, u64) {
    csv.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split(',').map(str::trim);
            let partition_name = fields.next()?;
            let _partition_type = fields.next()?;
            let _partition_subtype = fields.next()?;
            let offset = fields.next()?;
            let size = fields.next()?;
            (partition_name == name).then(|| {
                (
                    u64::from_str_radix(offset.trim_start_matches("0x"), 16)
                        .expect("partition offset is hexadecimal"),
                    u64::from_str_radix(size.trim_start_matches("0x"), 16)
                        .expect("partition size is hexadecimal"),
                )
            })
        })
        .expect("named partition exists")
}

fn assert_partition_csv(profile: &MemoryProfile, csv: &str) {
    let table = esp_partition_table(profile.id).expect("profile has a partition table");
    assert_eq!(
        csv.lines()
            .filter(|line| !line.trim().is_empty())
            .filter(|line| !line.trim_start().starts_with('#'))
            .count(),
        table.partitions.len()
    );
    for partition in table.partitions {
        let region = profile
            .region(partition.region)
            .expect("partition region exists");
        assert_eq!(
            csv_partition(csv, partition.name),
            (region.range.start(), region.range.byte_len())
        );
    }
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
}
