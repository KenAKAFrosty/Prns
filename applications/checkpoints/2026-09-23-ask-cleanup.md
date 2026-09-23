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

## Publication paused for new upstream merges

- #223 description updated and read back at unchanged head `8d7ea459b`: ordinary
  dual-role app rationale, actual #218 dependency, corrected comparison link,
  and explicit remaining peripheral-role hardware qualification.
- #209 candidate: `5bd8dc799c4c613676db1615a409afddb2fe3b09`, based on
  `7bcbbf4e0e001aeaab998e171c4eb6eba1990163`.
- Old #209 tip: `a20dcfd15b09f50a0d83fc66caf82cfc3fc3dd4d`, preserved locally as
  `rescue/prns-app-corebluetooth-scan-startup-pre-ask-removal-20260923`.
- The atomic rescue/#209 push was stopped during its normal publishing gate
  when the user reported a new upstream merge batch. No remote ref was updated:
  #209 still points at the old tip above, and the remote rescue ref is absent.
- Fresh upstream trunk is now `45d477e7bedb98fe86b665c904bda077061142e9`.
  The new merges are #199, #217, #219, #220, #225, #222, #215 and #202.
  #207, #208, #209, #218 and #223 remained open at their previously inspected
  heads. The landed #202 Apple implementation matches its former PR head;
  it does not invalidate the ordinary dual-role cleanup.
- An intermediate, unpublished scan candidate `9b588ff7121e5a1f55adedcdbe64a70498aede4f`
  was validated on merge base `ec9a95808a734f1534b4bdef339e23d5dfbf94de`
  (trunk `ca2c907af` plus #207 `7bcbbf4e0`). It retains the identical own patch
  and passes the focused checks above. This is **not** a candidate based on the
  subsequent #215/#202 merges and is not ready for publication as-is.
- The publishing sweep's desktop compilation failure was environmental:
  missing `pkg-config` and SDL2 development metadata. Restoring the matching
  Nix SDL2 development output and supplying `PKG_CONFIG` / `PKG_CONFIG_PATH`
  made the normal desktop workspace check pass without source changes.
  The complete publishing gate has not finished.
- The app branch has not yet incorporated this merge batch. Its checks above
  qualify the recorded cleanup commits, not integration with the new trunk.
  The latest trunk also contains refinements to preserve:

  - #222 adds `StreamDeframer::pending_frame_len()` and rejects an oversized
    L2CAP frame from its length prefix, before waiting for the full frame.
    The app branch still uses the earlier post-extraction check.
  - #220/#225 update the separate Personal Hopspot Android implementation.
    The new app's SDK already has equivalent legacy-notification and startup
    sequencing protections; do not replace that implementation unnecessarily.
  - #215's runtime diagnostics match the app implementation. Preserve the
    app branch's additional pairing expiry/persistence tests during integration.
  - #217/#219 touched files already match the app branch. #199 only removes an
    obsolete wake-receiver fixture in addition to the behavior already present.
- Final local scan candidate: `67f526e0aba4e602ebe46c0d37bac0cbe919dcb2`, based on
  `eb426538eeda4971e79590c6ed3fd6527772b650` (trunk `45d477e7b` plus current #207).
  Squash-history conflicts preserve #207's bounded ingress/restoration handling.
  Independent review confirms the Apple implementation matches the previously
  validated candidate, public APIs/imports occur once, and the original scan
  patch is unchanged. Fresh checks passed: 77 FFI tests in each logging mode
  (one hardware test ignored each), 15 Bluetooth-interface tests, strict Clippy
  and iOS compilation for both crates, formatting, notices and diff checks.
  The full publishing gate remains pending. The final candidate is frozen.
- An approval question briefly named provisional commits `62d4003b9` /
  `0163921bd` before final merge review removed a duplicate import. That
  question was explicitly withdrawn; it does not authorize publishing a
  different candidate. Revised approval must name the final commits above.
- Next: confirm the revised rewrite base, rerun the normal publishing gate, and use
  an exact lease on the old #209 tip. Close #208 only after #209 no longer
  depends on it remotely; retain #208's branch. The app branch remains unpushed.
