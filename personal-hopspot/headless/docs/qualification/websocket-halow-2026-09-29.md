# Browser → WebSocket → HaLoW qualification

Real Chrome browser nodes fetched the exact Hopspot page through the G4's plain
WebSocket listener, both locally and from each Heltec over native HaLoW. Eight
concurrent browser nodes also fetched remote pages successfully. This qualifies
the optional application listener and basic transport bridging, not an offline
web application installed on the G4.

The [structured evidence](websocket-halow-2026-09-29.json) records the dirty
candidate build, executable hash, browser version, individual results, memory
samples, capture hashes, and restoration checks. The hardware-tested candidate
SHA-256 is `9ed814ac3cbcf88d3248014a7687a676ebc45e2a4d50bca948d9f1950722b4c3`.
It was built on source base `936897505` with the WebSocket changes in progress.
Private captures and logs remain in `/private/tmp/halow-ws2-evidence` on the lab host.

## Conditions and results

Chrome 154.0.8037.58 ran isolated browser contexts with the current Prns WASM and
browser SDK, including its default dedicated-worker execution. Browser assets
were served from laptop loopback HTTP; the connection was plain
`ws://192.168.12.1:4344/`. No browser permission bypass flags were used. Each
context connected only to the G4; the two remote destinations were reached over
HaLoW. The G4, Heltec A, and Heltec B used open 802.11s, 924 MHz, 8 MHz, MCS2,
long GI, 18 dBm, power saving off, and mesh forwarding off. Vendor polling stayed
running. All three radios reported two established peers and healthy Morse status.

The first three concurrent requests returned 3542 bytes each: G4 local 51 ms,
Heltec A 73 ms, and Heltec B 147 ms. The subsequent eight concurrent requests
alternated between Heltecs, returning the same exact page with RTTs of 85–145 ms.
All response hashes matched the compiled page plus its MessagePack binary header:
`1c639c701bceeb3d09b1f95ab43a7535f54324a795bacb1b17cf3b543b6eff1c`.
These are application request RTTs, excluding initial discovery/link setup, and
are not sustained-throughput or field-range measurements.

The eight clients retained their sessions for fifteen seconds after receipt.
Two G4 socket-table samples showed eight established connections. G4 memory:

| Moment | RSS | Process high-water RSS |
| --- | ---: | ---: |
| Before eight-client run | 4148 KiB | 4224 KiB |
| Five-second sample | 4668 KiB | 4668 KiB |
| Ten-second sample | 4680 KiB | 4680 KiB |
| After clients closed | 4208 KiB | 4688 KiB |

The candidate executable was 3,709,772 bytes versus 3,514,192 bytes for the
preceding HaLoW-only candidate: +195,580 bytes, approximately 191 KiB. This is a
comparison of the recorded development candidates, not a general size guarantee.
The optional server does not require Rustls/ring TLS client dependencies.

After lint cleanup replaced an invariant semaphore `expect` with I/O error
propagation, the final static binary rebuilt at 3,710,604 bytes (about 192 KiB
above the HaLoW-only candidate), SHA-256
`e58e77bec27983cbdfac6d4480d2f6ecba88e58effcb191cbe6f8b672b85674d`.
Its local G4 browser page smoke passed at 75 ms and shut down gracefully without
changing the restored radios. The multi-radio/eight-client measurements above
belong to the earlier recorded candidate; they were not repeated with this final
error-path change.

Filtered radio captures decoded the native Prns envelopes and reported zero
kernel buffer drops. G4 addressed unicast frames to both Heltecs; each Heltec's
outgoing unicast frames addressed the G4. Capture boundaries differ and do not
prove losslessness or exact RF transmission counts.

## Bounds and regression checks

The headless option defaults to eight accepted sessions plus pending handshakes
combined. Existing frame/message limits and a ten-second handshake timeout apply.
A local integration test exercises an incomplete handshake occupying the only
slot, release on failure, an established session occupying it, release on close,
and a full page transfer through the newly admitted connection. Additional
connections wait in the kernel backlog; immediate HTTP rejection is not promised.

The full headless suite with `wifi-halow,websocket` passed on macOS, as did the
existing twelve WebSocket transport tests. Server-only and headless Clippy passed
with warnings denied. The Linux HaLoW validation entry now includes the combined
headless feature set; this does not claim those tests were executed on Linux in
this session. MIPS static cross-builds succeeded.

The first attempt used old staged WASM/SDK files and timed out during the request
after successful connection and link establishment. A fresh build passed. Building
the current playground also exposed missing RuntimeRejected close handlers and
an unstaged Casework browser dependency; those build issues are fixed alongside
this work. Regenerate a coherent browser package rather than combining stale
staged SDK/WASM files with current source.

## Reproduce

Build a board bundle with the qualified toolchain and
`./tools/prns run build.hopspot.g4 -- ... --with-websocket --with-probe`. Launch it
with explicit state, TCP and WebSocket bind addresses, plus the normal HaLoW
options. Preserve independent wired management, fresh backups, and timed rollback.

Stage current browser assets into a separate directory:

```sh
./tools/prns run build.wasm-docs.stage -- /tmp/hopspot-browser-assets
```

With the documentation site's Playwright dependency installed and a browser
available, run the smoke from the repository. `EXPECTED_PAGE_FILE` is the
`hopspot_index_no_source.mu` generated in the board build's `personal-hopspot-core`
OUT_DIR; using that exact build output verifies the complete response.

```sh
PRNS_BROWSER_CHANNEL=chrome node personal-hopspot/headless/scripts/websocket-browser-smoke.mjs \
  /tmp/hopspot-browser-assets "$EXPECTED_PAGE_FILE" \
  ws://192.168.12.1:4344/ "$G4_DESTINATION" "$HELTEC_A_DESTINATION" "$HELTEC_B_DESTINATION"
```

Omit the channel variable to use Playwright's installed Chromium. Repeat
remote destination arguments to create concurrent clients. Optional
`PRNS_BROWSER_HOLD_MS=15000` retains each successful session briefly for memory
sampling. The script serves only the supplied asset directory on loopback,
uses separate browser contexts, and validates every returned page hash and length.

## Recovery and remaining work

All temporary hosts stopped gracefully. Saved wireless/mesh configuration and
driver parameters matched byte-for-byte after restoration, UCI had no pending
radio changes, and all three radio health checks passed. The pre-existing G4 TCP
Hopspot still served the exact page with a 48 ms request RTT. No persistent
installation or firmware replacement was performed.

The listener is public Reticulum transport with no administrative API; it accepts
browser origins and supplies neither TLS nor HTTP assets. Hosted HTTPS access to
plain WebSockets, local-network permissions in other browsers, offline asset
hosting on the G4, AP/client discovery, sustained load, and installation/update
recovery remain separate qualification work. These results also do not establish
a forced two-radio-hop RF chain.
