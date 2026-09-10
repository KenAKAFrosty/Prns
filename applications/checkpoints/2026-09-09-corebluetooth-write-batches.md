# CoreBluetooth write-batch correction — September 9, 2026

This closes the source issue found during the
[upstream refresh](2026-09-09-offline-and-upstream-refresh.md#ios-stack).
It does not add physical peripheral-role qualification or update any published
PR head.

## Behavior and ownership

CoreBluetooth requires one response to the first request in each ATT write
callback, and no writes fulfilled when any member of that batch is refused.
The former per-request loop could publish earlier input before a later failure.
The requirement is documented in [Apple's callback contract](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanagerdelegate/peripheralmanager%28_%3Adidreceivewrite%3A%29?language=objc).

The shared Apple peripheral implementation now stages Native and Columba writes,
reserves all control/inbound slots and data-byte capacity, then publishes with no
remaining fallible admission decisions. New sessions and links stay private until
the whole batch is admitted. Failed staging refunds reservations and does not
alter unrelated owners; committed new links already have their initial input.

This uses the existing queues and budgets. Receiver closure after reservation is
teardown of accepted work, not a late partial rejection. Unsupported UUIDs,
nonzero offsets, absent values and malformed control/initial identity values are
rejected. Existing data-fragment admission, including charged empty values, is
preserved. The generated API, app admission policy and process-owned runtime are
unchanged. Tests use the production planner with a plain central context rather
than a second implementation or real Objective-C Bluetooth objects.

## Commits and integration

| Scope | Revision |
| --- | --- |
| Upstream code fix | `b70047e53049da588dd6d9da6af7b67a6d39e280` |
| Upstream branch tip, including snapshot-only update | `codex/fix-corebluetooth-write-batches` at `d916095d5bea37d450980a182aa534f8131ab540` |
| Local parent | `codex/refresh-ios-native-recovery` at `a2a75d13d40d319ca3fe2a54ce22d4253153c9df` |
| App code integration | `aaac4c991ea3ea363609a6e5c21eb6323b3ca217` |
| Audited app core pin, including snapshot-only update | `90b649e4f65d7426ac4a590db77b1f6a90a20173` |

Only the new code fix was cherry-picked into the app. The one conflict was a test
import: the app's central-only restoration type was preserved. Range-diff shows
only that context difference; production planner/peripheral code and new tests
match exactly. Existing central-only GATT composition and its regression remain.

The logical source dependency is the restored-session contribution. The current
local stack also contains central recovery, but this batch fix does not change
that algorithm. A direct-to-trunk port needs an explicit adaptation to cover the
restoration parent's additional refusal/admission paths. Upstream was rechecked
at `1d4d4ba8652875ee6fda835c633398d1bab3419a`; no published ref changed.

## Validation

- 23 new regressions: 16 batch-admission tests and seven data-reservation tests.
  They cover one response, late-failure rollback, budget/queue exhaustion, closed
  receivers before and after reservation, Native/Columba ordering, exact peer
  ownership, malformed inputs and refund accounting.
- An intentional early-control-publication mutation made the late-decode-error
  rollback test fail because a Welcome escaped. Restoring staged publication
  returned the full suite to green; no mutation remains in committed code.
- Isolated FFI default and logging suites: 100 passed, one hardware test ignored
  each. Strict all-target/all-feature Clippy, formatting and the all-feature iOS
  cross-check pass.
- The isolated canonical unsafe snapshot/check pass. Only FFI block/token counts
  change from 140/201 to 132/193; all 703 packages, graph/features and policies
  remain unchanged. Fewer lexical blocks reflect the consolidated adapter and
  response path, not a proof of memory safety.
- Integrated native verification passes: generated-output/provenance and
  compatibility checks, 136 native unit tests, four integration tests and
  49 SDK tests. The explicit iOS native-composition cross-check also passes.
- Integrated FFI default/logging suites each pass 106 tests with one hardware
  test ignored; strict Clippy and the iOS all-feature FFI check pass. The app's
  canonical inventory generation/check also pass: only FFI block/token counts
  change from its older 138/199 baseline to 132/193, with all 704 packages and
  every other graph, feature and policy field unchanged.
- After selecting the snapshot-inclusive core pin, compatibility and generated
  output checks pass again; no Rust source changed between that pin and the
  integrated test runs.

Private commands/results are archived under
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/ios-write-batches.hzqSVJ/`.
Build and generator caches were reused on the external drive. No new dependency,
binding output, UI, firmware or platform configuration change was required.

## Remaining boundaries

No phone or board was rebuilt, installed, flashed or rebooted in this pass. The
installed iOS framework remains `d4938e521b93f02317330f281feb34cf23e82c7640d14dc898d34bf4b374e191`
from native build `31dbe75e0` / core `3e61d5ac5`; the last tested JavaScript is
`c230870e6`. Earlier phone observations stay attached to those versions, not a
new binary built from the updated core pin. The app's central-only phone results
cannot qualify the shared peripheral/server callback.

Peripheral hardware batching, the broader publication matrix and the existing
#199 CI dependency remain separate. Published #208/#209 descendants are unchanged.
The local PR draft/report now includes this separate correction; publishing or
rewriting shared heads still needs the exact approved update plan. No push or
PR creation/edit occurred.
