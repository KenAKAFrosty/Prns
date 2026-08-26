# Heltec Vision Master E290-HF qualification

This receipt tracks the qualification target implemented on
`heltec-e290-support` from `origin/display_support` at `f04c5d5e`. The candidate
is currently an uncommitted working-tree implementation. It is not a shipping
or signed-release target.

## Supported hardware contract

| Authority | Qualification record |
|---|---|
| Board design | Heltec Vision Master E290-HF V0.3.1 |
| MCU and memory | ESP32-S3R8, 16 MiB flash, at least 8 MiB mapped octal PSRAM |
| Radio | Fitted HT-RA62-HF/SX1262 assembly; LF assembly unsupported |
| Display | DEPG0290BNS800F6 V2.1, SSD1680Z8, 296 x 128 monochrome |
| Input | Active-low GPIO21 key using the board external pull-up |
| Board A identifier | Operator-identified E290-HF; ESP USB serial `AC:A7:04:E1:3F:88`, current port `/dev/cu.usbmodem101`, USB location `0-1.4.2` |
| Board B identifier | Operator-identified E290-HF; ESP USB serial `AC:A7:04:E1:49:A4`, current port `/dev/cu.usbmodem1101`, USB location `0-1.4.4.1` |

The implementation uses the shared Personal Hopspot application, 16 MiB flash
layout, HSPCFG1 provisioning, exact two-frame retained presentation state, and
ordinary S3 LoRa/BLE/Wi-Fi/TCP/ESP-NOW/USB composition. It does not include
product management, appliance storage, RMAP, an LXMF mailbox, OTA policy,
battery telemetry, GNSS, QuickLink expansion, partial waveforms, or public
release promotion.

## Software evidence

The implementation is based on `f04c5d5e4246fd5ce2227a578b2508c0f71cd350`
with uncommitted changes. It must be committed and rebuilt before acceptance so
the final receipt and artifact are bound to one immutable source object. The
developer artifacts therefore record source capability as `absent`, rather
than claiming an immutable release candidate.

Powered testing rejected the first local candidate
`0.3.6-dev.dirty.cd5a293ae1195dc43451e297b51c83793b9fcd8d06258eb03151b74d13c3ee5c`:
both panels rendered the complete face horizontally mirrored. The board-local
packing now reflects increasing SSD1680 gate addresses before applying the
front-facing clockwise transform. Its asymmetric-corner fixture records the
powered controller order. The corrected candidate is
`0.3.6-dev.dirty.d0012511bd61ca8221db13c14542310c279628a19ff9b5e5b7377f040a36d60f`.
The operator confirmed on both boards that it is readable normally, upright,
centered, and unclipped.

Subsequent two-board LoRa triage found a generic SX1262 transmit-completion
hole: the driver could consume a stale DIO1 level and accept any non-timeout IRQ
as successful transmission without requiring `TxDone`. The shared driver now
requires DIO1 to release before `SetTx`, requires the completion IRQ to contain
`TxDone`, and hard-reinitializes after an unexpected transmit IRQ. Its focused
14-test LoRa suite covers stale-high, missing, non-releasing, and wrong IRQ
outcomes. Exact release builds pass for E290, Heltec V4, Heltec V4-R8, and
T-Beam Supreme. The powered candidate below subsequently passed isolated
bidirectional LoRa announces between the two E290-HF boards.

The same powered session also rejected an unnecessary five-second post-waveform
display delay. It had been copied from an unrelated appliance message-burst
coalescing interval, not a panel requirement. E290 now serializes complete
waveforms with `OperationCompletionOnly`, allowing changed button and menu
frames to begin as soon as the preceding waveform completes while retaining the
30-second routine-telemetry minimum. The fully exercised powered candidate is
`0.3.6-dev.dirty.e0445fe0780d400a86e8797c98886704ec84c92aacaadbbaf5940c39d02d89be`.

