# nRF52840 LTO experiment

These reports preserve the production-resource experiment reproduced from the
clean commit `5bc902b9b96cc3e74ab4c517cba2bd00ec95f69f`.
The builds used the same pinned Rust 1.96.0 toolchain and Arm codegen policy
except for the reported LTO selection.

## Production resource evidence

| Target | LTO | Result | Flash | Static RAM headroom | Largest modeled direct-call chain | Task pools | Scenario futures |
|---|---|---|---:|---:|---:|---:|---:|
| T-Echo S140 v6 | fat | success | 621,976 B image; 4,712 B headroom | 4,816 B | 45,344 B; 24,288 B within reservation (advisory) | 56,440 B | SX126x 828 B; LR1110 928 B |
| T-Echo S140 v6 | thin | memory overflow | 86,560 B beyond `FLASH` | unavailable | unavailable | unavailable | unavailable |
| MeshTower V2 | configured (fat) | success | 563,184 B image; 206,864 B headroom | 11,332 B | 46,488 B; 23,144 B within reservation (advisory) | 49,528 B | SX126x 828 B; LR1110 928 B |
| MeshTower V2 | thin | success | 651,508 B image; 118,540 B headroom | 11,324 B | 33,664 B; 35,968 B within reservation (advisory) | 49,528 B | SX126x 828 B; LR1110 928 B |

The failed T-Echo link provides authoritative overflow and partial attribution
evidence, but no final ELF. Consequently its thin-LTO static RAM, stack,
task-pool, future, and machine-code measurements are unavailable rather than
estimated. The successful MeshTower control makes those comparisons possible:
thin LTO added 88,324 flash bytes and four static RAM bytes, left task-pool and
scenario-future sizes unchanged, and reduced the largest modeled direct-call
chain by 12,824 bytes. Its executable code grew from 484,324 to 508,908 bytes
and its discovered function boundaries grew from 3,412 to 5,708.

The T-Echo partial map contains 91,272 more analyzed flash bytes under thin LTO.
Only 25,887 bytes of that increase are attributed; 65,385 bytes remain
unclassified. The largest positive advisory crate deltas include `prns_core`
(45,178 B), `personal_hopspot_core` (14,266 B), `embassy_sync` (13,658 B), and
`core` (10,472 B). The linker overflow and region accounting are authoritative;
crate and symbol attribution are investigation leads.

## Target-ISA semantic evidence

The shared Arm assurance kernel was compiled and executed under QEMU 11.1.1's
`mps2-an386` Cortex-M4 model with both LTO modes. The resulting ELF files were
different, establishing that both codegen selections were exercised. Both runs
completed the two SX126x/LR1110 scenarios and matched the host reference byte
for byte.

| LTO | Arm ELF | ELF SHA-256 | Transcript |
|---|---:|---|---|
| fat | 145,552 B | `03537a780b6fd6a8db871caeb1e72b4c1d6822c76cc13e385e51e052d4547803` | 1,737 B; `c19243f10bc120a2ccfd880850451157a3d09ac75811347f5267f35e1e53e862` |
| thin | 155,104 B | `e846c5b1910ba4b1a1891ccc51c8126674689f0eaa67098e047cab23d1673577` | 1,737 B; `c19243f10bc120a2ccfd880850451157a3d09ac75811347f5267f35e1e53e862` |

This establishes that the exercised async polling, parsing, state transitions,
radio command generation, cancellation, and recovery behavior agrees under fat
and thin LTO when executed as Arm machine code. It does not execute the complete
T-Echo firmware and does not model its SoftDevice, MMIO, radio, display, USB,
interrupt timing, RF behavior, power behavior, or electrical environment.

## Conclusion

The hardware-less evidence does not reproduce a general fat-LTO semantic
miscompile in the shared radio/state-machine paths. It does reproduce a distinct
and deterministic production constraint: the T-Echo S140 v6 thin-LTO firmware
cannot link because it exceeds its owned flash region by 86,560 bytes, while the
canonical fat-LTO firmware fits by 4,712 bytes. MeshTower confirms that thin LTO
can link production firmware on the same compiler and processor architecture,
but grows this firmware family substantially; the assurance kernel separately
confirms target-instruction execution.

The current evidence therefore supports retaining fat LTO as the canonical
codegen policy. It neither proves nor disproves a fault limited to T-Echo
hardware integration. The historical functional report should remain an
unconfirmed hardware observation unless it can be reproduced with a precise
milestone and captured fault evidence. The fat build's partial stack analysis
also leaves unresolved indirect calls, interrupt nesting, foreign frames, and
one recursive cycle. Its 45,344-byte modeled direct-call chain is useful
pressure evidence, but path feasibility is not proven and it is not a stack
safety bound.

## Reproduction

Build and compare the production evidence:

```console
./tools/prns build embedded resources report --target t-echo-s140-v6 --lto fat
./tools/prns build embedded resources report --target t-echo-s140-v6 --lto thin --allow-overflow
./tools/prns build embedded resources report --target mesh-tower-v2 --lto configured
./tools/prns build embedded resources report --target mesh-tower-v2 --lto thin
./tools/prns build embedded resources compare \
  target/flash-artifacts/resources/fat/reports/t-echo-s140-v6.json \
  target/flash-artifacts/resources/thin/reports/t-echo-s140-v6.json
./tools/prns build embedded resources compare \
  target/flash-artifacts/resources/configured/reports/mesh-tower-v2.json \
  target/flash-artifacts/resources/thin/reports/mesh-tower-v2.json
```

Run the Arm semantic experiment into ignored artifact directories:

```console
CARGO_PROFILE_RELEASE_LTO=fat \
PRNS_VALIDATION_ARTIFACTS=target/validation-artifacts/experiments/lto/fat \
python3 validation/run.py run --suite embedded-isa-thumbv7em

CARGO_PROFILE_RELEASE_LTO=thin \
PRNS_VALIDATION_ARTIFACTS=target/validation-artifacts/experiments/lto/thin \
python3 validation/run.py run --suite embedded-isa-thumbv7em
```

Full linker maps, disassembly evidence, ELF files, transcripts, and emulator
logs remain regenerable build artifacts and are intentionally not committed.
