# iOS recovery investigation — September 9, 2026

This follows the [generated-binding checkpoint](2026-09-09-follow-up.md).
It separates a reproduced Bluetooth restoration stall from the still-unexplained
ordinary-start nonconnection and earlier 32.024-second message delay.

## Reproduction and diagnostics

Three incoming-message comparisons on the previously installed framework
completed in **587, 429 and 385 ms** from submission to proof. Each used its
first Link attempt. The earlier long message delay did not reproduce. These
active-traffic, one-way trials are not repeated quiet-background acceptance.

Two network log captures exposed an internal reader failing and exiting while
the logger command remained running. A connected header therefore does not
establish an intact timeline. This explains those specific missing streams,
not every earlier capture gap. After a cable change made the phone available
over USB, two independent USB readers captured the complete restoration sequence
below and continued receiving until deliberately stopped.

App commit `3621b7f7c` adds three payload-free debug markers from existing FFI
log sites: Hello write completion, Welcome reception and local closed-session
reaping. No protocol, retry, timing, ownership or storage behavior changed.
Eight Rust probe tests, Swift platform/privacy/Release checks, iOS static checks
and the physical-device build passed. This is not a repeat of the full portable
or detached suite. Generated API output was unchanged.

The new iPhone 14 Pro / iOS 26.6.1 Debug framework has SHA-256
`8b31d51428bdfd7d73bead7f7aabf3c866b27d1e94728d8e61623539957be8ab`.
Core remains the footprint-corrected `7c9dcb2ed`. Installation retained data;
there was no board operation, fresh pairing or reset. Metro stayed available.
Foreground activation after installation reached the retained node, but its
native startup was not logged. No pre-activation process inventory was taken;
the process may already have started before capture. Its launch source is unknown.

## Captured restored-handshake stall

After developer SIGTERM of verified PID 10942, PID 10947 launched with a matching
CoreBluetooth restoration request. Both USB readers captured preparation, native
startup, restored-peer adoption and the same ordered diagnostic events:

| UTC | Event |
| --- | --- |
| 18:20:18.974 | Restored control subscribed |
| 18:20:19.094 | Hello write acknowledged |
| 18:20:28.978 | Session reaped locally; no Welcome marker observed |
| 18:20:28.981 | Disconnected |
| 18:20:33.415 | Fresh connection subscribed |
| 18:20:33.505 | Fresh Hello write acknowledged |
| 18:20:33.509 | Welcome received |

The approximately ten-second interval matches the shared handshake deadline.
The observed failure is a stalled restored handshake followed by local cleanup
and a successful fresh handshake—not seamless session continuation. Welcome
reception alone is not handshake validation, and the probe does not emit a typed
timeout reason. Source review indicates that a board retaining an already-settled
session no longer has a handshake receiver to answer another Hello; its
uninterrupted physical session state was not captured here.

A subsequent incoming message completed in **474 ms**, first Link attempt.
After stopping the fixture, non-terminating foreground activation retained PID
10947; its first Check completed in **383 ms**. Inbox showed eight records.
All four incoming messages from this investigation were individually verified
as Received / Verified source with their exact hashes, including the three
from before the diagnostic reinstall. Transport proof and mailbox storage are
separate observations; the displayed message time is not a measured receive time.

The fixtures were deliberately one-way and stopped after proof, not complete
two-way fixture passes. No new agreed no-touch window, natural suspension,
continuous lock, protected-data state, Metro-off execution or power-loss
durability is established. The earlier generated-binding pairing and broader
lifecycle qualification gaps remain open.

## Bounded correction

The isolated `codex/fix-restored-native-bluetooth-session` correction is committed
as `7cad8187c`, based on `prns-app-corebluetooth-restored-session` at `3af6e7c6a`.
It reconnects an initially connected, restored iOS native-PRNS peer once inside
its existing bounded dial. It discards old-session input, retains one completion,
owner and deadline, and guards against Stop, timeout or duplicate callbacks
restarting recovery. Columba, macOS and other connection states are unchanged.
This is not protocol resumption or a guarantee that another application's
physical connection closes. Phase checks cannot establish arbitrary late
CoreBluetooth callback-generation identity.

Independent review found no actionable issues. Isolated validation passed:
78 FFI tests, including six recovery regressions; one hardware test ignored;
strict all-target Clippy, iOS cross-check, formatting and diff checks.

The app already contained equivalent parent changes through a different history.
An attempted merge was aborted, and only the correction was cherry-picked as
`3e61d5ac5`, with the exact same stable patch ID
`b318ebf4b75d5817d12d197e1c06a769ca25252e`. App commit `31dbe75e0` pins that core.
Integrated FFI validation passed 84 tests with one hardware test ignored and
strict all-target Clippy. The app native gate passed 136 unit tests, four
integration tests, 49 SDK tests, generated-output and provenance checks. Swift
startup, diagnostics/privacy and Release checks also passed.

