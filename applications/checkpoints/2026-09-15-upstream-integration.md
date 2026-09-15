# Upstream integration and mobile acceptance — September 15

## Integrated source

The application integrates upstream trunk `35859bb89632c56bdd9c33e462683ad29e345f04`
in merge commit `a751b1cdaf84d429cb1d86938ae998d006e1b953`. This preserves the
published app history and incorporates 65 upstream commits since the previous
publication baseline. The compatibility record now selects this integrated core.

The CoreBluetooth merge combines stale-system-link recovery with the app's
queue-confined registry and explicit restoration ownership. Restored connections
are not mistaken for orphan connections; closed data owners retire without
cancelling a still-live opposite role. Upstream's drop-before-close notification
ordering is retained. Focused tests cover the combined admission and retirement
cases, including a restoration reset that is already cancelling its connection.

The app's Android DATA write policy already matches upstream's restored
write-without-response behavior; no parallel transport policy change was needed.
The app lockfile gains the upstream FFI crate's `libc` dependency. Canonical
notices and unsafe inventory were regenerated without restoring older dependency
versions.

## Completed checks

- CoreBluetooth macOS unit selection: 82 passed, one hardware test ignored.
- Strict macOS library check and Clippy; strict iOS-target library check.
- USB Auto unit selection: 11 passed.
- Tool and validation registry checks; 102 selected tooling/registry tests.
- App UI tests: 221 passed; TypeScript checks passed.
- SDK JavaScript tests: 49 passed.
- Native application tests: 136 unit and four integration tests passed.
- Swift lifecycle, protected-data, start-dispatch, restoration-diagnostic and
  release-symbol selections passed on the host.

Full `applications` verification passes at `a122dd21c`, including generated
bindings/provenance, portable native, SDK/UI, configuration, Expo Doctor 21/21
and web export. The first run stopped at Expo's newly updated patch requirements;
the six compatible SDK 57 patches and their required transitive updates were
committed separately. React Native, API 29 and the generated binding boundary are
unchanged. These source/host checks do not establish phone lifecycle acceptance.

Detached extraction also passes at `8e6ae75fa`, selecting exact core `a751b1cda`
from the local Git repository. It includes the full detached app checks and the
proven native/Python LXMF exchange. The generated JavaScript artifact SHA-256 is
`0b292b27e05d5c3f4e6f0d0861c1ef159059c62116ac983105f055eee5a71296`.
This checks extraction against exact local source, not remote pin availability.
The isolated checkout and disposable compilation/test caches were removed afterward.

## Android device builds

The current installed APK is the Expo-patched second build below. Results for
the initial APK are retained separately to make the retests explicit.

### Initial integrated APK

The bundled, standalone arm64 APK built successfully for Android API 29, passed
its JVM and 16 KiB native-library alignment checks, and was installed on the
Galaxy S9+ running Android 10. It requires no Metro connection. APK SHA-256:
`78e630beae02f764595eab7c5671572f2b3494d53e42d2bf320f7e24f61f7cd5`.
Its native core is the integrated `a751b1cda`; the subsequent `1a4dbfe54`
compatibility pin/documentation commit does not change compiled application code.

Before updating the board, a retained-grant Check failed against its older
`6800e43ab2ac` firmware despite an established Bluetooth connection. This is
recorded as a failed mixed-version trial, not evidence of successful retained
pairing or proof of a regression in the new build.

The disposable application data was then reset through its development UI.
Pristine interactive identity import passed these physical checks:

- Cancelling the picker leaves import unavailable and setup unchanged.
- A malformed 13-byte file is rejected before confirmation.
- A public 64-byte test identity previews and imports with the independently
  calculated identity hash `5235c813b0219ff915bc86f8725b51f4`.
- After a force-stop and cold launch, the local node runs with that same primary
  identity. The identity is deliberately public test material, not user key data.

On this first APK, a full physical two-way exchange through E290 passed against
the pinned Python LXMF peer. Python verified the app's message, and the phone
stored the incoming verified message and outgoing delivery confirmation (378 ms).
A subsequent message to the stopped peer failed as expected. With Bluetooth off
and after a cold launch, the saved conversation remained visible; Retry followed
by Cancel changed the same record to Cancelled. That state survived another cold
launch. The observed intermediate state was Sending, not a captured Queued state.

### Expo-patched APK

The Expo patch update produced a second standalone APK, installed without clearing
data. Its SHA-256 is
`8c221aa11cb86da89b6b1d99bb225ffeef748d438ee8d09e7371d8547833aa14`;
source changes are committed in `a122dd21c`. The new APK retained the three saved
messages while Bluetooth was off, including the cancelled record. Turning
Bluetooth on restored the E290 connection, and a second complete Python exchange
passed with phone delivery confirmation in 195 ms. The old cancelled message was
not received by the peer during that bounded exchange. This is not indefinite
no-resend or general upgrade/migration qualification.

