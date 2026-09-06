use personal_hopspot_memory::{AddressRange, HELTEC_V4, T_ECHO_S140_V6, XIAO_ESP32_C6};

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

#[test]
fn riscv_aliases_share_one_store_and_do_not_recharge_the_static_heap() {
    let origin = 0x4080_0000;
    let sections = [
        section(".trap", SectionKind::Code, origin, 0x100),
        section(".bss", SectionKind::ZeroFill, origin + 0x100, 100_000),
        section(
            ".stack",
            SectionKind::ZeroFill,
            origin + 0x100 + 100_000,
            20_000,
        ),
    ];
    let usage = analyze(&XIAO_ESP32_C6, &sections).expect("valid RISC-V evidence");
    assert_eq!(usage.len(), 1);
    assert_eq!(
        usage[0].address_spaces,
        [
            AddressSpaceId("instruction-ram"),
            AddressSpaceId("data-ram")
        ]
    );
    assert_eq!(usage[0].additional_reservation_bytes, 0);
    assert_eq!(usage[0].included_reservation_bytes, 88 * 1024);
    assert_eq!(
        usage[0].capacity,
        RamCapacity::Known {
            bytes: 120_256,
            headroom_bytes: 20_000,
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
