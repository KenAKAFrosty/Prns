# September 21: full-control pairing and first read/write screens

## Source and scope

`prns-app` is rebased on upstream
`8c211827b014bf7ef06112b383719c343432fe53`, preserving the reviewed prerequisite
merge structure. The old app head is retained at
`rescue/prns-app-before-remote-controls-20260921`. Upstream now includes
discovery-group read/write support and 30 RemoteControl request kinds.

The compatibility pin is `95af864b5580ccbf31bc6dd5808866398ad7d152`, a local
core-complete commit. No branch or PR was pushed or updated in this pass; remote
availability of this new pin is not established.

Firmware commit `c8115f013` changes local pairing to Administrator authority
plus exactly the board's advertised requests. Both confirmation screens explain
the access before approval. There is no permissions picker or retained-pairing
migration project. Fresh development pairing is required to exercise the broader
grant; the app does not infer authority from the request set.

Two reusable candidates are retained separately, both based on this upstream:

| Intended contribution | Candidate branch | Head |
| --- | --- | --- |
| Broader local pairing preset; follow-up to pairing PR #212 | `codex/remote-control-full-owner-pairing` | `37724e14e8b032aa83e20567c4daf72c69bd04fb` |
| Version-specific pairing rejection diagnostic; follow-up to diagnostics PR #215 | `codex/pairing-version-rejection-diagnostic` | `ad32e5c40eaea7ca48034871b22de0fc8b47417a` |

The existing PR branches/remotes are unchanged. The second candidate retains the
diagnostic footprint correction with its prerequisites, not a diagnostics-only
parent. Temporary source and test worktrees were removed after qualification;
the branches and logs remain. The local preparation report is
`scratch/prns-app/2026-09-21/pairing-preset-candidate.md`.

## Implemented slice

The application implementation is committed at `0c03d6173`.

The managed-node screen now reads firmware, battery/external power, interfaces,
configuration, discovery groups and peers. Pages are requested explicitly and
bounded to 128 entries. Reads cancel on route exit and cannot leak observations
between targets or native generations. Configuration refresh resets editor
drafts and confirmations.

Thirteen typed changes cover interface power/mode/group/LoRa/discovery groups,
positioning, display visibility/auto-off, station uplink, Bluetooth/hotspot mode,
system sleep/wake and radio sleep/wake. Unconfigured LoRa interfaces offer blank
setup fields with deliberate region selection, not invented current settings.
Unsupported discovery-group reads do not invalidate a valid LoRa/USB interface.

Rust validates domain values, reconnects and rechecks live capability/authority
before dispatch. Accepted writes belong to the native actor, not the screen's
promise. Results distinguish applied, unchanged, scheduled, failed and unknown;
no write is retried automatically. A result overwritten by another target's
latest operation does not resurrect a stale pending UI. Disruptive changes have
confirmation and recovery guidance. Setter-only values are actions, not fake
current-state switches.

The new facade and snapshots use generated UniFFI TypeScript, Swift and Kotlin
bindings. The contract fingerprint includes the new Rust contract module. No
handwritten language bridge, parallel permission database or JavaScript protocol
queue was introduced.

Together with existing Check/Share, the slice covers 21 of 30 operations. The
remaining nine are five transactional Wi-Fi requests, the distinct legacy Wi-Fi
setter and three controller-management requests. Those need guided workflows;
this checkpoint does not claim them implemented.

## Automated validation

- Native composition with the normal `host-test,android` features: 157 unit and
  four integration tests pass. Strict Clippy passes. The host-only variant also
  passes (152 unit and four integration tests).
- A real authenticated TCP fixture covers overview, a five-interface traversal,
  settings/discovery groups, a successful setting write, unsupported-write
  preflight, dropped reads/link cleanup and a held write reply becoming unknown
  without replay. Fixture-provisioned grants are not physical pairing evidence.
- SDK: 50 tests pass, including outbound platform admission and typed/cancellable
  management calls. Type checks, formatting and lint pass.
- App: all 247 tests across 29 suites pass. Type checks, formatting, lint,
  route/configuration checks and web export pass.
- Generated output/provenance checks, five compatibility tests, 12 detached-tool
  unit tests, dependency-boundary and unsafe-inventory checks pass. These are not
  a fresh full detached export or the complete publication gate.
- Swift startup/recovery/restoration diagnostic and release-symbol checks pass;
  this is not a new iOS app install.
- The pairing candidate passes 218 board-core and 138 Embassy tests, its
  canonical 28-graph notice check and unsafe inventory. The diagnostic candidate
  passes its payload-free regression and seven footprint behavior tests.
- Canonical main-branch notice regeneration changes only its input fingerprint;
  license inventory/allowlists are unchanged.

The broad app `verify` command stops at the live Expo dependency check, which
now asks for four newer SDK-57 patch versions (`expo`, `expo-router`,
`expo-constants`, `@expo/metro-runtime`). Dependencies and exclusions were not
silently changed. The first Expo Doctor reports 19/21: the same version check
plus the shell-selected CocoaPods Ruby missing its old OpenSSL 1.1 library.
Using the already-installed Nix CocoaPods 1.16.2 resolves that tooling check;
the repeat is 20/21 with only the four patch versions outstanding. No global
tool installation was changed. The remaining portable checks were run
independently; do not report the broad gate as green.

## Firmware and device qualification

An initial T-Echo resource attempt compiled firmware but correctly refused an
evidence report because source changed during the build. It is not a passing
resource gate. Fresh stable-source firmware results and Android packaging are
recorded below when complete.

No phone installation, board flash, fresh owner pairing or physical setting
change has yet qualified this slice. Earlier device/background evidence belongs
to earlier binaries. The board-menu freeze remains a separate unresolved issue.
Hardware acceptance and the guided Wi-Fi/controller workflows are next; see the
[current expansion plan](../docs/remote-control-expansion.md).

Local logs for this pass are under `scratch/prns-app/2026-09-21/`.
