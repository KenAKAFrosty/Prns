use super::*;
use std::vec::Vec;

use crate::profiles::ALL_MEMORY_PROFILES;
use crate::{
    AddressSpace, AddressSpaceKind, BackingStoreId, FirmwarePlacement, ProcessorArchitecture,
    TransportCompatibility, TransportEnvelope,
};

#[test]
fn every_memory_profile_has_one_valid_linker_profile() {
    assert_eq!(ALL_LINKER_ADDRESS_PROFILES.len(), ALL_MEMORY_PROFILES.len());
    for memory in ALL_MEMORY_PROFILES {
        let matching_count = ALL_LINKER_ADDRESS_PROFILES
            .iter()
            .filter(|profile| profile.memory_profile == memory.id)
            .count();
        assert_eq!(matching_count, 1, "{}", memory.id.as_str());
        let linker = linker_address_profile(memory.id).expect("linker profile");
        assert_eq!(linker.validate(memory), Ok(()));
    }
}

#[test]
fn derived_ranges_follow_memory_ownership_and_geometry() {
    let profile = linker_address_profile(T1000_E.id).expect("linker profile");
    let flash = profile.address_space(FLASH).expect("flash linker space");
    let ram = profile.address_space(RAM).expect("RAM linker space");
    assert_eq!(
        flash
            .ranges(&T1000_E)
            .expect("flash ranges")
            .collect::<Vec<_>>(),
        [T1000_E
            .region(T1000_E.firmware.firmware_owned_region)
            .expect("firmware region")
            .range]
    );
    assert_eq!(
        ram.ranges(&T1000_E)
            .expect("RAM ranges")
            .collect::<Vec<_>>(),
        [AddressRange::new(0x2001_0000, 0x2004_0000)]
    );
}

#[test]
fn overlapping_ranges_require_an_explicit_backing_alias() {
    const FIRST: AddressSpaceId = AddressSpaceId("first");
    const SECOND: AddressSpaceId = AddressSpaceId("second");
    const WINDOW: [AddressRange; 1] = [AddressRange::new(0x4000_0000, 0x4001_0000)];
    const LINKER_SPACES: [LinkerAddressSpace; 2] = [
        LinkerAddressSpace::explicit(FIRST, &WINDOW),
        LinkerAddressSpace::explicit(SECOND, &WINDOW),
    ];
    const ALIASED_MEMORY_SPACES: [AddressSpace; 2] = [
        AddressSpace {
            id: FIRST,
            kind: AddressSpaceKind::InstructionRam,
            geometry: AddressSpaceGeometry::LinkerDefined,
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
        AddressSpace {
            id: SECOND,
            kind: AddressSpaceKind::DataRam,
            geometry: AddressSpaceGeometry::LinkerDefined,
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
    ];
    const CONFLICTING_MEMORY_SPACES: [AddressSpace; 2] = [
        ALIASED_MEMORY_SPACES[0],
        AddressSpace {
            id: SECOND,
            kind: AddressSpaceKind::DataRam,
            geometry: AddressSpaceGeometry::LinkerDefined,
            backing_store: BackingStoreId("other-sram"),
            backing_offset: 0,
        },
    ];
    let aliased = fixture_memory(&ALIASED_MEMORY_SPACES);
    let conflicting = fixture_memory(&CONFLICTING_MEMORY_SPACES);
    let linker = profile(aliased.id, &LINKER_SPACES);
    assert_eq!(linker.validate(&aliased), Ok(()));
    assert_eq!(
        linker.validate(&conflicting),
        Err(LinkerAddressValidationError::ConflictingRanges {
            first: FIRST,
            second: SECOND,
        })
    );
}

#[test]
fn malformed_linker_profiles_report_typed_causes() {
    const FIRST: AddressSpaceId = AddressSpaceId("first");
    const UNKNOWN: AddressSpaceId = AddressSpaceId("unknown");
    const WINDOW: [AddressRange; 1] = [AddressRange::new(0x4000_0000, 0x4001_0000)];
    const OVERLAP: [AddressRange; 2] = [WINDOW[0], WINDOW[0]];
    const MEMORY_SPACES: [AddressSpace; 1] = [AddressSpace {
        id: FIRST,
        kind: AddressSpaceKind::InstructionRam,
        geometry: AddressSpaceGeometry::LinkerDefined,
        backing_store: BackingStoreId("sram"),
        backing_offset: 0,
    }];
    const MISSING: [LinkerAddressSpace; 0] = [];
    const UNKNOWN_SPACE: [LinkerAddressSpace; 1] = [LinkerAddressSpace::explicit(UNKNOWN, &WINDOW)];
    const DUPLICATE: [LinkerAddressSpace; 2] = [
        LinkerAddressSpace::explicit(FIRST, &WINDOW),
        LinkerAddressSpace::unmapped(FIRST),
    ];
    const OVERLAPPING: [LinkerAddressSpace; 1] = [LinkerAddressSpace::explicit(FIRST, &OVERLAP)];
    const OTHER_PROFILE: [LinkerAddressSpace; 1] = [LinkerAddressSpace::explicit(FIRST, &WINDOW)];
    let memory = fixture_memory(&MEMORY_SPACES);

    assert_eq!(
        profile(memory.id, &MISSING).validate(&memory),
        Err(LinkerAddressValidationError::MissingAddressSpace {
            address_space: FIRST,
        })
    );
    assert_eq!(
        profile(memory.id, &UNKNOWN_SPACE).validate(&memory),
        Err(LinkerAddressValidationError::UnknownAddressSpace {
            address_space: UNKNOWN,
        })
    );
    assert_eq!(
        profile(memory.id, &DUPLICATE).validate(&memory),
        Err(LinkerAddressValidationError::DuplicateAddressSpace {
            address_space: FIRST,
        })
    );
    assert_eq!(
        profile(memory.id, &OVERLAPPING).validate(&memory),
        Err(LinkerAddressValidationError::OverlappingRanges {
            address_space: FIRST,
        })
    );
    assert_eq!(
        profile(MemoryProfileId("other"), &OTHER_PROFILE).validate(&memory),
        Err(LinkerAddressValidationError::ProfileMismatch {
            expected: memory.id,
            actual: MemoryProfileId("other"),
        })
    );
}

fn fixture_memory(address_spaces: &'static [AddressSpace]) -> MemoryProfile {
    MemoryProfile {
        id: MemoryProfileId("fixture"),
        architecture: ProcessorArchitecture::RiscV32Imac,
        address_spaces,
        regions: &[],
        firmware: FirmwarePlacement {
            address_space: AddressSpaceId("firmware-space"),
            firmware_owned_region: MemoryRegionId("firmware"),
            transport_envelope: TransportEnvelope {
                range: AddressRange::new(1, 2),
                compatibility: TransportCompatibility::ExactFirmwareRegion,
            },
        },
        journals: &[],
        runtime_reservations: &[],
    }
}
