# App adoption of the independent Expo SDK

The app in [PR #197](https://github.com/KenAKAFrosty/Prns/pull/197) now builds on
the independent SDK in [PR #251](https://github.com/KenAKAFrosty/Prns/pull/251).
The integration preserves the published app history through normal merges,
including upstream `58db1c0c4350e950c77ff9cabfee9d1cda58df1e` and SDK
`a87990445c24e12b57695210af18e8b05994084f`. The SDK PR should merge first; the
app PR remains a draft targeting `trunk` until that dependency is accepted.

## Ownership and packaging

- `prns-react-native` always selects the standalone `prns_host_mobile` provider.
  Its maintained sources and generation recipe belong to the SDK PR.
- The app stages `personal-rns-expo` in `applications/target/react-native-sdk`,
  selecting the single `prns_app` aggregate image. All three application
  packages resolve that same SDK package. Product bindings add no second image.
- The shared generator derives staged runtime sources from the SDK package's
  publishable file list. It excludes development-only scripts and dependencies,
  checks owned output, and rejects stale files or a mixed image selection.
- The app retains storage, LXMF service composition, mailbox/contact policy,
  startup/reset admission, notifications and product workflows. It uses the
  public host and generated SDK values; no second handwritten host bridge is
  maintained by the app.
- Clean setup stages the package before `npm ci`. `stage` checks committed
  product bindings without rewriting them; explicit generation updates them.
  SDK and app generated-output checks can pass in the same checkout.

## CI and maintenance fixes

App CI stages the aggregate package before installing workspace dependencies.
The native lane checks canonical default generation after app generation and
rejects tracked SDK changes. Detached PR validation explicitly exports the
current source, packs the selected SDK, and installs it outside the checkout.
Tests resolve the installed SDK and transform its TypeScript in both linked and
packed installations.

Recorded-release qualification remains a separate release/scheduled suite. Its
old compatibility revision predates the extracted SDK and must be promoted to
a real containing commit with reviewed artifacts before it can qualify a
release. A successful current-source receipt has `releaseQualified: false`.

Expo's online compatibility check required patch updates to Expo 57.0.25,
build-properties 57.0.22, linking 57.0.11 and router 57.0.23, including their
resolved dependency updates. The application version assertions and lockfile
were refreshed. Rust lock cleanup removes obsolete dependency edges without
upgrading Rust packages; third-party notices were regenerated.

## Local validation

- Canonical SDK verification: 21 JavaScript tests, 14 Python tests and strict
  TypeScript. Default generation still passes after app generation/builds.
- App generation tests, strict staged-output checks, application dependency
  boundary, compatibility checks and the validation registry pass.
- App Rust: 186 unit tests and four integration tests. Actual generated Python
  bindings share the aggregate host, reject a competing application-event
  claim, retain identity across restart and exercise reset.
- App UI: 325 tests, strict TypeScript, formatting/lint, route/config checks,
  online Expo dependency compatibility, all 21 doctor checks and web export.
- Platform: 63 JavaScript tests, strict TypeScript, Apple lifecycle/recovery/
  restoration/release-symbol helpers and native linkage checks.
- An aggregate npm consumer installed the actual core, SDK and product archives
  outside the checkout. Strict TypeScript and packaged Android SDK compilation
  passed; installed Android/iOS artifact hashes matched and no second SDK/image
  was installed. This aggregate check did not compile a detached iOS consumer.
- Full current-source detached qualification passes all 23 recorded checks,
  including the packed SDK/platform and app tests, strict generated-output
  checks, aggregate foreign-object sharing, native tests, web export and a
  pinned Python/native LXMF exchange. Its 1,912-file snapshot is
  `5c451363c3f96faa537dfca0621bfe63f09179b15a72bbdae5c1d905ef9c15c5`.
  The receipt records reused disposable Cargo/compiler caches and
  `releaseQualified: false`. Compared with integration commit `283c60505`,
  only this checkpoint and two recorded-release license-copy entries differ;
  runtime inputs and the current-source qualification path are identical.

## Physical runtime checks

Both physical Android and iOS Debug apps passed SDK-owned session checks and
borrowed app-client checks. A real Hermes destruction/recreation with a pending
diagnostic reader preserved the native app generation and identity, advanced
uptime continuously, reclaimed diagnostics, and retained the native owner's
exclusive application-event lane. Releasing a borrowed client preserved its
owner; it exposed no stop/close authority.

These Debug checks preceded the final Expo patch refresh; the SDK/Rust runtime
sources were unchanged by that refresh. Android retained all 11 mailbox IDs,
zero contacts, the controller identity and saved target. iOS retained all 21
messages, two contacts, one pairing and its primary/controller identities.
Android's pre-install non-debuggable build permitted a UI baseline, rather than
a raw database comparison. iOS comparisons used its retained application data.
Private snapshots, keys and message contents remain outside tracked source.

Final standalone Release builds include the Expo patch refresh. Both installed
without resetting data, cold-launched with Metro stopped, and visibly showed the
running node and saved target. Android had no USB port reversal and passed normal
Stop/Start again. iOS exposes no equivalent Stop/Start control. All retained
identities, pairings, contacts and message counts still matched their baselines.
Both devices were left running the standalone build.

| Final artifact | SHA-256 |
| --- | --- |
| Android installed APK | `c2f0c6ace1256e98f95fa83771fa587153712294cb6752d3a9629308bc777180` |
| iOS app executable | `ac033670df2fa8d5d37067e70603628370966b7aa0cd8fdd484ae1cdd3a74e76` |
| iOS JavaScript bundle | `586cb58ed3b3e4b177515e11607e517732f95db7d43e2055764c9da2016ead2d` |
| iOS selected framework | `a1e505782fcfdbac52be6967b669a02cb77b4a5c22638b2a727e272aeeca41bf` |

Compact local receipts are retained under the ignored `target/qualification/`
directory. These are development identifiers and signing profiles with embedded
JavaScript, not production-distribution builds.

## Scope

This adoption run does not repeat the earlier two-phone LXMF exchange or board
pairing/firmware tests. It does not qualify long idle, OS restoration, production
distribution or additional Android versions. Independent default-provider
evidence belongs to the [SDK qualification record](../../prns-react-native/docs/qualification.md).
Current-source extraction and local device checks do not promote a recorded
release or establish hosted CI success; consult the PR checks for their current
commit.

Hosted CI exposes an existing upstream firmware-assurance failure: adding
`muzi-base-duo` raised the target count to 15, while four assertions and the
14-target baseline were not updated. Six tests fail identically in the
[upstream run](https://github.com/KenAKAFrosty/Prns/actions/runs/36047238325/job/107793683195)
and [SDK run](https://github.com/KenAKAFrosty/Prns/actions/runs/36055973989/job/107822855371).
A focused local reproduction confirms the same failures. Refreshing that
baseline requires its missing board evidence; this SDK adoption leaves those
checks intact and does not claim hosted CI is green.