A follow-on diagnostic revision with worktree source identity
`0.3.6-dev.dirty.1bcaed1be18add4e7dc80dd91b82db5d3ee8ab22def3edc45dae3de99b43ac8c`
adds board-local monotonic begin/completion traces for physical E290 waveforms
and strengthens the focused host fixture for exact unchanged-frame suppression.
Both boards received its device-verified sparse build. On board `3F:88`, three
successful full waveforms took 1.617-1.631 seconds; subsequent telemetry
waveforms began 30.005 and 30.004 seconds after the preceding completion. This
proves the powered telemetry floor. The measurement build embedded the ordinary
repository version `0.3.6`, not the source identity above, so it is not claimed
as a source-bound acceptance artifact. Its local trace is
`target/private-e290-proofs/1bcaed1b/display-refresh-timing.txt`, SHA-256
`0ad5b9ad5dbfa528525a5fb0baf5b01b72e832da042c5f9d8fca6c24a346c632`.

Provisioning both boards with a station network and direct IPv4 TCP target
exposed a shared S3 socket-capacity defect. The original capacity included
DHCP, Wi-Fi Auto, configured TCP, and service-discovery application sockets but
omitted the DNS socket that embassy-net installs internally whenever its `dns`
feature is enabled. Both boards associated successfully and then rejected the
first configured TCP socket with `adding a socket to a full SocketSet`. The
station stack now reserves the internal DNS slot explicitly. The corrected
candidate associated, obtained DHCP, and connected on both boards; survived
listener loss and restart; and reconnected after physical power loss and two
subsequent readback resets without a panic. Exact post-fix E290, Heltec V4,
Heltec V4-R8, and T-Beam Supreme builds pass.

This tracked receipt participates in the dirty source digest. The exact
post-receipt developer successor, its artifact hashes, both device-verified
sparse flashes, first-render timings, and final USB Auto handshakes are
therefore recorded in the ignored local evidence root at
`target/private-e290-proofs/<source-prefix>/source-bound-candidate.txt`. A later
accepted commit and rebuild provide the immutable source identity required for
release custody without making this receipt self-referential.

The local software gates passed on 2026-08-25:

- repository and ESP32 formatting, documentation formatting, diff hygiene,
  tooling-registry verification, validation-registry verification, and all 24
  first-party lockfile registrations;
- the locked 2,060-test Rust workspace inventory, workspace clippy with warnings
  denied, 37 ESP32 host tests, 88 validation-runner tests with one Windows-only
  skip, and 26 developer-flasher tests;
- catalog and flasher coverage including 56 catalog tests, nine candidate
  validator tests, 93 flasher tests, and two flasher JSON tests;
- default and E290-enabled local-development website tests, 43 tests in each
  configuration;
- the complete pinned dependency, unsafe, JavaScript-production, and
  third-party-notice audit;
- exact Xtensa release builds for `hopspot-heltec-e290`, `hopspot-heltec-v4`,
  `hopspot-heltec-v4-r8`, and `hopspot-t-beam-supreme`; and
- explicit fully exercised developer artifact construction for `heltec-e290`, producing
  1,792,608 bytes across the three sparse flash parts.

Independent non-writing doctor preflights exercised schema 3 against both
recorded ports. Each detected ESP32-S3 with 16 MiB flash and returned Heltec V4
R8 plus E290 as the deterministic same-identity candidates. ROM detection
cannot distinguish those pinouts, so the operator's physical E290 selection is
recorded separately from the detected USB identity. Exact PCB, panel, and radio
module markings were not transcribed during this session.

The ESP build used `rustc 1.95.0-nightly (95e5bda86 2026-04-15)` from the
`esp` toolchain and `xtensa-esp-elf-gcc 15.2.0` from
`esp-15.2.0_20250920`. The fully exercised powered sparse target contained:

