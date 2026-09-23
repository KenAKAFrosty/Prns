# Application validation and current limits

This is a development application, not a release-qualified client. The current
mobile implementation uses generated UniFFI bindings and one shared Rust image;
see the [binding boundary](../prns/native-composition/bindings/README.md).
Browser and Tauri runtime providers are not implemented.

The [September 21 integration](../checkpoints/2026-09-21-remote-control-management.md)
rebases the app on upstream `8c211827b` and implements the first expanded
[remote-control read/write slice](remote-control-expansion.md): overview,
interfaces/configuration/peers, discovery groups and 13 typed changes. New board
pairings grant Administrator authority plus the board's exact supported request
set, with full-control disclosure. All 30 upstream request kinds are represented.
The subsequent [settings workflow slice](../checkpoints/2026-09-21-remote-settings-workflows.md)
adds automatic reads/organized sections, guided Wi-Fi trials and controller
inventory/removal. Granting another controller access and the legacy Wi-Fi setter
remain unimplemented.

The linked workflow checkpoint records Galaxy/E290 LoRa read/write and restored
values after restart, access inventory, and failed-network trials with explicit
rollback. A separate firmware retry-wake candidate restores scanning promptly
after rollback. Successful new-network Keep and iOS qualification remain open.

The [September 22 UI refinement](../checkpoints/2026-09-22-ui-density.md) adds
compact grouped layouts without native-contract or firmware changes. Its 319 app
tests pass, and the retained-data Galaxy installation was checked for node reads,
keyboard visibility, message disclosures and 1.5x/2x text navigation. These are
presentation checks, not additional remote-write or iOS qualification.

The [subsequent MetalbeardMobile install](../checkpoints/2026-09-22-ios-install.md)
includes corrected back arrows and passes the 320-test app suite. Its signed,
bundled-JavaScript iOS build installs but cannot launch under the newly installed
iOS 27/Xcode 27 scene-lifecycle requirement. A scene-aware native Bluetooth startup
migration is required; installation is not successful iOS UI or runtime acceptance.

The expanded read/write screens have limited Galaxy/E290 evidence, not full
physical qualification. Fresh Android
pairing succeeded in a user-run trial against the separate E290 diagnostic
firmware, after earlier intermittent freezes/reboots; that fault remains open.
This is not production-firmware or iOS acceptance. Earlier phone, firmware-size
and background observations below do not qualify this rebase. The new checkpoint
records each build's checks and limits; source/UI tests are not device acceptance.
No deployed-pairing migration is required for these disposable development devices.

## What the evidence covers

The generated-binding cutover, follow-up builds and earlier phone trials are
different evidence. Do not transfer physical acceptance between their binaries.
Dated checkpoints describe their recorded source and builds, not current PR
status. The [September 15 integration](../checkpoints/2026-09-15-upstream-integration.md)
records the refreshed source, APK and firmware hashes. Android acceptance is in
progress; its recorded journeys are separate from earlier phone results.

| Evidence | Recorded scope | Not established |
| --- | --- | --- |
| Generated-binding host checks | Typed snapshots, exact integers, offline storage, admission, cancellation, durable work after caller departure, and real TCP cancellation | Phone Bluetooth or OS lifecycle behavior |
| Generated-binding RN/Hermes harnesses | iOS and Android startup, four JavaScript runtimes per platform, reload/destruction cleanup, and one surviving native generation | Unlimited-reload memory bounds or arbitrary callback interfaces |
| Integrated Expo simulator/emulator checks | Identity/start/snapshot, offline contacts and process retention; Android React reload, service continuity, and isolated native lifecycle/import tests | Fresh physical pairing, radio recovery, or background delivery on this build |
| Earlier physical iOS builds | Accessory authorization, pairing, authenticated checks, direct messaging, and bounded suspension/restoration observations | Reliable first-attempt recovery, complete restoration behavior, or qualification of the generated-binding build |
| Earlier physical Android 10 builds | Pairing, direct messaging, Stop/Start, permission recovery, offline retry/cancel, retention, and twenty-two clean radio cycles | Qualification of the generated-binding build, newer Android versions, deep Doze, or power-loss durability |
| Generated-binding Android follow-up, before footprint integration | Existing-grant checks, Stop/Start without resurrection, two-way Python LXMF messaging and cold-process retention | Fresh pairing, controlled radio recovery, held-request cancellation or qualification of the later rebuilt APK |
| Recorded generated-binding Android APK | Cold retained-grant check, one real Settings Bluetooth cycle with successful first reconnect check, and retained-data spot-check | Fresh pairing, repeated messaging, full controller power-off, long idle or controlled cancellation |
| Pre-diagnostic generated-binding iOS framework | Initial timeout, later foreground checks/two-way messaging and restart retention; logged SIGTERM restoration trial; separate controlled off-screen receipt, stored-message UI and first resumed Check in 359 ms | Initial-failure cause or fix, fresh pairing or full lifecycle qualification; no native timeline for the controlled repeat |
| Pre-correction iOS diagnostic framework | USB-captured restored-handshake stall, local cleanup and fresh handshake; incoming proof in 474 ms, first resumed Check in 383 ms and four exact stored-message checks | A transport fix, original-startup-failure cause, fresh pairing, natural suspension, continuously locked or Metro-off qualification |
| Corrected iOS restoration framework | Two captured one-shot recoveries without the old stall; SIGTERM reset-to-Welcome in 2.395 s, incoming proof in 485 ms, first resumed Check in 354 ms, stored-message verification and a separate full two-way exchange | Initial ordinary-start failure or 32-second-delay fix, fresh pairing, confirmed no-touch window, natural suspension, continuous lock or Metro-off qualification |
| Corrected iOS framework with offline Inbox UI fix | Cold denied-access saved messages/contacts, Failed → Retry → Queued → Cancelled, same-record retention after process restart; restored access, first Check in 378 ms, incoming delivery and bounded no-resend | Retry as first SDK call, power-loss durability, Android acceptance of this UI revision, fresh pairing or broader lifecycle qualification |
| September 15 first integrated Android APK | Pristine import, cold retention, two-way Python messaging, cold Bluetooth-off Retry/Cancel and retained cancellation | Acceptance of later Expo patches, fresh pairing, authenticated node requests or broader lifecycle behavior |
| September 15 Expo-patched standalone Android APK | Repeated pristine import/cold retention, retained-data install, cold offline Retry/Cancel with same-record retention, two-way Python messaging, Stop/Start, three radio cycles, one controlled off-screen receipt, fresh pairing, authenticated checks and bounded pending-read route-exit/retry recovery | On-wire cancellation after board receipt, all Stop/background races, deep Doze/continuous lock, resolution of the board navigation freeze or iOS acceptance of this integration |

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

