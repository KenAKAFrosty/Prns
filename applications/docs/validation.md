# Application validation and current limits

This is a development application, not a release-qualified client. The current
mobile implementation uses generated UniFFI bindings and one shared Rust image;
see the [binding boundary](../prns/native-composition/bindings/README.md).
Browser and Tauri runtime providers are not implemented.

## What the evidence covers

The generated-binding cutover and the earlier phone trials are different builds.
Do not transfer physical Bluetooth acceptance from the old bridge to the new one.

| Evidence | Recorded scope | Not established |
| --- | --- | --- |
| Generated-binding host checks | Typed snapshots, exact integers, offline storage, admission, cancellation, durable work after caller departure, and real TCP cancellation | Phone Bluetooth or OS lifecycle behavior |
| Generated-binding RN/Hermes harnesses | iOS and Android startup, four JavaScript runtimes per platform, reload/destruction cleanup, and one surviving native generation | Unlimited-reload memory bounds or arbitrary callback interfaces |
| Integrated Expo simulator/emulator checks | Identity/start/snapshot, offline contacts and process retention; Android React reload, service continuity, and isolated native lifecycle/import tests | Fresh physical pairing, radio recovery, or background delivery on this build |
| Earlier physical iOS builds | Accessory authorization, pairing, authenticated checks, direct messaging, and bounded suspension/restoration observations | Reliable first-attempt recovery, complete restoration behavior, or qualification of the generated-binding build |
| Earlier physical Android 10 builds | Pairing, direct messaging, Stop/Start, permission recovery, offline retry/cancel, retention, and twenty-two clean radio cycles | Qualification of the generated-binding build, newer Android versions, deep Doze, or power-loss durability |

The [generated-binding checkpoint](../checkpoints/2026-09-09-validation.md#generated-binding-cutover)
records the harness and integrated test boundaries. The [clean Android checkpoint](../checkpoints/2026-09-09-validation.md#clean-combined-checkpoint--september-9)
records exact APK/firmware hashes, failed comparisons, and the later copy-only
installation smoke. The [iOS observations](../checkpoints/2026-09-09-validation.md#historical-physical-observations)
retain the first failed delivery and UI recovery problems as well as successes.

The documentation consolidation does not requalify runtime or device behavior.
Follow-up source fixes and firmware-size work remain pending until their
specific results are recorded; a proposed fix is not validation evidence.

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

## Publication and repository blockers

The last matched constrained-Nordic comparison failed `t-echo-s140-v7` with
1,984 bytes of FLASH overflow. Upstream `1d4d4ba865` built with the same compiler
and 472 bytes of headroom. This is a branch integration regression, not an
Android runtime failure or a reason to change the board's memory layout.
The [dated measurement and publication record](../checkpoints/2026-09-09-validation.md#constrained-nordic-firmware-failure-and-publication-exception)
contains the exact comparison and gate boundaries. A correction is under
investigation; the firmware matrix has not been declared passing.

The recorded repository continuation passed all 24 host workspaces and the
listed Clippy, allocation, dependency-policy and unsafe-inventory checks.
Its browser package smoke built Rust but could not finish without the local
`wasm-bindgen 0.2.126` executable. A separate casework smoke had matcher-contract
failures; these are different commands, neither silently waived.

## Qualification still required

- Fresh generated-binding physical iOS and Android journeys: pairing, first
  authenticated request, two-way messaging, Stop/Start, caller cancellation,
  reload/process retention, and Bluetooth recovery.
- Pristine interactive identity-import onboarding, separately from picker
  guards and isolated native import tests.
- iOS suspension/restoration, authorized force-quit behavior and negative
  controls, protected-storage boundaries, and UI/first-delivery recovery.
- Newer Android permission/service and notification-channel behavior, deeper
  and longer battery idle, and wider board/transport coverage.
- Release/R8/signing, upgrades, retained-data migrations, and security/custody
  qualification before promising distribution or durable cross-version state.

The app uses event-driven iOS Bluetooth execution windows and an Android
foreground service; neither is a promise of continuous execution or guaranteed
background delivery. See the [current roadmap](roadmap.md) for implementation
priorities and the [dated archive](../checkpoints/2026-09-09-validation.md) for
the complete prior validation chronology.