| Artifact path below the fully exercised developer version | Bytes | SHA-256 |
|---|---:|---|
| `bootloader.bin` | 21,056 | `8a516bf82000501f129eb8bf7cd04ec6a33edb09487890beefe90989d806990d` |
| `partition-table.bin` | 3,072 | `e187b5a94e4423b42a5d41a02fd39ce1d89dd65c6c2241c14e9ec9786247a9a4` |
| `application.bin` | 1,768,480 | `a58594fdc728d6e49f4831953b20595c50dd6a14526d965de1e556e0f718ffd1` |
| `target.json` | 1,790 | `8468cc7a2012d49476f550454f456192aa78f6366431c0b9ce1ca956a25e9285` |

Host fixtures cover the 20-pixel margins, asymmetric logical corners,
SSD1680 controller order, full-waveform-only planning, and the
operation-completion-only user-input / 30-second telemetry spacing. An optional
cross-target clippy invocation also reached the complete E290 graph, then
stopped on the two existing shared-display `too_many_arguments` lints; the
supported exact board build and the registered host/workspace lint gates pass.
The manifest-driven pull-request selection for this macOS host now has passing
evidence for all 51 applicable portable and Apple suites, including the full
stock-RNS interoperability set, root tests and clippy, both Apple platform
lanes, registry/hygiene checks, oracles, and WASM/JavaScript checks. Its first
integration-capstone run exposed one stale example pattern that did not ignore
the newer announce application-data field; the example now accepts the complete
event non-exhaustively and the registered capstone suite passes. Linux-only
embedded and native lanes are not represented by that host selection; the exact
Xtensa builds above remain their E290-specific evidence.

Powered interoperability with the ordinary PRNS host runtime now passes for
route discovery and a standard proof probe over the isolated two-board LoRa
path. The pinned stock Python RNS 1.4.2 and 1.5.0 authorities subsequently
decoded the E290 name announce, discovered the same route, validated delivery
proofs, established a two-hop Link, and received the board's 2,045-byte
Quickstart page byte-for-byte as a Resource response. The first 1.5.0 run also
exposed a generic shared-instance omission: PRNS did not answer the authority's
new read-only `medium_path_timeout` RPC. That generic RNS compatibility work is
outside E290 support and is not included in this branch pending an upstream
resolution. The powered 1.5.0 run completed with the temporary compatibility
patch applied, so its path, Link, and Resource results are E290 transport
evidence rather than a claim that this branch implements that RPC. A
deliberately synchronized run then returned valid proofs for all 15 probes with
0% loss
while the operator continuously navigated through visible e-paper refreshes.
A later isolated run reset the remote board between proofs 2 and 3 of one
continuous 20-probe command; all 20 proofs returned, and a captured repeat
reset showed the same identity, persisted route state, display startup, and
LoRa route returning. A separate stock-RNS Link check then reproduced the same
86-byte Link request, packet hash, and Link ID after a captured reset and
reached ACTIVE again, documenting the runtime-scoped duplicate boundary.

## Powered hardware qualification

Powered checks began on 2026-08-25 on the two operator-identified boards above.
Both ports independently passed the non-writing doctor check. Each tested
candidate was written as three
sparse regions at `0x0`, `0x8000`, and `0x10000` with explicit ESP32-S3,
16 MiB, DIO, 40 MHz, USB-reset, and watchdog-reset settings. Espflash 4.5.0
completed its default device-side verification on both boards without a
full-chip erase.

Each qualifying warm boot reported its full developer version and source digest,
`HOPSPOT_HELTEC_E290`, ESP32-S3 revision 0.2, 16 MiB DIO/40 MHz flash, mapped
8 MiB octal PSRAM split into 4 MiB private and 4 MiB global arenas, a successful
PSRAM probe, SSD1680 readiness, completed first render, watchdog readiness,
ESP-NOW startup, and a clean persistence restore. No panic, reset loop, BUSY
timeout, display failure, or radio initialization error was observed.

