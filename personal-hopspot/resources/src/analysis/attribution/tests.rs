use std::path::Path;

use super::*;

const RUST_LLD: &str = "     VMA      LMA     Size Align Out     In      Symbol\n   26000    26000       20     4 .text\n   26000    26000       18     4 /work/deps/app.o:(.text._RNvCsh_3app3run)\n   26018    26018        8     4 /work/deps/app.o:(.text.Reset)\n20000000 20000000       10     4 .bss\n20000000 20000000       10     4 /work/deps/app.o:(.bss._RNvCsh_3app5STATE)\n";

const GNU_LD: &str = "Linker script and memory map\n\n.rodata 0x3c000020 0x20\n .rodata._RNvCsh_3app4DATA\n                0x3c000020 0x18 /work/deps/app.o\n *fill*         0x3c000038 0x8\n\n.bss 0x3fc80000 0x10\n .bss._RNvCsh_3app5STATE 0x3fc80000 0x10 /work/deps/app.o\n";

#[test]
fn rust_lld_partial_maps_report_unclassified_bytes() -> Result<(), AttributionError> {
    let document = rust_lld::parse(Path::new("linker.map"), RUST_LLD)?;
    let analysis = document.attribute(Path::new("linker.map"), AttributionBasis::Partial)?;
    assert_eq!(
        analysis.symbol_coverage,
        AttributionCoverage {
            analyzed_bytes: 32,
            attributed_bytes: 24,
            unclassified_bytes: 8,
        }
    );
    assert_eq!(analysis.largest_crates[0].name, "app");
    assert_eq!(analysis.largest_crates[0].bytes, 24);
    Ok(())
}

#[test]
fn gnu_maps_join_split_contribution_records() -> Result<(), AttributionError> {
    let document = gnu_ld::parse(Path::new("linker.map"), GNU_LD)?;
    let analysis = document.attribute(Path::new("linker.map"), AttributionBasis::Partial)?;
    assert_eq!(
        analysis.crate_coverage,
        AttributionCoverage {
            analyzed_bytes: 32,
            attributed_bytes: 24,
            unclassified_bytes: 8,
        }
    );
    assert_eq!(analysis.largest_symbols.len(), 1);
    Ok(())
}

#[test]
fn malformed_and_overlapping_map_evidence_is_rejected() -> Result<(), AttributionError> {
    assert!(matches!(
        rust_lld::parse(Path::new("linker.map"), "not a map"),
        Err(AttributionError::Unrecognized { .. })
    ));
    let overlapping = "     VMA      LMA     Size Align Out     In      Symbol\n   26000    26000       20     4 .text\n   26000    26000       18     4 /work/deps/app.o:(.text._RNvCsh_3app3one)\n   26010    26010       10     4 /work/deps/app.o:(.text._RNvCsh_3app3two)\n";
    let document = rust_lld::parse(Path::new("linker.map"), overlapping)?;
    assert!(matches!(
        document.attribute(Path::new("linker.map"), AttributionBasis::Partial),
        Err(AttributionError::OverlappingContributions { .. })
    ));
    Ok(())
}

#[test]
fn symbol_names_remain_bounded_and_identifiable() {
    let symbol = format!("crate_name::{}::method", "nested".repeat(200));
    let compact = compact_symbol(&symbol);
    assert!(compact.len() < symbol.len());
    assert!(compact.starts_with("crate_name::"));
    assert!(compact.ends_with(']'));
    assert_eq!(compact, compact_symbol(&symbol));
    assert_eq!(crate_name(&symbol), Some("crate_name"));
}