Pristine import was then repeated on the patched APK after resetting only the
disposable test data: picker cancellation, malformed-file rejection, correct
preview/import, and same-primary-identity retention after cold restart all pass.
The new test controller identity is `68f67abd65b0fdb389359c71ee672fe6`.
The earlier test messages were removed by this explicit reset; their recorded
evidence is retained, not claimed to remain on the phone.

On that patched APK, Stop remained stopped after a Home/foreground round trip;
manual Start restored the connection. Three further Settings Bluetooth off/on
cycles each produced a fresh E290 data/control subscription. The first automation
attempt stopped safely while the Settings switch was still transitioning; it is
not counted as a completed cycle. A bounded transition wait enabled the three
recorded passes without changing app code or Android settings policy.

After these cycles, the phone received and proved an incoming message while the
Samsung launcher remained the resumed activity before and after proof. Returning
to the app showed the stored verified message, and an outgoing reply completed
the third two-way exchange (193 ms reported delivery). This is one controlled
off-screen foreground-service observation on a USB-powered phone, not continuous
lock, deep Doze, natural suspension or guaranteed background delivery.

Finally, the cold Bluetooth-off Retry/Cancel journey was repeated on the patched
APK. The saved failed message `c3fbb0b39e4b637d7b984cf5d1aa42fb49c53b248368dd435e9f11d89a8505d8`
remained visible after cold launch, Retry entered Sending, Cancel completed, and
the identical record retained its Cancelled state after another cold launch.
Bluetooth was restored afterward. This closes the recorded Android offline-action
gap on this exact APK; it is not caller cancellation of an authenticated node read,
power-loss durability or a claim that a queued message was never transmitted.

Fresh pairing, authenticated requests and controlled caller cancellation remain
open; Bluetooth subscription is not remote-control success. The app is left on
Pair a node, awaiting the board's user-opened invitation window.
No physical iOS result is claimed for this integration.

## Upstream contribution follow-up

Upstream now contains the original compilation correction from
[#199](https://github.com/KenAKAFrosty/Prns/pull/199), with a different test wait,
and all four notice additions from
[#214](https://github.com/KenAKAFrosty/Prns/pull/214). Reassess those contributions
as superseded rather than replaying stale metadata. The app retains its bounded,
event-driven USB test wait.

The iOS PR stack needs reconciliation with upstream stale-link recovery; Host
snapshot and pairing-window branches also have reported merge conflicts.
No shared PR head was changed by this local app integration. Remaining PRs need
their own source and publishing checks before updates; an app test is not
qualification of an isolated PR branch.

## Firmware and platform limits

Upstream adds a third Hopspot destination and new embedded assurance gates.
T-Echo retains its configured fat LTO; thin LTO is specific to MeshTower V2.
Five canonical memory contracts and two freshly built resource profiles passed:

| Target | Application image | FLASH headroom | Static-RAM headroom |
| --- | ---: | ---: | ---: |
| T-Echo S140 v7 | 622,264 bytes | **328 bytes** | 4,032 bytes |
| E290 | 2,202,944 bytes | 12,923,584 bytes | 47,860 bytes |

These measurements use source `1a4dbfe54`, stable Rust 1.98.1, and working-tree
custody: unrelated untracked files were preserved. They are not clean-commit
assurance-baseline evidence or a fresh result for all 14 configured profiles.
The small T-Echo margin remains a release risk; do not attribute its difference
from earlier measurements solely to upstream source changes.

The E290 was rebuilt and flashed through the canonical local-flash path, with
all three sparse parts verified and saved Wi-Fi/NVS settings preserved. Runtime
logging confirms boot source `1a4dbfe54e50`. Flashed application SHA-256:
`9c55cdd5f64f47eb004a3459293c1da7d25536ed1fff19c798970a907902d2f1`.
The resource and flashed images have identical code/data and section sizes;
four build-timestamp bytes differ, so their distinct hashes are retained.

The missing pinned Miri/ISA tools and broken local ESP tool dependency were
repaired in the isolated external build root, without changing global tools or
waiving gates. Miri quick passed 35 tests (stacked borrows); the Thumbv7em and
RISC-V quick suites passed two shared-state-machine scenarios each with matching
transcripts. Pins are Rust 1.96.0, nightly-2025-11-21 and QEMU 11.1.1. These runs
captured working-tree custody at `1a4dbfe54`, with different working-tree diff
fingerprints; they are development evidence, not one clean publication baseline.
The full normal publishing selection and other emulator/platform scopes remain
separate.

Local raw logs, copied resource reports, the flash receipt, identity-import UI
captures and hashes are kept under `scratch/prns-app/2026-09-15/`. An unsuccessful
UI dump reused an old Android XML file; that capture is explicitly marked
`invalid-stale` and excluded from acceptance. The helper now rejects failed fresh
dumps, and a current screenshot verifies the retained identity instead.

Only the main source checkout is needed. Build caches and firmware artifacts
use a bounded external build root; archived historical working-directory paths
are not current build locations. Device evidence must record the installed APK
and firmware hashes, exact source, failures, and recovery boundaries.
