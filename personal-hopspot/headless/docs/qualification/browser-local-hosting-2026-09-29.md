# G4-hosted browser qualification

The browser application ran from the G4's own plain LAN HTTP origin and retrieved
both the local Hopspot page and a remote Heltec page over HaLoW. It required no
laptop asset server, external CDN, or browser security override. The
[structured evidence](browser-local-hosting-2026-09-29.json) includes the asset
manifest identity, each public-file hash, browser results, runtime observations,
and the tested application hash.

## What was exercised

Chrome 154.0.8037.58 loaded the browser playground, current SDK, WASM engine, and
workers from `http://192.168.12.1:8082/`. The G4 used a separate vendor `uhttpd`
process with a private empty configuration and public-only document root in RAM.
LuCI and the original TCP Hopspot stayed separate. Both G4 and Heltec A ran
bounded native Hopspot processes; their existing vendor AP/station radio setup
was used unchanged. This was not another 802.11s or forced two-hop radio test.

The browser used `ws://192.168.12.1:4344/`. A local and remote request each returned
the exact 3542-byte packed Hopspot page, at 103 ms and 94 ms request RTT respectively.
Both SHA-256 hashes matched
`1c639c701bceeb3d09b1f95ab43a7535f54324a795bacb1b17cf3b543b6eff1c`.
These RTTs exclude discovery and link establishment and are not throughput results.

The browser explicitly reported `isSecureContext=false` and no `crypto.subtle`.
The Prns browser path still completed link establishment and the Resource response;
the test harness computes the expected request-path hash and checks returned bytes
on the host so that the harness itself does not require SubtleCrypto. This does
not confer secure-context-only capabilities on the page. Bluetooth, USB, service
workers, other browsers, and HTTPS provisioning need their own qualification.

The visible UI also passed its smoke: the same-host WebSocket URL was prefilled,
Connect reached Active, Close reached Closed, and a reload preserved the actual
LXMF destination identity. The test blocked HTTP requests outside the G4 origin;
none were attempted, and no page errors were recorded. This establishes that this
flow requires no external HTTP assets; it is not a physical WAN-disconnection or
whole-device traffic-capture test.

## Packaging and resource boundary

`build.hopspot.browser` builds matching WASM/SDK assets, supplies a local favicon,
prefills the explicitly selected same-host listener port, and writes licenses,
a hosting guide, build metadata, and per-file SHA-256 hashes. It refuses an
existing output directory and invalid ports. All 96 initial candidate packaged-file hashes and
public byte accounting were verified. The final package also includes this
qualification and the hosting guide as local documentation: all 99 packaged
file hashes passed, and its 91 public files are byte-identical to the
hardware-tested candidate.

| Component | Bytes |
| --- | ---: |
| Public browser files | 2,332,698 |
| HaLoW/WebSocket executable | 3,710,604 |
| Combined payload | 6,043,302 |
| Observed free writable overlay | 5,464,064 |

The payload already exceeds available overlay by 579,238 bytes, before filesystem
overhead, retained state, logs, and update rollback. A transfer archive's compressed
size is not its installed footprint. This qualifies temporary RAM hosting, not a
persistent installation. One post-transfer `uhttpd` observation showed 1088 KiB RSS
and 1172 KiB process high-water RSS; that is not a long-running memory bound.

See [the hosting guide](../browser-hosting.md) for the build and private-root server
commands. The checked-in browser smoke supports `PRNS_BROWSER_ORIGIN` to load the
app from the board instead of loopback. The UI smoke separately checks the actual
visible controls and retained identity. The TypeScript/WASM build, registry
verification, script syntax checks, invalid-port rejection, and existing-output
refusal passed. No Rust runtime change was needed for this step.

## Limits and follow-up

This is the existing developer browser playground with a local endpoint default,
not a finished appliance landing page or signed installer. The user still chooses
when to connect. Plain HTTP does not authenticate delivered application code, even
though Reticulum link traffic is encrypted. The board continues to own its native
node identity; the browser has a separate origin-scoped identity.

Next appliance work is AP/client networking and discovery, followed by a concrete
persistent storage/update strategy. Locally hosted HTTPS is a separate deployment
choice. Keep the currently functioning radio/driver/firmware stack intact.

## Cleanup

The temporary HTTP and Hopspot listeners were stopped, with private state flushed
on shutdown. Ports 8082, 4343, and 4344 were no longer listening. Wireless, mesh,
and HTTP UCI had no pending changes, and both radios remained healthy in their
original AP/station modes. The pre-existing G4 TCP Hopspot still returned the
exact page (75 ms request RTT). No persistent files or radio settings were changed.
