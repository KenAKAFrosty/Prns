# September 23 upstream refresh

This refresh keeps the current application work and the maintainer's changes,
using normal merges rather than rewriting shared PR history. It follows the
[ordinary Bluetooth cleanup](2026-09-23-ask-cleanup.md).

## Integration

- Integrated upstream `a6748cea4` in `785d70cc6`, then the newly landed pairing
  persistence, scan diagnostics and controller persistence-event changes through
  trunk `fe4966bab`. The pinned core source is `e2c10c808`.
- Adopted upstream's immediate rejection of oversized L2CAP length prefixes and
  the legacy Android Hopspot callback/startup corrections. Preserved the app's
  pending target-expiry event, ordinary dual-role Bluetooth cleanup and tests.
- Reconciled the previously published app ancestry in `798e947d7`, without
  changing source: all 22 published-only non-merge commits already had
  patch-equivalent counterparts in the local app history. The former published
  tip remains an ancestor, so publication can be a normal fast-forward.
- The later #205, #209 and #210 integration merges changed no app source. The
  app already contained those corrections. Discovery-store test isolation and
  pending event coverage were retained; no test was duplicated.
- After #211 landed, merged trunk `7f19df026` in `3646fd879`. Git's clean merge
  duplicated the target-expiry test because the maintainer had moved it beside
  another test. Removing that duplicate preserves exactly the qualified parent
  tree, with one of each test; no production behavior or pinned core code changed.
- Pinned the combined source in `release/compatibility.json` using the app
  fork's repository, and regenerated the notices with their canonical generator.
  The legal text is unchanged; only the reviewed-input fingerprint changed.

The remaining core contributions are separate PRs: shared Host snapshots (#201)
and E290 pairing (#212). The E290
refresh also contains the already-approved full-control preset and disclosure.
The obsolete central-only Bluetooth PR remains closed. No menu-stall diagnostics
or unrelated Wi-Fi correction was added to the E290 pairing PR.

## App compatibility and checks

The initial app verification passed native and SDK checks, then stopped because
Expo's compatibility check required newer patch versions. Commit `911145f0d`
updates Expo, Metro runtime, constants and router within SDK 57, including their
lockfile and explicit version checks. This is not a new phone binary.

The clean-export check also exposed an omitted compatibility entry: the existing
Wi-Fi workflow tests directly enable `prns-runtime`'s `remote-control-wifi-host`
feature. That test dependency is now recorded in the explicit direct-package
list; it was already present in the source-package list. No new production
dependency or resolver override was introduced. The generator's disposable
compiler cache was moved outside the app export tree before qualification.

Fresh complete app verification then passed:

- Generated bindings, compatibility and detached-checker tests.
- 190 native unit tests and four integration tests.
- 59 SDK tests and 325 app UI tests.
- Formatting, lint, typechecks, route/configuration checks, Expo compatibility,
  Expo Doctor (21/21), and web export.
- Separate Swift authorization, native dispatch/recovery, restoration diagnostic
  and release-symbol checks passed.

These app checks cover the source preserved by the ancestry-only upstream merges.
Canonical notices and the unsafe dependency inventory also passed.

The clean detached-consumer check subsequently passed for app `309bc2a21` and
core `e2c10c808`, including the complete app checks and a two-way Python LXMF
exchange with verified inbound delivery and outbound proof. Its packed JavaScript
artifact matched the recorded SHA-256. This run fetched the exact core source
through a local Git URL; it does not establish public remote fetchability.

The normal publication gate is separate. This checkpoint does not itself claim
that gate's completion or a successful push; the PR's publication summary records
the final result.

No fresh iOS/Android binary or board firmware was installed. Earlier physical
pairing, settings and unpaired phone-to-phone BLE evidence remains tied to its
recorded builds. The intermittent board-menu freeze, broader background behavior
and the remaining local-node demo UX are not resolved or qualified by this refresh.

The untracked SDK architecture drafts in the primary checkout are intentionally
excluded. This refresh does not implement the proposed SDK extraction.
