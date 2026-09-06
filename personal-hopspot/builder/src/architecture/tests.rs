use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use personal_hopspot_memory::ProcessorArchitecture;

use super::*;

#[test]
fn registry_has_one_stable_adapter_per_architecture() {
    let ids = ADAPTERS
        .iter()
        .map(|adapter| adapter.id().as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), ADAPTERS.len());
    assert_eq!(
        ids,
        BTreeSet::from([
            "riscv32imac-rust-lld",
            "thumbv7em-rust-lld",
            "xtensa-esp32s3-gnu-ld",
        ])
    );
}

#[test]
fn adapters_define_target_and_linker_identity() {
    let expected = [
        (
            ProcessorArchitecture::ThumbV7em,
            "thumbv7em-none-eabihf",
            LinkerFlavor::RustLld,
            "rust-lld",
        ),
        (
            ProcessorArchitecture::RiscV32Imac,
            "riscv32imac-unknown-none-elf",
            LinkerFlavor::RustLld,
            "rust-lld",
        ),
        (
            ProcessorArchitecture::XtensaEsp32S3,
            "xtensa-esp32s3-none-elf",
            LinkerFlavor::GnuLd,
            "xtensa-esp32s3-elf-gcc",
        ),
    ];

    for (architecture, rust_target, linker_flavor, linker_program) in expected {
        let adapter = adapter_for(architecture);
        assert_eq!(adapter.rust_target(), rust_target);
        assert_eq!(adapter.linker_flavor(), linker_flavor);
        assert_eq!(adapter.linker_program(), linker_program);
        assert_eq!(
            adapter_for_rust_target(rust_target).ok().map(Adapter::id),
            Some(adapter.id())
        );
    }
    assert!(adapter_for_rust_target("unknown-none-elf").is_err());
}

#[test]
fn linker_environment_is_derived_from_the_rust_target() {
    let mut command = Command::new("cargo");
    command.env(
        cargo_linker_environment("riscv32imac-unknown-none-elf"),
        "/tools/rust-lld",
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("CARGO_TARGET_RISCV32IMAC_UNKNOWN_NONE_ELF_LINKER"))
            .and_then(|(_, value)| value),
        Some(OsStr::new("/tools/rust-lld"))
    );
}

#[test]
fn adapters_encode_their_linker_map_dialects() {
    let path = Path::new("/artifacts/linker.map");
    assert_eq!(
        adapter_for(ProcessorArchitecture::ThumbV7em).linker_map_argument(path),
        OsStr::new("link-arg=-Map=/artifacts/linker.map")
    );
    assert_eq!(
        adapter_for(ProcessorArchitecture::RiscV32Imac).linker_map_argument(path),
        OsStr::new("link-arg=-Map=/artifacts/linker.map")
    );
    assert_eq!(
        adapter_for(ProcessorArchitecture::XtensaEsp32S3).linker_map_argument(path),
        OsStr::new("link-arg=-Wl,-Map=/artifacts/linker.map")
    );
}
