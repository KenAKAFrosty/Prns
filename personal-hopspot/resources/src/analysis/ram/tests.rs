use personal_hopspot_memory::{
    AddressRange, HELTEC_V4, HELTEC_WIRELESS_STICK_LITE_V3, T_ECHO_S140_V6, XIAO_ESP32_C6,
};

use super::*;
use crate::analysis::SectionKind;

#[test]
fn fixed_arm_ram_charges_the_declared_stack_after_static_sections() {
    let sections = [section(
        ".data",
        SectionKind::InitializedData,
        0x2000_C000,
        0x1000,
    )];
    let usage = analyze(&T_ECHO_S140_V6, &sections).expect("valid Arm evidence");
    assert_eq!(
        usage,
        vec![RamBackingUsage {
            backing_store: BackingStoreId("internal-sram"),
            address_spaces: vec![AddressSpaceId("internal-ram")],
            capacity: RamCapacity::Known {
                bytes: 0x34000,
                headroom_bytes: 0x22000,
            },
            static_section_bytes: 0x1000,
            linker_padding_bytes: 0,
            additional_reservation_bytes: 68 * 1024,
            included_reservation_bytes: 0,
            external_reservation_bytes: 0,
        }]
    );
}

fn c6_sections(stack_bytes: u64) -> [AllocatedSection; 4] {
    let origin = 0x4080_0000;
    [
        section(".trap", SectionKind::Code, origin, 0x100),
        section(".bss", SectionKind::ZeroFill, origin + 0x100, 100_000),
        section(
            ".stack",
            SectionKind::ZeroFill,
            origin + 0x100 + 100_000,
            stack_bytes,
        ),
        section(
            ".dram2_uninit",
            SectionKind::ZeroFill,
            0x4086_E610,
            64 * 1024,
        ),
    ]
}

#[test]
fn riscv_aliases_and_reclaimed_heap_are_charged_once_with_a_stack_minimum() {
    let sections = c6_sections(60_000);
    let usage = analyze(&XIAO_ESP32_C6, &sections).expect("valid RISC-V evidence");
    assert_eq!(usage.len(), 2);
    let internal = backing(&usage, "internal-sram");
    assert_eq!(
        internal.address_spaces,
        [
            AddressSpaceId("instruction-ram"),
            AddressSpaceId("data-ram")
        ]
    );
    assert_eq!(internal.additional_reservation_bytes, 52 * 1024 + 16);
    assert_eq!(internal.included_reservation_bytes, 24 * 1024);
    assert_eq!(
        internal.capacity,
        RamCapacity::Known {
            bytes: 160_256,
            headroom_bytes: 60_000 - (52 * 1024 + 16)
        }
    );
    let reclaimed = backing(&usage, "reclaimed-sram");
    assert_eq!(reclaimed.static_section_bytes, 64 * 1024);
    assert_eq!(reclaimed.included_reservation_bytes, 64 * 1024);
    assert_eq!(reclaimed.additional_reservation_bytes, 0);
    assert_eq!(
        reclaimed.capacity,
        RamCapacity::Known {
            bytes: 64 * 1024,
            headroom_bytes: 0
        }
    );
    assert_eq!(
        internal.included_reservation_bytes + reclaimed.included_reservation_bytes,
        88 * 1024
    );
}

#[test]
fn riscv_link_success_with_insufficient_cpu_stack_is_rejected() {
    assert!(matches!(
        analyze(&XIAO_ESP32_C6, &c6_sections(52 * 1024 + 15)),
        Err(RamAnalysisError::RamOverflow { .. })
    ));
    assert!(matches!(
        analyze(&XIAO_ESP32_C6, &c6_sections(48 * 1024)),
        Err(RamAnalysisError::RamOverflow { .. })
    ));
    assert!(analyze(&XIAO_ESP32_C6, &c6_sections(52 * 1024 + 16)).is_ok());
}

#[test]
fn riscv_reclaimed_heap_requires_exact_nonoverlapping_bootloader_bank() {
    let mut sections = c6_sections(60_000);
    sections[3] = section(
        ".dram2_uninit",
        SectionKind::ZeroFill,
        0x4086_E610 - 4,
        64 * 1024,
    );
    assert!(matches!(
        analyze(&XIAO_ESP32_C6, &sections),
        Err(RamAnalysisError::InvalidArena { .. })
    ));
    assert!(matches!(
        analyze(&XIAO_ESP32_C6, &c6_sections(60_000)[..3]),
        Err(RamAnalysisError::IncludedReservationExceedsStatic {
            backing_store: BackingStoreId("reclaimed-sram"),
            ..
        })
    ));
}

#[test]
fn unused_riscv_reclaimed_ram_needs_no_synthetic_section() {
    let profile = MemoryProfile {
        runtime_reservations: &[],
        ..XIAO_ESP32_C6
    };
    let usage = analyze(&profile, &c6_sections(60_000)[..3])
        .expect("valid C6 evidence without reclaimed-RAM allocations");
    let reclaimed = backing(&usage, "reclaimed-sram");
    assert_eq!(reclaimed.static_section_bytes, 0);
    assert_eq!(reclaimed.included_reservation_bytes, 0);
    assert_eq!(
        reclaimed.capacity,
        RamCapacity::Known {
            bytes: 64 * 1024,
            headroom_bytes: 64 * 1024
        }
    );
}

