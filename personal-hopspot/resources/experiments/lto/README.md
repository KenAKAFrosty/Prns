# nRF52840 LTO experiment

These reports preserve the phase-one LTO experiment reproduced from clean
`trunk` at `62b2c6e27` on 2026-09-07. All three builds used the same toolchain
fingerprint recorded in the reports.

| Target | LTO | Result | Flash evidence | RAM headroom |
|---|---|---|---:|---:|
| T-Echo S140 v6 | fat | success | 619,968 B image; 6,720 B headroom | 5,004 B |
| T-Echo S140 v6 | thin | memory overflow | 86,240 B beyond `FLASH` | unavailable after failed link |
| MeshTower V2 | thin | success | 652,844 B image; 117,204 B headroom | 11,452 B |

The T-Echo comparison reports 92,952 additional analyzed flash bytes under
thin LTO. Of that increase, 26,667 bytes are attributed and 66,285 bytes are
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
