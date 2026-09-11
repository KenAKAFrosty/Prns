# Embedded assurance without hardware

Embedded assurance combines production firmware resource evidence with executable proofs that do not require a connected board. Start every assurance session with the read-only readiness check:

```console
./tools/prns doctor embedded-assurance
```

The doctor checks the current requirements from the assurance inventories: upstream and ESP Rust toolchains, compilation targets, LLVM tools, the pinned Miri nightly, QEMU runners, Renode and its nRF52840 model, Xtensa compiler tools, and libclang. It installs nothing. When something is missing or has the wrong version, it prints the exact setup commands or the pinned emulator package, checksum, and source revision. `PRNS_RENODE` may point to a Renode executable outside `PATH`; set `PRNS_RENODE_ROOT` only when its bundled platform models are not discoverable beside that executable.

The authoritative versions and targets remain in `validation/hardening/embedded-isa.toml`, `validation/hardening/embedded-platform.toml`, `validation/manifest.toml`, and `tools/release/release-esp-toolchain-identity.sh`. The doctor consumes those files rather than maintaining another compatibility table.

## Run the quick evidence

Check generated memory contracts first, then run the affected executable proofs:

```console
./tools/prns build embedded resources contracts --check
python3 validation/run.py run --suite embedded-miri-quick
python3 validation/run.py run --suite embedded-isa-thumbv7em
python3 validation/run.py run --suite embedded-isa-riscv32imac
python3 validation/run.py run --suite embedded-isa-xtensa-esp32s3
python3 validation/run.py run --suite embedded-platform-nrf52840
python3 validation/run.py run --suite embedded-platform-esp32s3
```

Use `python` instead of `python3` on Windows. The Miri runner can provision its pinned nightly and components on first use; the doctor tells you whether that download has already happened. ISA runners require the exact QEMU identity declared by their architecture adapter. The Xtensa lane also uses the pinned ESP Rust and crosstool-NG toolchains; its Espressif QEMU package is selected and checksum-pinned for the contributor's host platform.

The nRF52840 pilot builds a small integration image with the production `t-echo-s140-v6` flash/RAM profile and startup conventions. Renode must load its pinned nRF52840 model, derive the vector table, initial stack pointer, and reset entry from that ELF, and reach the named application-entry milestone. This is an advisory platform-integration pilot. It does not load or prove the SoftDevice, board peripherals, radio, display, USB, Bluetooth, timing, power, or physical behavior.

The ESP32-S3 pilot builds in the production ESP workspace with the pinned ESP Rust toolchain, `linkall.x`, frame-pointer policy, ESP-IDF application descriptor, ESP HAL, ESP RTOS, and Embassy entrypoint. The shared firmware builder packages that ELF with the 8 MiB production partition table; Espressif QEMU boots the resulting flash image through its modeled ROM and reaches the named runtime-initialized milestone. The representative `heltec-wireless-stick-lite-v3` profile deliberately avoids a PSRAM claim. This pilot does not initialize or prove radios, Wi-Fi, Bluetooth, USB, displays, external storage, timing, power, or physical peripherals. ESP32-C6 platform emulation remains explicitly unsupported; its RISC-V target-ISA evidence is separate.

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

Resource reports and linker evidence live below `target/flash-artifacts/resources`. Miri, ISA, and platform-pilot proof fragments, transcripts, target ELFs, and emulator logs live below `validation-artifacts/results`. The combined JSON and Markdown matrix is written to the requested output directory.

These checks establish memory contracts, executable structure, measured stack evidence, production task-pool allocation, target-ABI sizes for named semantic-scenario futures, Rust memory-model behavior for exercised components, and matching component behavior as target instructions. The checks do not prove RF behavior, physical peripherals, timing, power, SoftDevice behavior, or whole-board operation.
