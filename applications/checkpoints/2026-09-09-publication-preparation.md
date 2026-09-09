# Publication preparation and peripheral smoke — September 9, 2026

Continuation of the [atomic write-batch correction](2026-09-09-corebluetooth-write-batches.md).
This separates three different checks: the actual Mac radio, refreshed source
contributions, and the still-unqualified multi-request ATT callback. No published
branch or PR has been changed.

## Physical scope

Read-only USB inventory confirmed MetalbeardMobile
(`00008120-001E25C63E60C01E`) and the known Espressif board
(`AC:A7:04:E1:3F:88`, `/dev/cu.usbmodem101`). ADB listed no Android device.
No phone app was rebuilt/reinstalled, no accessory authorization changed, and
the board was not opened over serial, flashed or rebooted.

The exact existing ignored test
`bluetooth_auto::macos::tests::the_node_publishes_then_accepts_explicit_radio_modes`
passed against the real Mac radio on app source
`b9e878588cc80e2678483b5258cb9de917de9a39`: one passed, 106 filtered out,
0.06 seconds. This covers manager/listener startup, advertising/scanning
requests, logical radio off/on and owner cleanup. It does not dial a peer,
receive input, or assert that the write callback ran. It is not physical
write-batch qualification.

The installed iPhone app remains central-only and cannot qualify the shared
peripheral callback. The ordinary `bluetooth_node` example was not run: its Auto
supervisor selects peers automatically and does not provide a controlled ATT
batch stimulus.

Private evidence and the exact command:
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/ios-peripheral-smoke.uBLMG5/`.

## Next controlled peripheral test

Use the Mac as the peripheral and a disposable Android central fixture when the
Galaxy returns. This does not require changing the app's role, adding an iPhone
accessory grant, or flashing the board.

1. A bounded Mac harness should consume the public backend/link APIs, advertise
   only for the test, disable scanning, and omit the Auto supervisor. Select the
   Mac on the Android side using an explicitly verified test identity; the
   generic advertisement name `Prns` is not sufficient identification.
2. Establish a successful ordinary write before trying Android's reliable-write
   transaction APIs. Queue and verify each write, then execute; abort on any
   queue/verification error. These APIs are available on the Android 10 target.
3. Observe the actual Mac callback using temporary, payload-free diagnostics:
   callback sequence, request count, target categories, offsets/lengths, final
   result and response count. Record published input/link admission separately.
4. Test a valid batch and a valid first write followed by malformed control
   input. A rejected batch must publish no earlier input or new link. A later
   valid transaction must still work.

Require a recorded callback with **more than one request** before claiming batch
coverage. Android transaction success alone does not prove how CoreBluetooth
delivered it; the OS may reject some invalid writes before the delegate runs.
Public iPhone writes also do not provide a documented way to force callback
grouping. The deterministic production-planner tests remain the evidence for
rollback, capacity and receiver-teardown races until a matching physical case
is actually observed.

Primary API references: [Android BluetoothGatt](https://developer.android.com/reference/kotlin/android/bluetooth/BluetoothGatt),
[Apple characteristic writes](https://developer.apple.com/documentation/corebluetooth/cbperipheral/writevalue%28_%3Afor%3Atype%3A%29),
and [Apple's batch callback contract](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanagerdelegate/peripheralmanager%28_%3Adidreceivewrite%3A%29?language=objc).

## Refreshed contributions and CI comparison

Upstream was rechecked at `1d4d4ba8652875ee6fda835c633398d1bab3419a`.
New local branches preserve every original/published head:

| Contribution | New local branch | Exact tip |
| --- | --- | --- |
| #199 USB test fixtures | `codex/refresh-usb-auto-wake-tests` | `a0ba517957fc4a027ed625d14ef7bc30208f965a` |
| #208 central-only role, on refreshed #207 | `codex/refresh-ios-role-policy` | `243a153d5ce635aa73625ae3dece057f6c59a394` |
| #209 scan-startup logging, on refreshed #208 | `codex/refresh-ios-scan-startup` | `0615cb39810de62c61babcc27670f3cb1d97fd79` |

All five original patches match their refreshed versions exactly in range-diff;
there were no source conflicts. Neither iOS descendant changes the canonical
unsafe inventory: both actual checks pass with the same 703 package records and
FFI counts of 138 blocks / 199 tokens as their refreshed #207 parent.

- #208: FFI default/logging each 77 passed, one hardware test ignored; 14 focused
  Bluetooth-interface tests passed.
- #209: FFI default/logging each 78 passed, one hardware test ignored; 15 focused
  Bluetooth-interface tests passed.
- Both: strict FFI and owning-interface lint checks, iOS FFI/interface/public
  export compilation, formatting and canonical inventory checks passed.
- #199: all-feature/all-target host tests passed (280 unit, one integration),
  as did 17 focused USB tests, strict Clippy, formatting and iOS compilation.
  Its first iOS invocation failed in the host's Nix compiler wrapper; selecting
  Xcode's compiler for the iOS target passed without a source workaround.

The unchanged iOS descendant patches do not include the later recovery or batch
fix. They remain siblings of that follow-up stack, not newly qualified physical
implementations of it.

### What #199 fixes, and what it does not

Current trunk and the write-batch tip both reproduce two USB fixture type errors
when compiling every Tokio interface feature/target. Live macOS/BLE logs for
#202 and #207 show the same errors. The fixtures still pass the old per-interface
notification channel instead of the shared wake sender.

An isolated test composition, `codex/qualify-ios-refresh` at
`3b9763d5a2451267d2834ec9fe716e278b2b43f2`, adds only the exact #199 patch to
batch tip `d916095d5bea37d450980a182aa534f8131ab540`. The identical all-feature/
all-target check changes from exit 101 to exit 0, and strict Clippy passes.
This establishes removal of that compilation blocker without adding USB changes
to any Bluetooth PR head. The composition is not a publication candidate or a
full test-matrix result; #208/#209 are not part of it.

The existing #199 CI run has two separate failures: a split-response integration
test returned `Failed(LinkClosed)` on Linux, and generated third-party notices
drifted. Its dependency-policy and unsafe-snapshot checks passed. The exact
split-response test passed once locally on macOS; that neither reproduces nor
resolves the Linux failure. No notice, workflow or runtime fix was folded into
the narrow USB contribution, and no complete remote CI rerun was claimed.

Private evidence:

- `ios-dependents-logs/refresh-and-publication-plan.md`
- `usb-auto-refresh.WG3TKC/report.md`
- `ios-peripheral-smoke.uBLMG5/` (radio and before/after composition checks)

All are under `/Volumes/wavlink/dev/prns-app-acceptance-20260909/`.

## Publication boundary

The local PR report now has an exact five-head replacement proposal with current
bases, old/new SHAs, proposed rescue refs, explicit leases and concise revised
PR bodies. The existing GitHub bases stay `trunk`; own-diff links describe the
contribution stack. The app branch and new recovery/batch/Nordic PRs are outside
that replacement scope. Do not bypass hooks or treat an earlier publication
approval as permission for this shared-history update.

Next: address the separately recorded full-CI limits, obtain approval for the
exact protected update plan, and perform controlled peripheral batching when the
Android test device returns. No push or PR creation/edit occurred in this pass.
