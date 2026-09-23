# Retire AccessorySetupKit follow-up work

This follows the [ordinary CoreBluetooth migration and phone trial](2026-09-23-ordinary-corebluetooth.md).
The user approved cleanup, including retiring the central-only PR, and separately
confirmed the exact scan-branch rewrite, rescue ref and publication lease. The app
branch remains local; no new phone installation is part of this checkpoint.

## Upstream checked before publication

Fresh upstream/origin fetches and live PR inspection confirmed trunk
`8c211827b014bf7ef06112b383719c343432fe53`. Maintainer updates were already present
on #202 (`d3a112af5`), #207 (`7bcbbf4e0`), #218 (`6b15b127c`) and #223 (`8d7ea459b`).
#208 and #209 remained open at their previously published tips. Earlier merged
work, including #198, #213, #216, #221 and #224, is already in trunk; it is not
replayed or reopened. Unrelated PR branches are left unchanged.

The new #209 candidate starts at the maintainer's exact #207 tip, not the stale
local branch. Its sole own commit is an unchanged port of the scan diagnostics
and startup-order test: three files, 153 insertions / one deletion, identical
patch ID `900e8063fd6d26810ca48c962b7b31ded05ff515`. It contains no central-only API.

## App branch changes

- `7c3fc1e4c` removes the unused central-only public APIs, identifier, backend
  variants and role-specific branches. Generic closed-channel protection, radio
  reconciliation, scan diagnostics and restoration remain. Net deletion: 397 lines.
- `53f6f2717` imports maintainer commit `700856198` with authorship/provenance:
  small control-inbox errors and updated restoration test greetings.
- `40ced6bbc` imports the remaining batching-fixture changes from maintainer
  commit `f7070cde9`. Conflicts retain the newer restoration-aware enqueue path
  and existing test imports. Application compatibility now pins this core state.
- Debug diagnostics recognize actual dual-role readiness and timeout messages as
  `bluetooth_managers_ready` / `bluetooth_managers_timeout`. The dynamic PSM is
  never exported. Tests reject obsolete central-only codes and appended payloads.
- The pre-existing, untracked SDK implementation plan now names ordinary Bluetooth
  authorization rather than accessory setup. It remains untracked; its unrelated
  contents are not swept into this cleanup commit.

Independent review found no lost dual-role behavior. The retained peripheral
implementation and recovery/batching tests match current #223; the central
implementation differs only in method placement. App-owned restoration IDs,
process lifetime, protected-data recovery, Stop/reset guards and anti-ASK metadata
tests remain. Historical ASK checkpoints retain their original evidence scope.

## Checks

- Application native: 190 unit tests and four integration tests passed.
- SDK: 59 tests, typechecks, formatting, lint and iOS source/metadata checks passed.
- Swift diagnostic allowlist, authorization, dispatch/recovery and release-symbol
  checks passed. Generated bindings and compatibility checks passed.
- Integrated core: 108 FFI tests passed with and without logging, one hardware
  test ignored per run; 16 Bluetooth-interface tests passed. Strict Clippy passed
  for both crates with and without logging. Both crates also passed iOS
  cross-compilation in both modes; the application passed iOS compilation with
  its restoration probe enabled.
- Scan candidate: 77 FFI tests passed with and without logging, one hardware test
  ignored per run; 15 Bluetooth-interface tests passed. Both crates passed strict
  Clippy, iOS compilation and formatting. Publishing preflight passed notices,
  14 baseline contracts and the unsafe inventory without source changes.

The first shell-linker attempts failed because Nix `cc` could not resolve iconv
after removing `LIBRARY_PATH`. Explicit Apple compiler/SDK settings fixed the
toolchain issue; no source workaround was added. A first backup push from the
dirty app checkout failed its notice-input gate before any remote update; the
clean scan candidate passes that gate. Publishing must use the clean candidate
and the normal hook, not bypass the failure.

No new binary was installed. The earlier physical proof remains evidence for
`f606a3f5c`, not automatic acceptance of this cleanup build.

## Publication

- #223 description updated and read back at unchanged head `8d7ea459b`: ordinary
  dual-role app rationale, actual #218 dependency, corrected comparison link,
  and explicit remaining peripheral-role hardware qualification.
- #209 candidate: `5bd8dc799c4c613676db1615a409afddb2fe3b09`, based on
  `7bcbbf4e0e001aeaab998e171c4eb6eba1990163`.
- Old #209 tip: `a20dcfd15b09f50a0d83fc66caf82cfc3fc3dd4d`, preserved locally as
  `rescue/prns-app-corebluetooth-scan-startup-pre-ask-removal-20260923`.
- Atomic rescue/#209 publication is undergoing the normal publishing gate.
  Exact force-with-lease applies only to `prns-app-corebluetooth-scan-startup`
  at the old tip above. Close #208 only after the dependent branch is published;
  retain its branch. Update this section with the confirmed result.
