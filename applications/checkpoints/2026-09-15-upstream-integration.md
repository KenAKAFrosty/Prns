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

These are source and host checks, not phone Bluetooth or lifecycle acceptance.
Android packaging, firmware resource measurements and device journeys are the
next qualification stage. Historical firmware margins and earlier phone results
do not qualify this integration.

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
Remeasure the integrated firmware rather than transferring either upstream's
or the previous app's flash margin.

The embedded readiness check found missing pinned Miri/ISA emulator prerequisites
and a local ESP tool executable linked to a removed library. Resource-tool
repair is separate from full assurance readiness; no gate is waived.

Only the main source checkout is needed. Build caches and firmware artifacts
use a bounded external build root; archived historical working-directory paths
are not current build locations. Device evidence must record the installed APK
and firmware hashes, exact source, failures, and recovery boundaries.
