# nRF52840 LTO experiment

These reports preserve the phase-one LTO experiment reproduced from the source
state committed alongside them on 2026-09-07. All three builds used the same
toolchain fingerprint and Arm codegen policy recorded in the reports.

| Target | LTO | Result | Flash evidence | RAM headroom |
|---|---|---|---:|---:|
| T-Echo S140 v6 | fat | success | 621,976 B image; 4,712 B headroom | 4,816 B |
| T-Echo S140 v6 | thin | memory overflow | 86,560 B beyond `FLASH` | unavailable after failed link |
| MeshTower V2 | thin | success | 651,508 B image; 118,540 B headroom | 11,324 B |

The T-Echo comparison reports 91,272 additional analyzed flash bytes under
thin LTO. Of that increase, 25,887 bytes are attributed and 65,385 bytes are
unclassified. The report's ranked crate and symbol lists are advisory; the
linker overflow and region accounting are authoritative.

The MeshTower result is a build-time control showing that thin LTO can complete
a link on the same architecture and toolchain. It is not hardware-functionality
evidence.

Reproduce the evidence with:

```console
./tools/prns build embedded resources report --target t-echo-s140-v6 --lto fat
./tools/prns build embedded resources report --target t-echo-s140-v6 --lto thin --allow-overflow
./tools/prns build embedded resources report --target mesh-tower-v2 --lto thin
```

Render the ranked investigation list with:

```console
./tools/prns build embedded resources compare personal-hopspot/resources/experiments/lto/t-echo-s140-v6-fat.json personal-hopspot/resources/experiments/lto/t-echo-s140-v6-thin.json
```

The full linker maps remain build artifacts under
`target/flash-artifacts/resources`; they are intentionally not committed.
