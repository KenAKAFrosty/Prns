# Cold offline actions and upstream refresh — September 9, 2026

This continues the [iOS recovery investigation](2026-09-09-ios-recovery-latency.md).
It records one app-only fix, a bounded physical journey and unpublished upstream
replacement candidates. It is not a new release or full publication qualification.

## Saved messages while Bluetooth access is blocked

On MetalbeardMobile, disabling only the app's accessory authorization reproduced
an Inbox bug: Nodes correctly requested Bluetooth access, but Inbox showed
indefinite preparation and false empty-state text despite existing messages.
The same behavior followed an explicit process termination and manual cold launch.
Contacts remained available through their independent storage path.

Commit `c230870e6` lets Inbox read storage while network admission is waiting.
It displays one actionable access-needed card, waits for a successful mailbox
read before claiming an empty mailbox, and keeps peer/network reads behind the
existing running-state gate. It changes only React presentation and tests; the
native admission policy, storage owner, generated bindings and SDK are unchanged.

Validation passed: 80 focused tests, independent review, and full app verification
including 221 UI tests, Expo Doctor 21/21, typecheck, formatting, lint, route/config
checks and web export. This React-only change did not trigger a native rebuild.

### Exact physical build and scope

| Component | Evidence |
| --- | --- |
| Phone | MetalbeardMobile, iPhone 14 Pro, iOS 26.6.1; USB and iPhone Mirroring |
| JavaScript | `c230870e6`, served by restarted development Metro on port 8088 |
| Installed native build | Unchanged `31dbe75e0`, core `3e61d5ac5` |
| Framework SHA-256 | `d4938e521b93f02317330f281feb34cf23e82c7640d14dc898d34bf4b374e191` |
| Board | Unchanged firmware `ae411b4197ee`; no flash, reboot or serial operation |
| Android | Absent from ADB; no new Android physical evidence |

The initial phone process was already different from the previous checkpoint's
last process. The intervening interval was not observed and adds no idle or
reclamation evidence. Its first retained-node Check completed in 467 ms.

### Cold denied-access journey

1. Observed the known Python peer, stopped it, then submitted one disposable
   message. After the attempt settled, the corrected cold UI explicitly showed
   **Failed after one failed attempt**; the earlier Sending display alone was
   not used to infer persisted failure.
2. Disabled only the app-specific allowed-to-connect switch in iOS Settings.
   Global Bluetooth, Metro, the accessory record and Prns pairing grant remained
   intact. A saved contact could be read and a destination-only contact saved.
3. After explicit SIGTERM/manual launch, PID 11137 reported
   `setupRequired`, zero authorized accessories and `nativeStart=notRequested`.
   Inbox showed the saved conversation and 12 messages. Retry changed the failed
   record to Queued; Cancel changed the same record to Cancelled, with both
   action buttons removed. Its message hash did not change.
4. Another SIGTERM/manual launch created PID 11144 with the same denied-access
   state. Inbox retained the message count, contact label, cancelled hash and
   cancellation timestamp. No native-start marker was observed before access
   was restored.
5. Restored the accessory switch and verified it ON. The same PID 11144 admitted
   native startup only after the authorized roster returned, completed an
   ordinary Bluetooth handshake, and returned the first node Check in 378 ms.
6. Returned the Python peer. Both sides observed the exact destination, and a
   new incoming message reached delivery proof in 319 ms and appeared in the
   phone as Received / Verified source. The cancelled record remained unchanged.

The no-resend observation ran from peer observation around 20:21:15 UTC to an
intentional fixture stop at 20:23:50 UTC, approximately 155 seconds. No receipt
of the cancelled payload was observed; that payload would fail this fixture's
strict inbound expectation. This is a finite observation with a successful
incoming delivery, not a guarantee for arbitrary time. The fixture was stopped
with exit 130 after this check, not counted as a full two-way exchange.

The cancelled message was
`992c766ba8d1e387fe8613f91426d4345afccc97e68d7ea585cbe5fd34300436`,
with the unchanged local UI timestamp `9/9/2026, 4:13:15 PM`. The new received message was
`5be10d7244bd6f30a8d743b46540ccb8b9dd6ac624af236b594ff862eccccdca`;
fixture submission/proof were 20:23:16.083 / 20:23:16.402 UTC. Delivery proof and
the later stored-message UI check are separate observations.