The [offline continuation](../checkpoints/2026-09-09-offline-and-upstream-refresh.md)
fixes Inbox hiding saved messages while Bluetooth admission waits. The React-only
change passed full app checks and one physical iOS cold offline Retry/Cancel
journey without changing the native binary or bypassing authorization. The same
cancelled record survived a restart and a bounded return of the messaging peer.

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

The [September 15 integration](../checkpoints/2026-09-15-upstream-integration.md)
passes five canonical memory contracts and fresh T-Echo S140 v7/E290 resource
builds. T-Echo has **328 bytes** of FLASH and 4,032 bytes of static-RAM headroom;
E290 has 12,923,584 and 47,860 bytes respectively. These are working-tree
measurements on the recorded source/toolchain, not a new full-profile or
clean-commit assurance result. E290's canonical flash and subsequent boot were
verified; saved Wi-Fi/NVS settings were retained.

The [September 10 app qualification](../checkpoints/2026-09-10-app-publication.md)
records fresh full-app, detached-consumer and iOS platform checks for core
`451e669da`, and that build's repository/firmware results. Earlier checkpoints
below remain measurements of their own source and builds, not current PR state.

The [September 10 test fixes](../checkpoints/2026-09-10-app-ci-fixes.md) address
the app's Windows-only unused import and a release fixture whose source archive
outgrew the firmware region. Both are test-only changes; their checks and the
remaining full-publishing boundary are recorded separately from firmware results.

