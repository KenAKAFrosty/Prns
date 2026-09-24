# Remote settings usability and workflows

This follows the [initial expanded management slice](2026-09-21-remote-control-management.md).
Source and host-test coverage are not interchangeable with installed-device
acceptance. No branch is pushed by this work.

## Scope

- Managed nodes open directly into settings, with one automatic read per focus,
  busy deferral, explicit retry and cancellation on departure. Interfaces,
  Device, Information and Access sections replace the long single list.
- Interface counters and node identity/connection diagnostics are disclosures.
  Current-state getters remain distinct from action-only setters.
- Wi-Fi uses native actor-owned Start, Inspect and Keep/Restore steps with a
  30-second step bound and the node-owned confirmation window. Passwords enter
  upstream zeroizing storage and never enter snapshots, retry queues or logs.
  A lost mutation reply is uncertain; no mutation is replayed automatically.
- Controller inventory/removal uses the existing typed management lifecycle.
  Inventory contains hashes only, this phone is protected locally, and the
  target is authoritative about protected grants. Adding another phone is not
  implemented; it needs verified public-identity exchange and reciprocal setup.

Logical source commits:

- `d4bc19331`: primary settings view and automatic reads.
- `abf6a2052`: typed native Wi-Fi trials, controller paging/removal, and generated
  bindings (29 files; most added lines are generated adapters).
- `29f021e58`: guided Wi-Fi/access presentation, SDK integration and UI tests.
- `b1c53968e`: bounded read-only activation reconciliation and honest zero-window
  wording after the first physical Wi-Fi trial.

## Host validation

- 294 app tests in 34 suites; formatting, lint and source/test/tools TypeScript
  checks pass. Coverage includes focus/busy/cancellation, stale generations,
  foreground Wi-Fi reinspection, cleared credentials, exact revisions, uncertain
  admission, no replay, self-protection and malformed pagination.
- 51 SDK tests in five suites, SDK formatting/lint/types all pass. All 11 outbound
  calls use platform preflight, and failed submissions are not repeated.
- 185 native unit tests plus four integration tests pass. The 24 Wi-Fi-specific
  tests cover bounded lifecycle/fault paths and an authenticated TCP target with
  trial, readiness refusal, rollback and keep. Controller tests cover operator
  denial, self-removal before dispatch, protected administrator refusal, operator
  removal and already-removed results. These targets are host fixtures, not E290
  Wi-Fi acceptance.
- Native strict Clippy, Rust formatting, generated API check and 26 binding-tool
  tests pass. The simulated Wi-Fi target feature is a dev-dependency only; it does
  not add a Wi-Fi host or transport to the production app composition.

Main logs are `scratch/prns-app/2026-09-21/settings-app-final-tests.log`,
`settings-app-{format,lint,final-typecheck}.log`, `settings-sdk-verify.log` and
`settings-android-build.log`. Native final results are in
`settings-native-final-tests.log`; the initial independent native checks used the
same existing external publication wrapper.

## Galaxy/E290 baseline exercise

The Galaxy S9+ (Android 10) began this pass running the previously installed
`fd211f5e2` UI APK, SHA256
`b7beb6c2960798d3936de1dac726366b7c599f09cf01bcb7d98fbbd3aa45c6b0`.
The E290 remained on the isolated diagnostic image `04b0be101bdd`; its build
label reports `0.3.7+0d00c25`. No firmware was changed for these tests.

The app read firmware and interface inventory over the existing pairing.
LoRa interface `0e2a7fdfaa0df7d5` reported US915, 915000000 Hz, 250000 Hz bandwidth,
SF9, coding rate 4/5, 22 dBm and 18 preamble symbols. An identical-profile
submission returned Applied. A subsequent 22 → 21 dBm change returned Applied,
and a fresh overview/interface read reported 21 dBm. The control route stayed
on Bluetooth. The original 22 dBm value was then submitted for restoration;
readback evidence is recorded with the final installation below.

Important limitation: the target's LoRa setter stores a manual profile.
Inventory does not report whether the initial resolved profile was automatic
or manual. Restoring the captured fields restores the on-air values, not proof
of restoration of the previous selection mode. The confirmation now warns
that saving may replace automatic selection; identical fields are not called
a strict no-op.

The passive board trace is `scratch/prns-app/2026-09-21/remote-settings-device-trace.log`.
It opens serial without toggling reset controls. The intermittent board-menu
freeze remains unresolved; successful settings traffic is not a fix for it.

## First workflow APK and physical checks

