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
            &["-flavor", "gnu", "--version"][..],
            &[
                "-C",
                "link-arg=--icf=all",
                "-C",
                "llvm-args=-enable-machine-outliner",
                "-C",
                "llvm-args=-machine-outliner-reruns=2",
            ][..],
            DisassemblerFlavor::LlvmObjdump,
            "llvm-objdump",
        ),
        (
            ProcessorArchitecture::RiscV32Imac,
            "riscv32imac-unknown-none-elf",
            LinkerFlavor::RustLld,
            "rust-lld",
            &["-flavor", "gnu", "--version"][..],
            &["-C", "link-arg=-Tlinkall.x"][..],
            DisassemblerFlavor::LlvmObjdump,
            "llvm-objdump",
        ),
        (
            ProcessorArchitecture::XtensaEsp32S3,
            "xtensa-esp32s3-none-elf",
            LinkerFlavor::GnuLd,
            "xtensa-esp32s3-elf-gcc",
            &["--version"][..],
            &["-C", "link-arg=-Tlinkall.x", "-C", "force-frame-pointers"][..],
            DisassemblerFlavor::GnuObjdump,
            "xtensa-esp32s3-elf-objdump",
        ),
    ];

    for (
        architecture,
        rust_target,
        linker_flavor,
        linker_program,
        version_arguments,
        rustflags,
        disassembler_flavor,
        disassembler_program,
    ) in expected
    {
        let adapter = adapter_for(architecture);
        assert_eq!(adapter.rust_target(), rust_target);
        assert_eq!(adapter.linker_flavor(), linker_flavor);
        assert_eq!(adapter.linker_program(), linker_program);
        assert_eq!(adapter.linker_version_arguments(), version_arguments);
        assert_eq!(adapter.rustflags(), rustflags);
        assert_eq!(adapter.disassembler_flavor(), disassembler_flavor);
        assert_eq!(adapter.disassembler_program(), disassembler_program);
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
fn thumb_codegen_policy_is_applied_to_cargo() {
    let adapter = adapter_for(ProcessorArchitecture::ThumbV7em);
    let mut command = Command::new("cargo");
    command.env(
        cargo_rustflags_environment(adapter.rust_target()),
        "inherited flags",
    );
    adapter.configure_rustflags(&mut command);

    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("RUSTFLAGS"))
            .and_then(|(_, value)| value),
        Some(OsStr::new(
            "-C link-arg=--icf=all -C llvm-args=-enable-machine-outliner -C llvm-args=-machine-outliner-reruns=2"
        ))
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| { *key == OsStr::new("CARGO_TARGET_THUMBV7EM_NONE_EABIHF_RUSTFLAGS") })
            .and_then(|(_, value)| value),
        None
    );
}

#[test]
fn esp_cargo_aliases_mirror_adapter_rustflags() {
    let config = include_str!("../../../embedded/esp32/.cargo/config.toml");
    assert!(config.contains(
        "rustflags = [\"-C\", \"link-arg=-Tlinkall.x\", \"-C\", \"force-frame-pointers\"]"
    ));
    assert!(config.contains("rustflags = [\"-C\", \"link-arg=-Tlinkall.x\"]"));
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