#[test]
fn missing_linker_pool_evidence_is_rejected() {
    let sections = [section(".trap", SectionKind::Code, 0x4080_0000, 0x100)];
    assert!(matches!(
        analyze(&XIAO_ESP32_C6, &sections),
        Err(RamAnalysisError::MissingSection {
            section: ".stack",
            ..
        })
    ));
}

#[test]
fn static_ram_and_additional_reservations_cannot_exceed_capacity() {
    let sections = [section(".bss", SectionKind::ZeroFill, 0x2000_C000, 0x30000)];
    assert!(matches!(
        analyze(&T_ECHO_S140_V6, &sections),
        Err(RamAnalysisError::RamOverflow {
            backing_store: BackingStoreId("internal-sram"),
            ..
        })
    ));
}

#[test]
fn xtensa_alias_claims_and_distinct_ram_banks_are_accounted_once() {
    let data_origin = 0x3FC8_8000;
    let dummy_bytes = 0x1000;
    let bss_bytes = 130_000;
    let stack_bytes = 10_000;
    let reclaimed_origin = data_origin + dummy_bytes + bss_bytes + stack_bytes;
    let sections = [
        section(".rwtext", SectionKind::Code, 0x4037_8400, dummy_bytes),
        section(
            ".rwdata_dummy",
            SectionKind::ZeroFill,
            data_origin,
            dummy_bytes,
        ),
        section(
            ".bss",
            SectionKind::ZeroFill,
            data_origin + dummy_bytes,
            bss_bytes,
        ),
        section(
            ".stack",
            SectionKind::ZeroFill,
            data_origin + dummy_bytes + bss_bytes,
            stack_bytes,
        ),
        section(
            ".dram2_uninit",
            SectionKind::ZeroFill,
            reclaimed_origin,
            72 * 1024,
        ),
        section(
            ".rtc_fast.persistent",
            SectionKind::ZeroFill,
            0x600F_E000,
            8,
        ),
    ];
    let usage = analyze(&HELTEC_V4, &sections).expect("valid Xtensa evidence");
    assert_eq!(usage.len(), 6);
    let internal = backing(&usage, "internal-sram");
    assert_eq!(internal.static_section_bytes, dummy_bytes + bss_bytes);
    assert_eq!(internal.included_reservation_bytes, 124 * 1024);
    assert_eq!(
        internal.capacity,
        RamCapacity::Known {
            bytes: dummy_bytes + bss_bytes + stack_bytes,
            headroom_bytes: stack_bytes,
        }
    );
    assert_eq!(
        backing(&usage, "reclaimed-sram").capacity,
        RamCapacity::Known {
            bytes: 72 * 1024,
            headroom_bytes: 0,
        }
    );
    assert_eq!(
        backing(&usage, "dcache-sram").capacity,
        RamCapacity::Known {
            bytes: 32 * 1024,
            headroom_bytes: 0,
        }
    );
    assert_eq!(
        backing(&usage, "fast-retention-sram").capacity,
        RamCapacity::Known {
            bytes: 8 * 1024,
            headroom_bytes: 8 * 1024 - 8,
        }
    );
    assert_eq!(
        backing(&usage, "external-psram").capacity,
        RamCapacity::RuntimeDetected
    );
}

#[test]
fn unused_xtensa_reclaimed_ram_needs_no_synthetic_section() {
    let data_origin = 0x3FC8_8000;
    let dummy_bytes = 0x1000;
    let bss_bytes = 70_000;
    let stack_bytes = 10_000;
    let sections = [
        section(
            ".rwdata_dummy",
            SectionKind::ZeroFill,
            data_origin,
            dummy_bytes,
        ),
        section(
            ".bss",
            SectionKind::ZeroFill,
            data_origin + dummy_bytes,
            bss_bytes,
        ),
        section(
            ".stack",
            SectionKind::ZeroFill,
            data_origin + dummy_bytes + bss_bytes,
            stack_bytes,
        ),
    ];

    let usage = analyze(&HELTEC_WIRELESS_STICK_LITE_V3, &sections)
        .expect("valid S3FN8 evidence without reclaimed-RAM allocations");
    let reclaimed = backing(&usage, "reclaimed-sram");
    assert_eq!(reclaimed.static_section_bytes, 0);
    assert_eq!(reclaimed.included_reservation_bytes, 0);
}

fn section(name: &str, kind: SectionKind, start: u64, bytes: u64) -> AllocatedSection {
    AllocatedSection::from_parts(
        name.to_string(),
        kind,
        AddressRange::from_start_and_size(start, bytes).expect("fixture range"),
        if kind == SectionKind::ZeroFill {
            0
        } else {
            bytes
        },
        4,
    )
}

fn backing<'a>(usage: &'a [RamBackingUsage], id: &str) -> &'a RamBackingUsage {
    usage
        .iter()
        .find(|usage| usage.backing_store.0 == id)
        .expect("backing store")
}