The standalone Android build from `29f021e58` passed package checks and was
installed over the existing Galaxy data at 21:12:48 EDT. APK SHA256:
`1b2299eb0992007f6c29461dc16cea11faa5b5e250cc65fe6d5490984f010d15`.
It retains Android 10 minimum, bundled JavaScript and development signing.
No Metro server is required. The cold launch preserved the pairing and showed
automatic settings loading plus Interfaces/Device/Information/Access navigation.

A fresh LoRa read returned the restored 22 dBm. The board was deliberately
restarted once using its exact USB port, without a flash or erase. A subsequent
overview/interface read reconnected and again returned 22 dBm. Access inventory
listed this phone and one other ID; this phone had no removal button. No actual
controller grant was revoked. Wi-Fi status returned the existing factory baseline.

Using a deliberately nonexistent test network (no real credentials), the app
started a trial. The board's Wi-Fi log confirmed absent network/disconnected
station while Bluetooth control remained usable. Keep was correctly refused
as not ready. Explicit Restore returned the factory baseline and empty form.
The board subsequently rejoined its original network and acquired IPv4 again.

This exposed two issues:

- Initial post-activation status returned zero remaining seconds before the
  scheduled activation executed. `b1c53968e` bounds read-only inspection to five
  seconds for that exact candidate; it does not repeat Stage/Activate. UI zero
  observations no longer claim a zero-second remaining window or expiry.
- Firmware rollback did not wake an existing 120-second station retry delay.
  Restore was requested around 01:19:28 UTC; scan resumed at 01:20:40, association
  at 01:20:43 and IPv4 was observed at 01:20:53. Source permits retry waits up to
  300 seconds, longer than the 120-second trial. A persisted rollback result is
  not proof that the radio worker has applied it. The separate upstream fix and
  its validation are recorded below.

Board reset receipt: `scratch/prns-app/2026-09-21/settings-board-reset.log`.
Post-reset physical trace: `settings-after-reset-trace.log` in the same directory.

## Retry-wake fix and final installation

`codex/wifi-credential-retry-wake` contains the isolated upstream candidate
`6ce5c9ff0`, based directly on fetched trunk `8c211827b`. It wakes the station
worker's retry wait when replacement credentials or a clear command arrive,
without consuming the latest-command mailbox in the wait. It changes two core
files; no protocol, deadline, persistence or confirmation rule changes. The app
branch includes it as `c81e78ebb`. The branch is local, not pushed, with no PR.

The candidate passes 211 shared-core tests, 32 ESP32 host-policy tests,
library-only strict Clippy and an exact-head E290 release build. New mailbox
regressions cover registered wakeups, replacement/clear retention, cancellation
and stale notifications. All-targets strict Clippy remains blocked by the
unchanged upstream `drop(first)` test warning; it is not reported as passing.

The final standalone Android build at `c81e78ebb` was installed over the existing
Galaxy data at 21:32:06 EDT. APK SHA256:
`89f82d19d9580cce7ed2af5d90e2490fdf5f6958778114939b3d65f52b19d24c`.
The cold launch and automatic settings read passed with the existing pairing.
Build log: `scratch/prns-app/2026-09-21/settings-final-android-build.log`.

The E290 test integration is `codex/e290-wifi-trial-diagnostics` at `c5ff748ab`,
the existing menu-freeze diagnostic branch plus this fix. Its sparse flash
verified all 2,351,488 bytes and preserved provisioning and grants. Application
image: 2,327,360 bytes, SHA256
`4202f6c8e18a69289da04fe784af1bc10452b8b890a61a07915d2d2660216474`.
This is not a hardware test of the isolated upstream branch alone. Flash receipt:
`scratch/prns-app/2026-09-21/wifi-retry-wake-diagnostic-flash.jsonl`.

The repeated missing-network trial on this installed pair initially displayed
119 seconds remaining without a manual refresh, confirming the bounded
read-only activation reconciliation. The phone remained reachable over Bluetooth.
The station entered a 120-second retry wait at 01:40:50.989 UTC. Restore reached
the target at 01:41:36.713; scanning resumed at 01:41:36.972 (259 ms later),
association completed at 01:41:39.983, and IPv4 was observed at 01:41:42.135.
The app returned to the factory baseline with cleared fields. A subsequent fresh
LoRa read still reported 22 dBm. No additional reset or serial reattachment
occurred during this exercise, and the passive capture was stopped afterward.
These are host-observed serial timestamps, not precise radio event latency.
Physical retry-wake evidence is recorded in
`scratch/prns-app/2026-09-21/settings-wifi-wake-fixed-trace.log`.

## Remaining qualification

Wi-Fi success needs an explicit test network; no saved workstation credentials
are read. No controller grant was actually removed on the physical board; native
authenticated fixture coverage is distinct from that acceptance. No iOS install,
Nordic hardware test, all-parameters test or production-release claim is made here.
