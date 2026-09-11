# Embedded assurance without hardware

Embedded assurance combines production firmware resource evidence with executable proofs that do not require a connected board. Start every assurance session with the read-only readiness check:

```console
./tools/prns doctor embedded-assurance
```

The doctor checks the current requirements from the assurance inventories: upstream and ESP Rust toolchains, compilation targets, LLVM tools, the pinned Miri nightly, QEMU runners, Xtensa compiler tools, and libclang. It installs nothing. When something is missing or has the wrong version, it prints the exact Rust and ESP setup commands or the pinned emulator source identity.

The authoritative versions and targets remain in `validation/hardening/embedded-isa.toml`, `validation/manifest.toml`, and `tools/release/release-esp-toolchain-identity.sh`. The doctor consumes those files rather than maintaining another compatibility table.

## Run the quick evidence

Check generated memory contracts first, then run the affected executable proofs:

```console
./tools/prns build embedded resources contracts --check
python3 validation/run.py run --suite embedded-miri-quick
python3 validation/run.py run --suite embedded-isa-thumbv7em
python3 validation/run.py run --suite embedded-isa-riscv32imac
python3 validation/run.py run --suite embedded-isa-xtensa-esp32s3
```

Use `python` instead of `python3` on Windows. The Miri runner can provision its pinned nightly and components on first use; the doctor tells you whether that download has already happened. ISA runners require the exact QEMU identity declared by their architecture adapter. The Xtensa lane also uses the pinned ESP Rust and crosstool-NG toolchains; its Espressif QEMU package is selected and checksum-pinned for the contributor's host platform.

Run the full Miri borrow-model matrix before release-sensitive changes:

```console
python3 validation/run.py run --suite embedded-miri-full
```

## Build and combine the full evidence

The production resource matrix is heavier because it builds every current firmware target through the canonical builder:

```console
./tools/prns build embedded resources report --all
```

After resource and proof artifacts exist, combine them without rebuilding firmware:

```console
./tools/prns build embedded assurance summarize \
  --resources target/flash-artifacts/resources/configured \
  --proofs validation-artifacts/results \
  --output validation-artifacts/assurance
```

Resource reports and linker evidence live below `target/flash-artifacts/resources`. Miri and ISA proof fragments, transcripts, target ELFs, and emulator logs live below `validation-artifacts/results`. The combined JSON and Markdown matrix is written to the requested output directory.

These checks establish memory contracts, executable structure, measured stack evidence, production task-pool allocation, target-ABI sizes for named semantic-scenario futures, Rust memory-model behavior for exercised components, and matching component behavior as target instructions. The checks do not prove RF behavior, physical peripherals, timing, power, SoftDevice behavior, or whole-board operation.