The constrained-Nordic integration regression was corrected without changing
layouts or capacities. The earlier integrated candidate passed all 14 configured
resource profiles; T-Echo S140 v7 retained only 616 bytes of nominal FLASH
headroom. The earlier clean upstream port separately retained 1,216 bytes.
The [follow-up measurement](../checkpoints/2026-09-09-follow-up.md#firmware-footprint-correction)
distinguishes those builds and preserves the earlier 1,984-byte overflow.
This is not a new full repository publishing-gate result or device qualification.

The [September 9 Nordic refresh](../checkpoints/2026-09-09-offline-and-upstream-refresh.md#unpublished-upstream-refresh)
fits T-Echo S140 v7 with 1,224 bytes remaining; its diagnostics-only parent fails
by 1,344 bytes. These changes were published together as
[#215](https://github.com/KenAKAFrosty/Prns/pull/215) on September 12. The refreshed
iOS candidates passed focused FFI and canonical inventory checks; native recovery
and write batching were subsequently published as
[#218](https://github.com/KenAKAFrosty/Prns/pull/218) and
[#223](https://github.com/KenAKAFrosty/Prns/pull/223). The separate
[write-batch source correction](../checkpoints/2026-09-09-corebluetooth-write-batches.md)
was committed and integrated with a new core pin; native and generated-contract
checks passed. That checkpoint installed no new phone binary. Publication does
not add peripheral-role hardware acceptance or transfer earlier phone results
to a new build.

The [publication continuation](../checkpoints/2026-09-09-publication-preparation.md)
adds a real Mac manager/radio smoke and refreshed #199/#208/#209 candidates.
Focused tests, iOS checks and an isolated Bluetooth-plus-USB compilation/lint
comparison pass. The radio smoke receives no writes. The subsequent
[CI corrections](../checkpoints/2026-09-09-ci-corrections.md) fix the reproduced
split-resource failure and notices drift on separate branches. Linux validation
passes 1,887 core tests, 25 integration tests and 50 repeated split responses;
the integrated native and iOS checks pass with core pin `451e669da`. Physical
batching and a full remote CI result remain open. At that checkpoint, no existing
shared PR head, phone binary or board firmware changed.

The later [publication record](../checkpoints/2026-09-09-ci-publication.md)
opens independent PRs #213 and #214 after the normal publishing hook passed.
The isolated resource branch passed 22 host workspaces and all 41 selected
parity gates, including all 14 configured firmware profiles, browser smoke,
JVM compilation, Swift smoke and dependency/inventory checks. Its T-Echo S140 v7
build retains 472 FLASH bytes; this does not replace the app or Nordic-candidate
measurements above. The notices branch also passed its separate canonical check.
GitHub checks started afterward; #213's macOS Bluetooth job failed on the existing
USB fixture mismatch addressed by #199. Other remote results were still pending
at that readback. Passing the local hook is not a full remote-matrix pass.

The subsequent [five-PR refresh](../checkpoints/2026-09-09-pr-refresh.md) publishes
the prepared #199/#202/#207/#208/#209 heads, with backup refs and exact leases.
Fresh, source-verified publishing hooks passed from #209 (41 selected gates)
and #199 (14), each with 22 host workspaces and all 14 firmware profiles.
T-Echo S140 v7 retained 472 and 480 FLASH bytes respectively. Fresh focused FFI
and USB tests passed; the app remote and installed devices were unchanged.
The new GitHub runs were still incomplete at the post-publication readback.

On September 12, eleven new draft PRs were published:
[#215](https://github.com/KenAKAFrosty/Prns/pull/215),
[#216](https://github.com/KenAKAFrosty/Prns/pull/216),
[#217](https://github.com/KenAKAFrosty/Prns/pull/217),
[#218](https://github.com/KenAKAFrosty/Prns/pull/218),
[#219](https://github.com/KenAKAFrosty/Prns/pull/219),
[#220](https://github.com/KenAKAFrosty/Prns/pull/220),
[#221](https://github.com/KenAKAFrosty/Prns/pull/221),
[#222](https://github.com/KenAKAFrosty/Prns/pull/222),
[#223](https://github.com/KenAKAFrosty/Prns/pull/223),
[#224](https://github.com/KenAKAFrosty/Prns/pull/224), and
[#225](https://github.com/KenAKAFrosty/Prns/pull/225).
Each exact published branch passed its unchanged normal first-publication hook:
22 host workspaces, 41 selected parity lanes and all 14 firmware profiles.
The shared Host snapshot [#201](https://github.com/KenAKAFrosty/Prns/pull/201)
was updated separately to `851b8b359`; its two normal follow-up pushes passed
23 host workspaces and their separately selected parity checks. The app remained
at `f3755a977`, and publication involved no device installation, board flash or
new hardware acceptance. Remote CI still had inherited USB-fixture/notices
failures, reviewed test-only CodeQL findings and an unresolved Android Auto-WiFi
test timeout at that readback. These are dated observations, not a current CI
snapshot or a claim that all published branches passed the remote matrix.

Keep compiled Cargo targets separate for each worktree. A reused target accepted
older source timestamps and ran another checkout's test binary; its firmware
tool also retained that checkout's compile-time root. That attempt was stopped,
discarded as evidence and rerun in unseeded targets. Verify source ownership,
not just a successful exit code, before attaching results to a branch.

The earlier app repository continuation passed all 24 host workspaces and the
listed Clippy, allocation, dependency-policy and unsafe-inventory checks.
Its browser package smoke built Rust but could not finish without the local
`wasm-bindgen 0.2.126` executable. A separate casework smoke had matcher-contract
failures; these are different commands, neither silently waived.

## Qualification still required

- Rebuild both native apps and matching board firmware, reset/re-pair test
  devices as needed, and exercise the full-control disclosure and the new read/
  write screens. Check differing capabilities, pagination, denied or busy
  operations, navigation/Stop cancellation, disconnection and uncertain write
  outcomes. Qualify Android and iOS separately; no current physical acceptance
  is claimed for this expansion.
- Complete the exact-build physical journeys in the follow-up checkpoint.
  The September 15 Android build now passes fresh pairing, authenticated checks,
  bounded pending-read route-exit/retry recovery, three repeated radio cycles and
  one bounded off-screen receipt. iOS fresh pairing, wider cancellation races,
  broader OS lifecycle and repeated authenticated-request recovery remain open;
  grant reuse is not new pairing.
- Diagnose the E290 navigation freeze/startup-notice event in the September 15
  checkpoint. Its timing and lost USB/Bluetooth connections are consistent with
  a watchdog restart, but the reset reason and trigger were not captured.
- Pristine interactive identity-import onboarding on iOS. The September 15
  Galaxy build passed the interactive import and cold-retention journey;
  picker guards and isolated native tests alone are not phone acceptance.
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