Private logs and action notes are in
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/ios-offline.Yh8XZZ/`.
The bounded payload-free USB readers exited normally at their time limits.
For PID 11144, device timestamps and host ingestion timestamps differ by about
four seconds; do not mix the clocks to calculate latency.

This closes one generated-binding iOS cold offline UI Retry/Cancel and restart
retention journey. It does not establish Retry as the first SDK operation,
power-loss durability, natural OS reclamation, fresh pairing, pristine import,
held-request cancellation or broader background behavior. Access is restored;
the test peer and readers are stopped, and Metro remains available.

## Unpublished upstream refresh

Upstream trunk was checked at `1d4d4ba8652875ee6fda835c633398d1bab3419a`.
New local copies preserve every original and published ref. No push, PR edit,
PR creation or published-history rewrite occurred.

| New candidate | Exact tip | Parent |
| --- | --- | --- |
| `codex/refresh-ios-manager-policy` | `47241e744334a2e4aaa79090bc3c3d5b4c1dccc4` | Trunk |
| `codex/refresh-ios-restored-session` | `799c09690b19dbe279c5af2280c4669978cd8d63` | Manager policy |
| `codex/refresh-ios-native-recovery` | `a2a75d13d40d319ca3fe2a54ce22d4253153c9df` | Restored sessions |
| `codex/refresh-request-ingress-diagnostics` | `67252dc502135993f15d7e4856cb10164a906c13` | Trunk |
| `codex/refresh-request-ingress-footprint` | `72d7489286a7c817882a6063307c636aa291e78f` | Request-ingress diagnostics |

### iOS stack

All ten original contribution patches retain exact range-diff matches. Canonical
unsafe inventory generation and the actual checks pass at each stage. Two
generated-snapshot commits account for FFI block/token counts of 124/185 →
138/199 → 140/201; all 703 packages, graph membership, features and audit policies
are unchanged. Final FFI tests pass with and without logging (78 passed and one
hardware test ignored each), along with strict Clippy and the iOS cross-check.
No device binary was rebuilt from these refreshed refs.

Broader CI still depends on the separate USB Auto test-seam fix in #199. The
published #208/#209 descendants are not restacked by this work. A scoped unsafe
source review also found a separate, pre-existing peripheral write-batch issue:
the callback responds per request, and new restoration refusal paths repeat
that pattern. Apple's [write-batch contract](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanagerdelegate/peripheralmanager%28_%3Adidreceivewrite%3A%29?language=objc)
requires one response for the whole callback and all-or-none handling.
This affects the peripheral/server role, not the app's
central-only path; phone results do not qualify it. No batching change was mixed
into the inventory refresh. Resolve it separately before declaring the parent
stack globally ready.

The [subsequent write-batch correction](2026-09-09-corebluetooth-write-batches.md)
records the separate source fix and its validation; the measurements above remain
the original refresh snapshot.

### Constrained Nordic target

Diagnostics alone now exceeds T-Echo S140 v7 FLASH by **1,344 bytes**. The combined
diagnostics/footprint candidate passes at **621,368 bytes**, leaving **1,224 bytes**
of the configured 622,592-byte image region. Static RAM plus the configured stack
leaves 4,816 bytes. No layout, capacity or feature reduction was used. This is a
new isolated-trunk measurement, not a replacement for the integrated app build's
previously measured 616-byte headroom.

The footprint patch depends on the diagnostic types and helper. Recommend **one
PR from the combined child against trunk**, retaining its three logical commits,
so the proposed tip fits the target. Two separately green PRs cannot be obtained
by simply reordering these patches.

Seven quiet-path regressions pass. Combined core default/movable suites pass
1,898 tests each, external-allocation passes 1,951, and Embassy default/log pass
115 plus two doc tests each; strict Clippy and formatting pass. Only T-Echo S140
v7's resource gate was rerun here, not the full 14-profile matrix or Nordic
hardware qualification.

Local PR drafts/report in `scratch/prns-app/` have the updated branches,
maintainer-facing motivation and publication caveats. Exact commands, range-diffs
and reviews are archived in the sibling `ios-refresh-logs/` and
`nordic-refresh-logs/` private evidence directories. Published-ref replacement
still requires an explicit old-tip/rescue/lease plan and approval.
