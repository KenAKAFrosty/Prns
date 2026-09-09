# Application validation and current limits

This is a development application, not a release-qualified client. The current
mobile implementation uses generated UniFFI bindings and one shared Rust image;
see the [binding boundary](../prns/native-composition/bindings/README.md).
Browser and Tauri runtime providers are not implemented.

## What the evidence covers

The generated-binding cutover, follow-up builds and earlier phone trials are
different evidence. Do not transfer physical acceptance between their binaries.

| Evidence | Recorded scope | Not established |
| --- | --- | --- |
| Generated-binding host checks | Typed snapshots, exact integers, offline storage, admission, cancellation, durable work after caller departure, and real TCP cancellation | Phone Bluetooth or OS lifecycle behavior |
| Generated-binding RN/Hermes harnesses | iOS and Android startup, four JavaScript runtimes per platform, reload/destruction cleanup, and one surviving native generation | Unlimited-reload memory bounds or arbitrary callback interfaces |
| Integrated Expo simulator/emulator checks | Identity/start/snapshot, offline contacts and process retention; Android React reload, service continuity, and isolated native lifecycle/import tests | Fresh physical pairing, radio recovery, or background delivery on this build |
| Earlier physical iOS builds | Accessory authorization, pairing, authenticated checks, direct messaging, and bounded suspension/restoration observations | Reliable first-attempt recovery, complete restoration behavior, or qualification of the generated-binding build |
| Earlier physical Android 10 builds | Pairing, direct messaging, Stop/Start, permission recovery, offline retry/cancel, retention, and twenty-two clean radio cycles | Qualification of the generated-binding build, newer Android versions, deep Doze, or power-loss durability |
| Generated-binding Android follow-up, before footprint integration | Existing-grant checks, Stop/Start without resurrection, two-way Python LXMF messaging and cold-process retention | Fresh pairing, controlled radio recovery, held-request cancellation or qualification of the later rebuilt APK |
| Current generated-binding Android APK | Cold retained-grant check, one real Settings Bluetooth cycle with successful first reconnect check, and retained-data spot-check | Fresh pairing, repeated messaging, full controller power-off, long idle or controlled cancellation |
| Pre-diagnostic generated-binding iOS framework | Initial timeout, later foreground checks/two-way messaging and restart retention; logged SIGTERM restoration trial; separate controlled off-screen receipt, stored-message UI and first resumed Check in 359 ms | Initial-failure cause or fix, fresh pairing or full lifecycle qualification; no native timeline for the controlled repeat |
| Pre-correction iOS diagnostic framework | USB-captured restored-handshake stall, local cleanup and fresh handshake; incoming proof in 474 ms, first resumed Check in 383 ms and four exact stored-message checks | A transport fix, original-startup-failure cause, fresh pairing, natural suspension, continuously locked or Metro-off qualification |
| Corrected iOS restoration framework | Two captured one-shot recoveries without the old stall; SIGTERM reset-to-Welcome in 2.395 s, incoming proof in 485 ms, first resumed Check in 354 ms, stored-message verification and a separate full two-way exchange | Initial ordinary-start failure or 32-second-delay fix, fresh pairing, confirmed no-touch window, natural suspension, continuous lock or Metro-off qualification |