| Required powered check | Status | Evidence and remaining limit |
|---|---|---|
| Physical identity and memory | Partial | The operator identified both devices as E290 boards; USB serials, ports, ESP32-S3, 16 MiB flash, and 8 MiB mapped PSRAM are recorded. Exact PCB, panel, and fitted-module markings remain to be transcribed. |
| Build, doctor, sparse flash, verify, reset, and banner | Passed | Both ports passed doctor, repeated sparse flash, device verification, watchdog reset, and source-bound `HOPSPOT_HELTEC_E290` banners. The exact post-receipt smoke candidate is recorded under the ignored evidence root described above. The canonical developer-flasher HSPCFG1 output was written to each hopcfg sector, read back after power loss, and matched byte-for-byte; browser delivery itself was not exercised. |
| Identity and configuration persistence | Passed | The two boards retained distinct node and BLE identities through warm resets, repeat sparse flashes, and a physical disconnect/reconnect. Board `AC:A7:04:E1:3F:88` has public node identity hash `26ef9bee409714ad1125ca0fa7e5dd98` and BLE identity `8c09e8ad0ed0927620077f8d8b3c4a4c`; board `AC:A7:04:E1:49:A4` has public node identity hash `44689452a9d44ecd1c00bd6c0d7824c6` and BLE identity `c6444aac4147e1abe7160a6d1eaf25ac`. All 12 KiB covering `hopcfg`, `node_id`, and `phy_init` remained byte-for-byte unchanged across repeat sparse flashes. The final bidirectional announce trace still carries node hashes `26ef9bee409714ad1125ca0fa7e5dd98` and `44689452a9d44ecd1c00bd6c0d7824c6`, independently confirming persistence after the last flash. Both radio-profile sectors were byte-for-byte identical after reconnect: US915, 915.000 MHz, SF9, 250 kHz, CR 4/5, 22 dBm, and preamble 18. |
| E-paper presentation | Partial | The first candidate was mirrored on both boards and was rejected. The corrected orientation is operator-confirmed readable normally, upright, centered, black-on-white, and unclipped on both boards. Normal full refresh, controller deep sleep, and rail-off image retention passed during the observation interval. The accepted implementation removes the unrelated five-second post-waveform dwell, so changed input frames may start immediately after operation completion while telemetry retains its 30-second minimum. The operator confirmed on both boards that display timing feels better after this correction. Follow-on monotonic instrumentation measured successful 1.617-1.631-second full waveforms and proved two completion-to-begin telemetry spacings of 30.005 and 30.004 seconds. The E290 host fixture also proves exact unchanged frames return `Unchanged`, but a powered quiet-interface interval is still required to observe the corresponding absence of a physical operation. Prolonged retention and a system sleep cycle also remain to run. The capability model exposes no Display Off action. |
| GPIO21 input | Partial | The operator confirmed that short and long GPIO21 presses navigate successfully and used the fully exercised powered candidate to disable two interfaces and issue both announces. Changed input frames no longer wait through a separate five-second post-waveform floor, and the operator confirmed that interactive display timing feels better. The operator also deliberately pressed during an active full refresh and confirmed that the navigation event was preserved and applied afterward. During a separately synchronized run, all 15 two-hop LoRa probes returned valid proofs with 0% loss while the operator continuously navigated through visible e-paper refreshes. Both boards remained live and emitted no reset, timeout, or display/radio fault while diagnostics were attached. Measured debounce bounds remain to run. GPIO0 remains reserved for ROM boot. |
| BUSY bounds and recovery | Partial | Hardware reset, software reset, RAM write, full refresh, and deep sleep completed within their bounds during every observed render. A simulated stuck-BUSY or disconnected-panel recovery was not run. |
| HF LoRa transmit and receive | Passed | With identical persisted US915 profiles and fitted HF antennas, the operator disabled BLE and ESP-NOW on both boards. Two independent USB Auto observers did not bridge Data frames. Board `49:A4` announced destination `b1df8767144c1f373969c13fbfebfe4d`, which board `3F:88` forwarded one second later; board `3F:88` announced `8dd55e5e84803ac6ee19cb5d52cdce16`, which board `49:A4` forwarded one second later. Each board's LoRa card then displayed one known peer. The content-addressed local trace is `target/private-e290-proofs/e0445fe0/lora-bidirectional-usb-observer.txt`, SHA-256 `826d2629c6f3da4ba6b0f9984f0467af73f7b8b051ffd8f3a038a1849ba05983`. |
| ESP-NOW transmit and receive | Passed | With LoRa and BLE disabled on both boards, unprovisioned Wi-Fi/TCP, and independent USB Auto observers that did not bridge Data frames, the operator issued the two signed node announces about ten seconds apart. Board `49:A4` announced destination `b1df8767144c1f373969c13fbfebfe4d`, which board `3F:88` transported one second later; board `3F:88` announced `8dd55e5e84803ac6ee19cb5d52cdce16`, which board `49:A4` transported one second later. Each transported form repeated five seconds afterward. ESP-NOW was the only possible inter-board path. The powered test used the `1bcaed1b` diagnostic build whose ESP-NOW composition matches the source-bound successor; its content-addressed local trace is `target/private-e290-proofs/1bcaed1b/espnow-bidirectional-usb-observer.txt`, SHA-256 `d1dce1cfdca5ccc69a31d129a07a2f5c306d7a97dfc0739e16ccba4052d6d70a`. |
| PRNS host route and proof interop | Passed | The host daemon attached by USB only to board `3F:88`; a passive exclusive holder prevented any daemon access to board `49:A4`, and BLE plus ESP-NOW remained disabled on both boards. After discarding its cached route, the ordinary host runtime discovered board `49:A4`'s LXMF destination `32a551818fb15a0d9bdf8fe14ed567e9` two hops away via board `3F:88`, then sent a 16-byte standard probe and received a valid proof reply in 1.419 seconds with 0% packet loss. USB counters increased in both directions. The only available inter-board path was LoRa. The content-addressed local trace is `target/private-e290-proofs/e0445fe0/path-and-proof-probe-interop.txt`, SHA-256 `3e21d94a771572d5a8e31edbe871d874a8814919bf3cc59ec9433d38a5311b36`. |
| Reboot during live LoRa traffic | Passed | The host daemon again attached only to board `3F:88` while an exclusive holder owned board `49:A4`'s USB port. A continuous 20-probe command reset board `49:A4` between proofs 2 and 3 using the ESP32-S3 USB Serial/JTAG hard-reset sequence; all 20 32-byte probes returned valid two-hop proofs with 0% loss. A captured repeat reset reported `USB_UART_CHIP_RESET`, the same E290 candidate, 16 MiB flash, 8 MiB PSRAM and readback, restored route state, and completed first display render. After discarding its route, the host rediscovered the same remote destination and identity two hops away. Its first three-probe sample retained one timeout at 2/3, followed by 10/10 with 0% loss. Board B's default BLE and ESP-NOW interfaces returned after reboot, but both remained disabled on board A, so LoRa was still the only possible inter-board path. A later Bluetooth-only macOS run, with desktop USB and Wi-Fi disabled, established a fresh native GATT session to the reset board's original BLE identity. The content-addressed local trace is `target/private-e290-proofs/e0445fe0/reboot-during-lora-probe-interop.txt`, SHA-256 `93c56df67b46317989c3eb6ac4ba7c9185a30e2f4f405659d9987c249910636e`. A pinned stock RNS 1.5.0 client also established a direct USB Link, reset board `49:A4`, regenerated the exact original 86-byte Link request from its retained ephemeral key material, asserted complete byte equality, and re-established the same packet hash and Link ID. This complements the registered within-runtime replay test: duplicate/link tables reset while the board identity and persisted routing state survive. Its trace is `target/private-e290-proofs/e0445fe0/duplicate-link-request-across-reboot.txt`, SHA-256 `ca2fc335c375e3fb34df1c39f6c0ab36e134951d810acfbc7964f7efee4bda10`. |
| Wi-Fi station and configured TCP Client | Passed | The canonical HSPCFG1 image provisioned both boards for the operator's station network and a direct IPv4 PRNS listener. After the shared socket-capacity correction, both boards associated on channel 11, received distinct DHCP leases, and held simultaneous TCP sessions. Stopping the listener produced bounded reconnect attempts without disrupting Wi-Fi, display, or other interfaces; both reconnected automatically after listener restart. A physical USB power cycle and two later flash readback resets each produced the same automatic association and reconnect, and both 4 KiB HSPCFG1 readbacks matched the provisioned image exactly. With fresh daemon counters, one manual announce from each board produced 1,005 received and 830 transmitted BackboneServer bytes and two direct one-hop paths over distinct TCP peers. Credential bytes and their image digest are excluded from evidence. The local trace is `target/private-e290-proofs/05d2884e/wifi-tcp-provisioning-recovery.txt`, SHA-256 `748dfe8604df25f3e2677ca6b68d17c612cc2af773257e2dbb4db0eac01b8cee`. |
| Interfaces and recovery | Partial | Final boots initialized LoRa without an error and started ESP-NOW, Bluetooth, provisioned Wi-Fi Auto, configured TCP, and USB diagnostics. An earlier candidate formed a native BLE Auto GATT link from board A to board B with DLE, 2M PHY, and ATT MTU 508. A diagnostic macOS Personal Hopspot later held distinct BLE sessions to both boards; each board's textual BLE interface menu reported `Peers 2`. The desktop reported three remote BLE members: the two E290 boards plus a nearby iOS Personal Hopspot, not itself. After board B was reset, a Bluetooth-only desktop run with USB and Wi-Fi disabled found it again, completed the native GATT handshake, and received its unchanged BLE identity `c6444aac4147e1abe7160a6d1eaf25ac`; the other observed session explicitly identified itself as iOS. The earlier desktop run's aggregate USB Auto interface simultaneously held both enumerated serial ports and received two independent liveness replies per interval. Isolated bidirectional HF LoRa and ESP-NOW announces, path/proof probes, stock-Python Links, a board-to-client Resource response, and bidirectional configured TCP traffic now pass. Deliberate fault injection beyond the recorded listener-loss, board-reset, and power-loss cases remains to run. |
| Python-authority interoperability | Passed | Through the loopback shared instance and board `3F:88`'s USB Auto interface, pinned stock RNS 1.4.2 and 1.5.0 each decoded board `49:A4`'s `Personal Hopspot E290` announce and identity, discovered its node and LXMF destinations two hops away, validated a delivery proof, established a Link, requested `/page/quickstart.mu`, and received its 2,045 bytes byte-for-byte as a Resource with SHA-256 `cda56535e0dfed19021671df3d002f907e39096ee8b99e5d7a2ddb588b27e3ef`. The 1.5.0 run used the temporary generic RPC compatibility patch described above; that patch is not part of this E290 branch. BLE and ESP-NOW were disabled and an exclusive passive holder prevented direct USB access to board `49:A4`; LoRa was the only inter-board path. A synchronized 15-probe run also returned valid proofs for every packet with 0% loss while the operator continuously navigated through visible e-paper refreshes; both boards' LoRa byte counters advanced consistently with the traffic. Afterward, one board's LoRa card showed one known identity and zero active Links while the other showed two known identities and three active Links; these are the interface-attributed routed-destination and endpoint-plus-transported-Link counts, and the operator confirmed them as correct for the test. The content-addressed main trace is `target/private-e290-proofs/e0445fe0/stock-rns-link-resource-interop.txt`, SHA-256 `e14094e4eb9f64e2d83839ff80759c33b1ff92ddce6cc59641a90e13e32ffd45`; the separate reboot traces cover live proof recovery and byte-identical duplicate behavior. |

The ignored evidence roots under `target/private-e290-proofs/` retain the tested
sparse artifacts and content-addressed traces by source-digest prefix. They are
local developer evidence, not signed release custody. Shipping and
public-flasher promotion require a later receipt tied to the exact accepted
commit and signed artifact digest, with every remaining powered check either
passed or explicitly accepted by maintainers.
