# 0.3.7 T096 and T1000-E Developer Flasher Qualification

Observed independently on 2026-08-21 with the local developer flasher based on
commit `f48cfd6a74cf9fd7c0082becc5dbe50c4ac4ccbf`:

```console
./tools/prns run device.hopspot.dev-flasher.serve -- t096 t1000-e --port 8765
```

The command built both selected qualification targets, assembled and
ephemerally signed one preview manifest, staged the pinned Nordic DFU browser
core, and served the candidate on `http://127.0.0.1:8765`. The resulting source
identity was
`0.3.6-dev.dirty.24469cc3cfbbeba65caeecb05b0f4d0ca48196dfe6e9bb2c815a9131d7967d0e`.
The pre-0.3.7 version and dirty marker are deliberate facts of this developer
receipt, not release-version or production-signing claims.

At build time, every firmware, catalog, flasher, and website path matched the
recorded commit. The dirty state came only from concurrent, excluded
observability work in `docs/observability.md`,
`prnsd/observability/grafana/prnsd.json`, and
`prnsd/src/observability/metrics/dimensions.rs`. The preview key ID
`0D9296818B217512` was ephemeral and is not a production trust root.

## Exact candidate artifacts

| Artifact | Purpose | Size | SHA-256 |
| --- | --- | ---: | --- |
| `t096-s140-6.1.1.uf2` | T096 recovery UF2 | 1,043,968 bytes | `0e1914e20334f88a33b665a47e2ee3a3343c0bb021b288ed2036ab8f2de309ec` |
| `t1000e.bin` | T1000-E serial-DFU application | 362,260 bytes | `ff9d54bf9ca0261d2664c99dc7778ebb5a4d79d46f53d2ce715c4a57503d4005` |
| `t1000e.dat` | T1000-E serial-DFU init packet | 14 bytes | `e1c27d1adf3639c1cb2d8d54e9c4fd03cf05227a61f2cbceb8d9e17a0567bf0c` |
| `t1000e.uf2` | T1000-E recovery fallback | 724,992 bytes | `19fb83e2ef0bbd9b80a5731d24d6497a17a3678abc6cf47101aafd54945c29d3` |
| `flash-manifest.json` | Preview release manifest | 4,413 bytes | `b2703290c46c540a4dcc153bcc2e62aeac32d7d611a97749cf60a42e1a6b6e45` |
| `flash-manifest.json.minisig` | Ephemeral preview signature | 348 bytes | `bcaaa92b6ae2d43bf17d5f955c33016fbf2621c84639f27f9dc4cdf2b75374ec` |

The T096 UF2 retained application base `0x00026000`, nRF52840 family
`0xADA52840`, 256-byte UF2 payloads, and 2,039 blocks. The T1000-E recovery UF2
retained application base `0x00027000`, the same family, and 1,416 blocks. The
serial delivery paired the exact application and init packet for S140 7.3.0,
FWID `0x0123`, device type `0x0052`, revision `52840`, single-bank layout, and
application range `0x00027000..0x000EA000`.

## Host and browser coverage

The host was an arm64 Mac running macOS 26.4 build 25E246. The installed
browsers exercised during the session were Chrome 151.0.7922.173, Firefox
153.0.3, and Safari 26.4. The initial local run also used the Codex in-app
browser; that surface identified itself as `Codex In-app Browser` but did not
expose an exact engine version to the inspection interface.

| Browser surface | Observed result |
| --- | --- |
| Chromium-capable local browser | T096 recovery UF2 completed end to end. T1000-E Personal Hopspot entry completed WebUSB bootloader entry, exact serial reconnection, and Nordic serial DFU end to end. |
| Chrome 151.0.7922.173 | Web Serial and WebUSB capability paths were available, and the local candidate behaved as the complete direct-install browser route. |
| Firefox 153.0.3 | Desktop Web Serial exposed the exact stock/bootloader route. The missing WebUSB capability kept Personal Hopspot automatic entry disabled and presented Chrome/Edge, recovery-UF2, and CLI alternatives. |
| Safari 26.4 | T096 and recovery-UF2 browser operations remained available. T1000-E direct DFU failed closed because Safari exposed neither required device API, with truthful fallback guidance. |

Microsoft Edge was not installed on this host and was not independently
exercised. The Chrome/Edge recommendation describes the supported Chromium
capability route; it is not evidence of an Edge hardware run.

## T096 physical result

The selected `INFO_UF2.TXT` reported the exact recovery foundation already
recorded in the [T096 qualification receipt](t096-qualification.md): board ID
`HT-n5262G`, mount label `HT-n5262G`, and SoftDevice S140 6.1.1. The browser
parsed the descriptor locally, selected the one compatible signed variant,
verified its byte count, SHA-256, UF2 structure, family, base, and bounds, and
downloaded the file. Copying that exact UF2 to the recovery drive completed,
the drive disappeared, and the board rebooted into Personal Hopspot. This was
reported as an end-to-end physical pass.

The application identity remained USB `1209:0001`, manufacturer
`Stay Personal`, product `Personal Hopspot (Heltec T096)`, and serial
`PERSONAL-RNS-T096-HOP`. The similar T114 identity `HT-n5262` was not accepted
for this target.

## T1000-E physical and compatibility result

The default `Personal Hopspot is running` route selected only USB `1209:0001`
with manufacturer `Stay Personal`, product `Personal Hopspot (T1000-E)`, and
serial `PERSONAL-RNS-T1000E-HOP`. The exact device-level vendor request `0x50`
with value `0x5052` and index `0x4E53` entered the bootloader without a button
sequence. The browser then selected the exact Web Serial identity `2886:0057`,
validated the application and init packet in the Rust/WASM core, and completed
the acknowledged Nordic serial transfer. The tracker rebooted into Personal
Hopspot, producing an end-to-end physical pass.

The separate `Seeed/Meshtastic firmware or bootloader` selection was exercised
against the exact `2886:0057` bootloader path, including the bounded serial
permission continuation and completed DFU transfer. Firefox's lack of WebUSB
correctly prevented it from claiming that it could enter a running Personal
Hopspot application, while preserving this Web Serial path for a tracker that
is already in the supported stock/bootloader state.

The recovery artifact remained bound to mount label `T1000-E`, exact board ID
`nrf52840-t1000-e-v1`, family `0xADA52840`, and the same application bytes as
the DFU application. The page exposed it only after an explicit recovery
selection and valid local `INFO_UF2.TXT` parsing.

## Remaining limits and disposition

The T1000-E recovery UF2 was structurally and byte-for-byte validated but was
not physically copied during this session. Once Personal Hopspot was running,
repeated use of the stock single-button/cable sequence did not expose the
factory mass-storage bootloader; direct WebUSB-to-serial DFU was the successful
managed path. The separately tested serial route began from the exact
bootloader identity, so this receipt also does not independently prove the
1200-baud transition from a factory Seeed application. A stock-position unit
remains necessary to close both physical observations.

Safari's pass is a UF2 and fail-closed compatibility result, not direct Nordic
DFU support. The exact Codex in-app Chromium engine version, Edge, other
operating systems, device-loss injection on physical hardware, long-duration
soak behavior, remote management, and refined LED/button policy were not
qualified here. No nRF provisioning or full-chip erase was introduced or
exercised.

This is a local developer receipt, not signed release acceptance. The eventual
0.3.7 candidate must be rebuilt, reproduced, signed with the production trust
root, and qualified against its own immutable hashes. Both T096 and T1000-E
remain qualification targets; this evidence does not alter the shipping set.