The app gate also passed formatting, lint, TypeScript, routes/configuration,
dependency checks, all 21 Expo Doctor checks, 215 UI tests and web export.
The first Doctor attempts selected an unusable RVM CocoaPods executable linked
to missing OpenSSL 1.1. Selecting the existing `/usr/local/bin/pod` 1.15.2 on PATH
passed without dependency or source changes. The device helper had already
selected that working executable through its fallback. This is not a fresh
detached-consumer or complete repository/firmware gate run.

## Corrected physical comparison

The device build and installation passed on the same MetalbeardMobile, retaining
data. App source is `31dbe75e0`, core `3e61d5ac5`, and the Debug framework SHA-256 is
`d4938e521b93f02317330f281feb34cf23e82c7640d14dc898d34bf4b374e191`.
Generated API output was unchanged. Metro remained available; no board operation,
pairing reset or application-data reset was performed.

Both USB readers started before installation. PID 11034 launched with a matching
restoration request at 18:49:35 UTC, before the recorded process inventory and
operator activation. The legacy reader captured sequence 1–18 exactly once,
including reset request, disconnect, reconnect request, subscription and
Hello/Welcome. The unified reader omitted four markers on this launch despite
remaining active, so it is not a complete first-launch timeline. Its captured
Hello-to-Welcome interval was 2.364 ms. The first retained-node Check passed in
383 ms.

After moving the app to Home, developer SIGTERM of verified PID 11034 at
18:51:37 UTC caused a restoration-requested launch as PID 11038. Both readers
captured sequence 1–18 exactly once. Unified device timestamps show:

| UTC | Event |
| --- | --- |
| 18:51:37.526 | Native restoration launch |
| 18:51:37.693 | Reset requested, disconnected, then reconnect requested |
| 18:51:39.615 | Connected |
| 18:51:39.905 | Control subscribed |
| 18:51:40.026 | Hello write acknowledged |
| 18:51:40.088 | Welcome received |

Reset-to-Welcome was **2.395 seconds**, with **62.232 ms** between Hello and
Welcome. No repeated dial/reset/reconnect or local closed-session-reaping marker
was observed before capture ended. The previous unanswered-Hello stall did not
occur in either corrected restoration. These subsecond times come from the
unified device timestamps, not host log-ingestion times; legacy timestamps are
whole-second local time.

The subsequent incoming message was submitted at **18:52:33.414 UTC** and proved
at **18:52:33.899**, **485 ms**, first Link attempt. Its hash is
`d3303f390d9eb83f9f9954307939dc3b822b4fe3806fa2f7b93fee23ba604297`.
The operator did not foreground prns between SIGTERM and proof; Home was visible
again at the end. The user was asked to leave it unopened but did not separately
confirm a no-touch window. This is an operator-offscreen observation, not
independent proof of continuous inactivity, lock or natural suspension.

The one-way fixture and both readers were deliberately stopped before foreground
activation. The final reader exit follows that operator stop, not an unexplained
failure. Non-terminating activation retained PID 11038. Its first Check passed
in **354 ms**; Inbox showed nine records, and the exact incoming hash was verified
as Received / Verified source in the UI. Proof and mailbox storage remain
separate observations, not a measurement of durable receipt latency.

A separate foreground two-way fixture then passed with
`PINNED_PYTHON_LXMF_OK inbound=verified outbound=proof links=two` and exit 0.
Its incoming message completed in **378 ms**, first Link attempt; hash
`58d67cae52b9231969398c7bf1c937b3e9883702f1ccef9ba4270f716adb0eed`
was also checked as Received / Verified source. The phone's `Rust` /
`rust-to-python` reply showed Delivered in **389 ms**. The earlier one-way fixture
was deliberately stopped after proof with exit 130, not counted as a two-way pass.

Private logs are in `ios-latency.gVggHX` beneath the September 9 acceptance
directory: `ios-recovery-build.log`, `recovery-usb-{unified,legacy}.jsonl`,
`correction-*.json`, `lxmf-corrected-recovery.log`, and
`lxmf-corrected-two-way.log`. The UI checks were observed through iPhone Mirroring.

This establishes a bounded physical correction comparison, not fresh pairing,
quiet idle, protected-data recovery, Metro-off execution, OS reclamation,
App-Switcher force quit or broad lifecycle qualification. Neither the original
ordinary-start failure nor the earlier 32-second LXMF delay has a demonstrated
fix. Upstream remains `1d4d4ba86`; the published Bluetooth parent is eleven
commits behind and still needs an approved stack refresh before publication.
Nothing was pushed or opened.
