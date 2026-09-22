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
resource gate. The subsequent stable-source matrix passes **all 14 configured
profiles** at `e805c32495c6e79677882db770be7660e50e8590`. Every report names
that source head; the existing unrelated untracked files were preserved.
This is main-branch firmware qualification, not a rerun of each isolated PR's
full publication gate.

| Profile | Firmware image | FLASH headroom |
| --- | ---: | ---: |
| T-Echo S140 v7 | 619,628 bytes | 2,964 bytes |
| T-Echo S140 v6 | 619,508 bytes | 7,180 bytes |
| E290 | 2,323,184 bytes | 12,803,344 bytes |

The Android standalone development APK builds successfully and passes all 43
Android adapter unit tests. Its bundled JavaScript includes this slice; the APK
contains the arm64 Rust library, retains Android 10 as the minimum (API 29),
and passes signing/alignment checks. It is locally development-signed, not a
production release. No package versions, lockfile or generated source changed
during packaging.

- APK: `prns/app/android/app/build/outputs/apk/release/app-release.apk`.
- SHA256: `7153a0a24bfd8ca1780b1f50a8a5a401752470bf11a97df159ea7146e4cf1cd1`.
- Build log: `scratch/prns-app/2026-09-21/android-build.log`.
- Firmware log: `scratch/prns-app/2026-09-21/firmware-matrix.log`.
- Reports: `target/flash-artifacts/resources/configured/reports/` (external
  build cache, not committed application source).

At the initial checkpoint, no phone installation, board flash, fresh owner
pairing or physical setting change had qualified this slice. Earlier
device/background evidence belongs to earlier binaries. The board-menu freeze
remains a separate unresolved issue. Hardware acceptance and the guided
Wi-Fi/controller workflows are next; see the
[current expansion plan](../docs/remote-control-expansion.md).

Local logs for this pass are under `scratch/prns-app/2026-09-21/`.
The final upstream check still resolved to `8c211827b`; nothing was pushed.

## Same-day device installation

Both packages were rebuilt from `0d00c25f35136c33f1c587063bf3e2e0d039259c`
(only documentation differs from the resource-qualified firmware source).

- The standalone, bundled-JavaScript Android APK was installed over the existing
  app on the USB-connected Galaxy S9+ (SM-G965U, Android 10), preserving app data.
  The cold launch succeeded, the Nodes screen rendered, Bluetooth reported ready,
  and the previous pairing remained visible. No startup crash appeared in the
  captured process log. This is startup evidence, not new remote-command or
  background-lifecycle acceptance.
- Installed APK SHA256:
  `b2ed4ff5a4a13890de4241cd2144aa205ebe6153fac08ee9222df4b3a53b7bdb`.
  Build log: `scratch/prns-app/2026-09-21/android-install-build.log`.
- The newly connected E290 was identified on `/dev/cu.usbmodem101` as ESP32-S3
  with 16 MiB flash. The other serial device was not touched. The canonical local
  firmware build wrote and verified all three sparse parts (2,347,312 bytes),
  then issued the board's reset. No full-chip erase or provisioning change was
  requested; the existing settings partitions were left intact.
- Flashed application: 2,323,184 bytes, SHA256
  `29bbd93aea9ae593acf4eeb59db405c5d695bb4c33b9b82e4aab2ced81fd0e0d`.
  This is the newly staged local-flash artifact, not a reused resource-cache
  artifact. Receipts: `scratch/prns-app/2026-09-21/e290-preflight.jsonl` and
  `scratch/prns-app/2026-09-21/e290-flash.jsonl`.

At this installation checkpoint, fresh full-control pairing, post-flash board
behavior, and physical read/write acceptance were still pending. Later pairing
evidence is recorded below. No branch was pushed during installation.

## Later pairing trial and confirmation polish

The user completed fresh pairing on the Galaxy after earlier board freezes and
reboots. The phone was still running the APK above; the E290 was running
`04b0be101bdd` from the isolated `codex/e290-ui-stall-diagnostics` branch. The
phone's Paired screen was observed, and the node remained in its saved list after
an app update and restart. This is diagnostic-firmware pairing evidence, not
resolution of the intermittent fault or acceptance of every remote operation.

The diagnostic trace captured one failed button attempt (all output stopped for
about 15 seconds before USB loss) and one successful close after expiry. The
original reset reason was obscured by the first capture helper's reattachment;
the revised passive helper preserved uptime during a tested live reattachment.
No pairing Close timeout, display recovery, or watchdog change was introduced.
Diagnostic source and its detailed record remain on their separate branch;
`prns-app` does not enable that firmware tracing.

App confirmation now puts the code first, replaces the comma-separated control
list with a short summary derived from the actual requests, and keeps the
Administrator disclosure visible. **Show pairing details** reveals the exact
controls one per line and the node ID. Disclosure resets for each attempt;
approval/rejection behavior and the grant itself are unchanged. Redundant setup
instructions and Bluetooth setup cards are hidden during code confirmation.
The saved-node cards and live connection result reuse the same compact summary;
their request availability and connection-check authorization are unchanged.

The new confirmation component was also rendered in a local React Native Web
fixture at phone width and checked collapsed and expanded. This validates that
layout with fixture data, not an actual confirmation on either native platform.
