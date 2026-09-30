# Bluetooth ownership and authenticated-key retention

These are source and automated-validation results from September 28, 2026.
No updated mobile binary was installed. Android was not visible to the device
tools during this continuation. The earlier
[controlled restart failure](2026-09-24-messaging-discovery.md#controlled-ios-cold-restart-check)
therefore remains open; these checks do not replace that physical journey.

## Implemented changes

- LXMF refreshes existing bounded key retention only after authenticated
  recipient-link establishment or verified inbound source/signature binding.
  The [retention follow-up](2026-09-24-messaging-discovery.md#authenticated-key-retention-follow-up)
  records the policy, persisted-key evidence and remaining Unknown cases.
- Tokio admits each Bluetooth member with a private session token. Delayed
  close events cannot remove replacements or invoke their backend cleanup.
  Embassy's live-slot failure handling has no detached close queue and needed
  no equivalent token change.
- Android Rust handshake/data owners retain a lease for the exact physical
  connection. Rejecting a same-address challenger leaves the keeper intact;
  queue identity prevents a late lease drop from closing a reused connection ID.
- Android client writes and server notifications have a 30-second progress
  deadline from the first attempt. Matching completion clears it; Busy retries
  do not. Expiration terminates the physical link instead of freeing its lane.
- Android client callbacks check their GATT owner. Server callbacks use retained
  notification/disconnect ownership and a registration epoch. A timed-out
  peripheral address stays quarantined until server reset, protecting a future
  owner from ambiguous late callbacks. Quarantine is bounded and saturation
  refuses new inbound work; it does not automatically recover the address.
- SDK and Hopspot compile the same Kotlin GATT state and callback-owner helpers
  and lifetime tests. The SDK archive includes the canonical sources; no new
  Expo dependency or native image is added to Hopspot.

## Remaining work

An idle stale PRNS session can still appear physically connected, and settled
control receivers are still discarded. No Ping/Pong extension, capability
characteristic or graceful-close protocol has been enabled. The
[shared recovery sequence](../../docs/bluetooth-session-recovery.md) records the
required control lifetime, completion, compatibility and qualification work.
Keep this in shared core/platform ownership, outside contact and app policy.

## Validation

- LXMF: 50 unit tests, three integration tests and strict all-target Clippy.
- App native integration: 198 tests with Apple and host-test features.
- Native platform library: 123 tests passed, one real-radio test intentionally
  ignored; strict all-target Clippy, iOS target Clippy and Android target check
  passed. Fourteen tests cover the Android bridge, including the new lease
  regressions.
- Tokio Bluetooth: 23 tests and strict all-target Clippy passed, including stale
  close at the same or a different address and unchanged healthy-keeper behavior.
- Android Kotlin: both consumers' production and unit-test compilation passed.
  The full SDK JVM suite passed 53 tests, including lifecycle and radio recovery;
  Hopspot passed 22 focused state, lifetime and callback tests. The eleven new
  lifetime tests are compiled from one shared source in both consumers. Final
  tested sources were checked byte-for-byte against this checkout.
- SDK package staging: six tests and direct canonical-helper inclusion checks
  passed. The deleted duplicate is absent from the staged source inventory.

Android logs and the source hash manifest were recorded under
`/Volumes/wavlink/dev/prns-android-gatt-lifetime-*-20260928.*`; Gradle XML results
were recorded in the external adoption checkout's staged SDK and Hopspot build
directories. That checkout was removed during September 29 cleanup; these are
historical evidence locations, not current build paths. This records compiled
code and JVM behavior, not physical GATT callback timing on Android.

Hosted CI, updated APK/iOS builds, phone installs and physical recovery tests
were not performed for these changes.
