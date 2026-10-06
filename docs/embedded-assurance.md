# Embedded assurance without hardware

Embedded assurance combines production firmware resource evidence with executable proofs that do not require a connected board. Start every assurance session with the read-only readiness check:

```console
./tools/prns doctor embedded-assurance
```

The doctor checks the current requirements from the assurance inventories: upstream and ESP Rust toolchains, compilation targets, LLVM tools, the pinned Miri nightly, QEMU runners, Renode and its nRF52840 model, Xtensa compiler tools, and libclang. It installs nothing. When something is missing or has the wrong version, it prints the exact setup commands or the pinned emulator package, checksum, and source revision. `PRNS_RENODE` may point to a Renode executable outside `PATH`; set `PRNS_RENODE_ROOT` only when its bundled platform models are not discoverable beside that executable.

The nRF52840 production firmware compiler is pinned by
`personal-hopspot/embedded/nrf52840/rust-toolchain.toml`. This keeps release
flash sizes reproducible when the upstream stable compiler changes. The doctor
reads that same pin.

The other authoritative versions and targets remain in `validation/hardening/embedded-isa.toml`, `validation/hardening/embedded-platform.toml`, `validation/manifest.toml`, and `tools/release/release-esp-toolchain-identity.sh`. The doctor consumes those files rather than maintaining another compatibility table.

CI stages emulator builds and packages into an explicit disposable root through the validation control plane. The same path is available when reproducing a runner locally:

```console
python3 validation/run.py prepare-embedded-assurance \
  --root target/embedded-assurance-tools \
  --suite embedded-isa-thumbv7em
export PATH="$PWD/target/embedded-assurance-tools/bin:$PATH"
```

The preparation command derives its download, checksum, build, identity, and model requirements from the same inventories checked by the doctor. It refuses unknown suites, unsafe archive paths, checksum mismatches, unexpected tool identities, and broad installation roots. A host package format that cannot be prepared automatically remains an explicit doctor-guided setup rather than silently falling back to a different emulator.

## Run the release evidence

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

The nRF52840 pilot builds a small integration image with the production `t-echo-s140-v6` flash/RAM profile and startup conventions. Renode must load its pinned nRF52840 model, derive the vector table, initial stack pointer, and reset entry from that ELF, and reach the named application-entry milestone. This platform-integration pilot is a required automated release check. It does not load or prove the SoftDevice, board peripherals, radio, display, USB, Bluetooth, timing, power, or physical behavior.

The ESP32-S3 pilot builds in the production ESP workspace with the pinned ESP Rust toolchain, `linkall.x`, frame-pointer policy, ESP-IDF application descriptor, ESP HAL, ESP RTOS, and Embassy entrypoint. The shared firmware builder packages that ELF with the 8 MiB production partition table; Espressif QEMU boots the resulting flash image through its modeled ROM and reaches the named runtime-initialized milestone. The representative `heltec-wireless-stick-lite-v3` profile deliberately avoids a PSRAM claim. This pilot does not initialize or prove radios, Wi-Fi, Bluetooth, USB, displays, external storage, timing, power, or physical peripherals. ESP32-C6 platform emulation remains explicitly unsupported; its RISC-V target-ISA evidence is separate.

The pre-push hook and pull-request CI use the same changed-path classifier. They
print the exact resource, Miri, and target-ISA suites selected and the paths that
selected each suite. Pre-push runs those required suites without installing
missing tools; readiness failures point back to the embedded-assurance doctor.
Platform pilots remain scheduled/release work, so pre-push reports selected
pilots as deferred instead of silently omitting them.

The `embedded-miri-quick` suite is the focused Miri gate for pull requests and
releases. It exercises both radio drivers and an explicit persistence selection covering
encoding buffer limits, durable snapshots, recovery, isolated ownership, cancelled
commit confirmation, and rollback retry. Its target duration is five to ten minutes;
the validation runner enforces a 15-minute limit. Every selected filter must complete
with passing, unignored tests. A timeout or missing test fails the gate.

All native persistence tests, including every fault-injection boundary, remain required.
Focused Miri adds memory-model checking without interpreting the entire native fault
campaign for each release.

## Optional exhaustive Miri

Run the exhaustive borrow-model matrix for deeper investigation:

```console
python3 validation/run.py run --suite embedded-miri-full
```

The `embedded-miri-deep` workflow runs weekly and on manual dispatch. The monthly
`deep-validation` workflow also includes this suite. Exhaustive Miri is outside the
routine PR and release gate; reported failures still require investigation.

The registered full suite expands into eight round-robin shards. Each discovers the
complete component inventory and runs its assigned tests under both borrow models,
with two interpreter processes per model. CI runs each shard as a separate job with a
five-hour limit; the local command runs up to two shards concurrently, bounded by the
host's processor count. The longest power-loss campaigns divide their boundaries into
four disjoint test partitions, retaining every interruption point and assertion. Their
filters run first so each shard starts with the expensive campaigns.

Full evidence requires a clean commit and every discovered test executed exactly once
under each model. Individual shards save test logs and fingerprinted records, but do not
produce passing capability proofs. The collector checks the complete union, matching
source, toolchain, host and build flags, and each named test result before recording
full proofs. Ignored tests, missing shards and changed logs fail collection.

To retry a failed shard without repeating successful shards from the same clean commit:

```console
python3 validation/run.py run --suite embedded-miri-full-shard-0-of-8
python3 validation/run.py aggregate --tier scheduled --suite embedded-miri-full --expected-sha "$(git rev-parse HEAD)"
```

Use the failed shard's actual index. Per-test logs remain in each shard's artifact
directory while execution is in progress; assembled proofs live under
`validation-artifacts/results/embedded-miri-full`. Test identity fixtures reuse derived
keys, while each simulated node still owns fresh keys, engine state and authorization
tables.


## Build and combine the release evidence

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

Replace the reviewed baseline only from a complete passing matrix:

```console
./tools/prns build embedded assurance refresh-baseline \
  --matrix validation-artifacts/assurance/matrix.json
```

Refresh accepts passing focused Miri alongside the required firmware and emulator
evidence. Proofs explicitly record focused or exhaustive scope, borrow models, test
counts, source commit, tool identities, and artifact fingerprints. Exhaustive scope
still requires both models and the complete shard collection. A focused result is
never relabeled as exhaustive.

Refresh rejects missing required evidence, incompatible target or capability contracts,
and working-tree or mixed-commit evidence. This retains exact candidate provenance
while removing the requirement to rerun exhaustive Miri for baseline refreshes. The assurance crate also parses the checked-in baseline during its tests so schema or canonical-matrix drift cannot leave the snapshot silently stale.

These checks establish memory contracts, executable structure, measured stack evidence, production task-pool allocation, target-ABI sizes for named semantic-scenario futures, Rust memory-model behavior for exercised components, and matching component behavior as target instructions. The checks do not prove RF behavior, physical peripherals, timing, power, SoftDevice behavior, or whole-board operation.

From 0.3.8 until 1.0, these applicable emulator checks must pass alongside the
deterministic simulator and complete release-tier tests. Full physical board
qualification is optional for version bumps and promotion; the automated gate
does not claim the unmodeled physical behavior described above.
