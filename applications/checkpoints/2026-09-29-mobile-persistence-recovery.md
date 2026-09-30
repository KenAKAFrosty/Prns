# Mobile persistence and Bluetooth restart checks

On September 29, 2026, retained-data Release builds on MetalbeardMobile and the
Galaxy S9+ exposed a shared persistence regression before the planned BLE
recovery trial. The corrected builds complete interval saves and exchange
source-verified messages in both directions. Restarting only Android also passes
the saved-contact send check. Restarting only iOS still leaves a stale session on
Android and blocks recipient resolution; the shared recovery work remains open.

## Builds and retained data

Both final binaries were built from `prns-app` at
`c2fee3fe9226bdec30db777c698b9a7922294d52`, after integrating the sandbox-safe
persistence fix. They are standalone, locally signed Release builds with bundled
JavaScript, not production-distribution or recorded-release qualification.
Android still supports API 29. The iOS artifact targets physical iPhones, contains
one aggregate `prns_app.framework`, retains dual-role Bluetooth background modes
and restoration identifiers, and passes strict signature verification. It has
neither AccessorySetupKit metadata/direct linkage nor the developer TCP fixture.

| Artifact | SHA256 |
| --- | --- |
| Android APK | `80930d717548fa592bf49d37e3838d3354ac0566228da566faef0ab09de20a86` |
| iOS executable | `d2e5ea52cea13d1d85b1883da511281da984ce6ad45e723653911a78b007bcbc` |
| iOS aggregate framework | `75d9a8eb916abd0a9ed5282e2bd38f69d346189d2baa34be8082012ab1cf3a3e` |
| iOS bundled JavaScript | `2d473b390357b923f992ce813aeb0b62527483d6522bc41a184d3982db05e17b` |

Installation updated the existing apps without uninstalling or clearing data.
Primary/controller identities, messaging names, saved contacts, paired-board
records and existing conversations were spot-checked across the upgrades. No
board was needed, flashed or reset. The only configured transport was
`AutomaticBluetoothLe`; USB provided installation/control/logging, not the
message path. No system pairing or accessory chooser was used.

Private build logs, signature/install receipts and device logs are retained in
`/Volumes/wavlink/dev/prns-mobile-build/logs/`. The ignored local trial ledger is
`scratch/prns-app/2026-09-29/phone-recovery-observations.json`. Artifact hashes
identify these binaries; a later source revision does not inherit their results.

## Persistence regression and correction

The initial builds from `a35e18dd4` started normally, then both nodes were found
stopped. Android's failure timing matched the first approximately five-minute
save interval; iOS was observed Failed later, without an exact transition time.
The UI reported
`PersistenceRestore: shared host shutdown failed: the recipe-managed persistence
worker could not save`. The app process remained alive. `PersistenceRestore` is
the displayed coarse failure category, not evidence that startup restore failed.

Upstream `fcc56b9d4` added Unix confirmation of every ancestor up to `/`.
Mobile sandboxes cannot necessarily open those ancestors even when the app's own
directory is writable. A host regression reproduced `PermissionDenied` with an
existing writable store below an execute-only parent, while direct owned-file
write and sync succeeded. Release phone logs did not expose the underlying OS
error; source, timing and the host reproduction established the repair to test.

The fix makes an existing supplied directory the caller's namespace durability
anchor. For new directories, it confirms the new chain through the first existing
parent. It captures those obligations before creation, retains them across
same-owner retries, preserves typed uncertainty and keeps post-rename durability
errors intact. Runtime construction and the SDK's persistence-directory lock now
use that preparation rather than silently pre-creating the directory. This does
not skip file/directory syncs or add arbitrary sandbox access.

A new owner cannot reconstruct another owner's incomplete directory creation.
That limitation is documented; same-owner retry coverage is not a fresh-owner
durability guarantee.

After installation, both final builds showed **Running**, **Persistent: Yes**,
**Restored: Yes**, and **Last flush: Interval**. The iPhone uptime exceeded
639 seconds; Android remained PID 308 from launch through this check. These are
successful physical interval-save observations, not simulated power-loss tests.
Android again showed **Last flush: Interval** after its later controlled restart,
with the replacement process still running as PID 3131.

### Source validation

- FileStore: 18 tests passed, including the red-before/green-after sandbox case,
  nested directory construction, confirmation retry and typed error conversion.
