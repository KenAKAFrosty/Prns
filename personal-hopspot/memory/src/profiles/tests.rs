use super::*;

#[test]
fn every_canonical_profile_is_unique_and_valid() {
    assert_eq!(ALL_MEMORY_PROFILES.len(), 11);
    for (index, profile) in ALL_MEMORY_PROFILES.iter().enumerate() {
        assert_eq!(profile.validate(), Ok(()), "{}", profile.id.0);
        assert_eq!(memory_profile(profile.id), Some(*profile));
        assert!(ALL_MEMORY_PROFILES[..index]
            .iter()
            .all(|prior| prior.id != profile.id));
    }
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
    assert_eq!(T114.architecture, MESH_TOWER_V2.architecture);
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