The [generated-binding checkpoint](../checkpoints/2026-09-09-validation.md#generated-binding-cutover)
records the harness and integrated test boundaries. The [clean Android checkpoint](../checkpoints/2026-09-09-validation.md#clean-combined-checkpoint--september-9)
records exact APK/firmware hashes, failed comparisons, and the later copy-only
installation smoke. The [iOS observations](../checkpoints/2026-09-09-validation.md#historical-physical-observations)
retain the first failed delivery and UI recovery problems as well as successes.

The [follow-up checkpoint](../checkpoints/2026-09-09-follow-up.md) records the
three completed source fixes, firmware measurements and bounded phone journeys.
Its final detached/Swift gates and bounded Android radio smoke passed. The iOS
framework later passed foreground checks and two-way messaging, but its initial
disconnected timeout remains unexplained. A logged developer-SIGTERM restoration
trial included possible user foreground activity and does not qualify background
reconnection/delivery. A separate agreed no-touch repeat proved one off-screen
incoming message before foregrounding; its first resumed Check took 359 ms and
the stored message was verified in the UI. That repeat has no native startup
timeline and a 32.024-second submission-to-proof delay. Neither trial adds a
second full two-way pass. Continuous lock, natural suspension, quiet idle and
Metro-off behavior remain unqualified. The checkpoint preserves the separate
chronologies, missing logs, chooser question and successful scrolling retest.

The [later recovery investigation](../checkpoints/2026-09-09-ios-recovery-latency.md)
records separate diagnostic and corrected builds. The diagnostic USB timeline
distinguishes a stalled restored handshake from a successful fresh one. The
isolated correction is now committed and integrated; two bounded physical
restorations avoided that stall, followed by a successful resumed request and
messaging. The previous long LXMF delay did not reproduce, but its cause and the
initial ordinary-start failure remain unexplained. Do not transfer these results
to broader background or exact-build pairing qualification.

## Repeatable checks

Use the [workspace setup and checks](../README.md#generate-build-and-check) with
the versions in [the compatibility record](../release/compatibility.json).
The principal gates are:

- `verify`: generated-output/provenance, portable native, SDK, UI, route,
  configuration, and web-export checks.
- `mobility:verify`: a clean tracked application export with its exact recorded
  core revision and JavaScript artifact, including native/Python LXMF exchange.
- `native:ios:test`: explicit macOS Swift lifecycle and release-symbol checks.
- Root application-boundary, personal-path, and diff-selected pre-push checks.

The detached check validates extraction, not phone packaging or remote source
availability unless its recorded run actually uses a remote source. Simulator,
host tests, an APK build, and a physical journey answer different questions.
Platform procedures are in the [iOS](ios.md) and [Android](android.md) guides.

## Firmware and repository checks

The constrained-Nordic integration regression is corrected without changing
layouts or capacities. The integrated candidate passes all 14 configured
resource profiles; T-Echo S140 v7 retains only 616 bytes of nominal FLASH
headroom. The clean upstream port separately retains 1,216 bytes on that target.
The [follow-up measurement](../checkpoints/2026-09-09-follow-up.md#firmware-footprint-correction)
distinguishes those builds and preserves the earlier 1,984-byte overflow.
This is not a new full repository publishing-gate result or device qualification.

The recorded repository continuation passed all 24 host workspaces and the
listed Clippy, allocation, dependency-policy and unsafe-inventory checks.
Its browser package smoke built Rust but could not finish without the local
`wasm-bindgen 0.2.126` executable. A separate casework smoke had matcher-contract
failures; these are different commands, neither silently waived.

## Qualification still required

- Complete the exact-build physical journeys in the follow-up checkpoint.
  Fresh generated-binding pairing, controlled caller cancellation and current
  repeated radio/lifecycle recovery remain open; grant reuse is not new pairing.
- Pristine interactive identity-import onboarding, separately from picker
  guards and isolated native import tests.
- iOS natural suspension and repeated restoration, authorized force-quit
  behavior and negative controls, protected-storage boundaries, and
  repeatable delivery/resume recovery with complete native timelines. The bounded
  SIGTERM and off-screen trials do not qualify OS reclamation or App-Switcher
  force quit.
- Newer Android permission/service and notification-channel behavior, deeper
  and longer battery idle, and wider board/transport coverage.
- Release/R8/signing, upgrades, retained-data migrations, and security/custody
  qualification before promising distribution or durable cross-version state.

The app uses event-driven iOS Bluetooth execution windows and an Android
foreground service; neither is a promise of continuous execution or guaranteed
background delivery. See the [current roadmap](roadmap.md) for implementation
priorities and the [dated archive](../checkpoints/2026-09-09-validation.md) for
the complete prior validation chronology.