- Full core library: 2,079 passed, three ignored, no failures.
- Runtime constructor and native-host sandbox regressions passed without
  permission-fixture skips.
- SDK native-host library: all 49 tests passed.
- Core and native-host strict all-target Clippy, formatting and whitespace checks
  passed. Both Release builds passed; Android's native module suite passed eight
  tests.

### Upstream separation

The independent candidate is `codex/file-store-sandbox`, commit `6b4b0c7e3`, based
on upstream `586b075cd`. Suggested title: **Keep snapshot directory syncs inside
the application sandbox**. Its rationale is the mobile app's first interval-save
failure, with the generic FileStore/runtime/host regression tests above.

The SDK-only lock integration is `2f47e1d29` on `codex/react-native-sdk`, following
merge `d7050581d`. The app adopts it in `c2fee3fe9`. These changes are committed
locally; this continuation did not push branches or open/update PRs.

## Physical BLE results

Each phone explicitly announced once for the baseline. The other phone's
Discovered list showed its messaging name as just heard and already saved. Later
restart trials used Saved contacts without another manual announce.

| Trial | Observation |
| --- | --- |
| Galaxy to iPhone baseline | `BLE 0929 A1: Galaxy baseline`, sent 23:42:27 UTC; delivered in 119 ms; receiver showed Verified source |
| iPhone to Galaxy baseline | `I1`, sent 23:48:08 UTC; delivered in 209 ms; receiver showed Verified source |
| Restart only iPhone | Explicit terminate/relaunch at 23:50:05 UTC changed PID 2093 to 2098; Android stayed PID 308. Saved contact and identity association remained. `I2` lookup failed; draft retained and explicitly not queued |
| Diagnostic recovery | Disable/enable only Android's app Bluetooth, leaving both app processes running. Retrying unchanged `I2` delivered in 93 ms at 23:52:52 UTC, with Verified source on Android |
| Restart only Android | Explicit stop/relaunch at 23:54:16 UTC changed PID 308 to 3131, without changing iOS. Saved-contact send `BLE 0929 A2: Android restart` delivered in 144 ms at 23:55:22 UTC; iPhone showed Verified source |

Matching sender/receiver message IDs were checked for all four delivered messages:

- A1: `c117b4ed78e392197a9290baa5c999be4fa4c709972569f2899593710a5d88ee`
- I1: `d7ce07ae3bad3e406c724dd8a1bbffae10dbc40c7cb984ff99a292202d5785a6`
- I2: `7bfe6fe2cba8fcfb8f719aec1cf26b5d950716bf1767f7a22735ee4d9015f3ff`
- A2: `1cd63c980cf50527c20b6e5e98ca4ee3156023eab039823572cb59ec4bc13b87`

Both conversation counts advanced from 19 to 23, consistent with the four
delivered test messages and no duplicate visible in that bounded history.
Unrelated conversation counts remained 13 on iOS and three on Android. These UI
counts cover the latest loaded mailbox records, not a complete database audit.

During the failed iOS-restart trial, iOS showed Bluetooth Ready while Android
still showed one connected device. Android's log retained listener 3 without a
disconnect/close for it, while new physical candidates connected and closed.
Candidate churn also occurred during healthy operation, so it is not sufficient
evidence alone. Together with the isolated restart and same-draft recovery, this
supports stale session retention rather than contact loss.

The log's `data out` precedes native submission and does not prove completion or
remote receipt. Control payload lengths do not identify decoded message kinds,
and `policy close` does not expose the precise Rust rejection reason. No stronger
wire-level conclusion is claimed.

## Remaining work

Follow the [shared recovery sequence](../../docs/bluetooth-session-recovery.md):
retain and drive settled control ownership across native backends and runtimes,
then add actual completion receipts and negotiated, bounded liveness. Preserve
healthy incumbents and exact-session cleanup. Retaining controls alone cannot
detect every idle stale peer; do not replace this with app retries or periodic
Bluetooth resets.

The Android restart pass and diagnostic reset are not an iOS automatic-recovery
pass. These runs do not qualify app Off/On without intervention on the peer,
radio/permission recovery, long idle, natural suspension, OS-triggered relaunch,
or background delivery. Verified fresh messages do not prove every retained-key
or unknown-sender path, and older unverified messages were not re-verified.
