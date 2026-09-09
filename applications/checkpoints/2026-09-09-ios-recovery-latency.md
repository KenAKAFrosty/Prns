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
| 18:20:28.978 | Session reaped locally, with no Welcome received |
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

## Next correction

The isolated `codex/fix-restored-native-bluetooth-session` branch is being prepared
on `prns-app-corebluetooth-restored-session`, not yet validated or ready to publish.
The proposed scope is one reconnect of an initially connected, restored iOS
native-PRNS peer inside its existing bounded dial. It must discard old-session
input, retain one completion/owner/deadline, and prevent Stop, timeout or duplicate
callbacks from resurrecting a connection. Columba, macOS and other connection
states must remain unchanged. It is not a protocol-resumption mechanism or a
license to bypass another application's connection ownership.

No transport fix is part of the diagnostic build above. Neither the original
ordinary-start failure nor the 32-second LXMF delay has a demonstrated fix.
