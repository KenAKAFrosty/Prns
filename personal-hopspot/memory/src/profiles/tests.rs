use super::*;
use crate::{AddressSpaceGeometry, AddressSpaceKind};

#[test]
fn every_canonical_profile_is_unique_and_valid() {
    assert_eq!(ALL_MEMORY_PROFILES.len(), 21);
    for (index, profile) in ALL_MEMORY_PROFILES.iter().enumerate() {
        assert_eq!(profile.validate(), Ok(()), "{}", profile.id.0);
        assert_eq!(memory_profile(profile.id), Some(*profile));
        assert!(ALL_MEMORY_PROFILES[..index]
            .iter()
            .all(|prior| prior.id != profile.id));
    }
}

#[test]
fn profiles_resolve_by_their_stable_external_names() {
    for profile in ALL_MEMORY_PROFILES {
        assert_eq!(memory_profile_named(profile.id.as_str()), Some(profile));
    }
    assert_eq!(memory_profile_named("unknown-profile"), None);
}

#[test]
fn architecture_matrix_has_three_adapters_without_board_specific_architectures() {
    assert_eq!(
        HELTEC_V4.architecture.rust_target(),
        "xtensa-esp32s3-none-elf"
    );
    assert_eq!(
        XIAO_ESP32_C6.architecture.rust_target(),
        "riscv32imac-unknown-none-elf"
    );
    assert_eq!(
        T_ECHO_S140_V6.architecture.rust_target(),
        "thumbv7em-none-eabihf"
    );
    assert_eq!(T096.architecture, T114.architecture);
    assert_eq!(T114.architecture, MESH_POCKET_5000.architecture);
    assert_eq!(MESH_POCKET_5000.architecture, MESH_TOWER_V2.architecture);
    assert_eq!(MESH_TOWER_V2.architecture, MUZI_BASE_DUO.architecture);
    assert_eq!(MUZI_BASE_DUO.architecture, RAK4631.architecture);
    assert_eq!(RAK4631.architecture, WIO_TRACKER_L1.architecture);
}

#[test]
fn persistent_regions_are_disjoint_from_current_firmware() {
    for profile in ALL_MEMORY_PROFILES {
        let firmware = profile.region(FIRMWARE);
        assert!(firmware.is_some());
        if let Some(firmware) = firmware {
            for persistent in profile
                .regions
                .iter()
                .filter(|region| region.retention == RegionRetention::PreserveAcrossFirmwareUpdate)
            {
                if persistent.address_space == firmware.address_space {
                    assert!(
                        !persistent.range.overlaps(firmware.range),
                        "{} lets firmware overlap persistent region {}",
                        profile.id.0,
                        persistent.id.0,
                    );
                }
            }
        }
    }
}

#[test]
fn c6_preserves_radio_heap_total_and_reserves_cpu_stack_separately() {
    let heap_bytes: u64 = XIAO_ESP32_C6
        .runtime_reservations
        .iter()
        .filter(|reservation| {
            matches!(
                reservation.accounting,
                crate::ReservationAccounting::SharedPool { .. }
            )
        })
        .map(|reservation| reservation.bytes)
        .sum();
    assert_eq!(heap_bytes, 88 * 1024);
    let stack = XIAO_ESP32_C6
        .runtime_reservations
        .iter()
        .find(|reservation| reservation.id.0 == "minimum-runtime-stack")
        .unwrap();
    assert_eq!(stack.bytes, 48 * 1024);
    assert_eq!(
        stack.accounting,
        crate::ReservationAccounting::Dedicated {
            charge: crate::ReservationCharge::AdditionalToStatic
        }
    );
    let guard = XIAO_ESP32_C6
        .runtime_reservations
        .iter()
        .find(|reservation| reservation.id.0 == "runtime-stack-guard")
        .unwrap();
    assert_eq!(guard.bytes, 4 * 1024 + 16);
    assert_eq!(guard.accounting, stack.accounting);
    let reclaimed = XIAO_ESP32_C6
        .address_spaces
        .iter()
        .find(|space| space.kind == AddressSpaceKind::ReclaimedRam)
        .unwrap();
    assert_eq!(
        reclaimed.geometry,
        AddressSpaceGeometry::Fixed(AddressRange::new(0x4086_E610, 0x4087_E610))
    );
}
