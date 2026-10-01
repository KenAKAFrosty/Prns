# Application validation and current limits

This is a development application, not a release-qualified client. This index
separates implemented behavior, automated checks and observed device journeys.
Each checkpoint names its source and artifacts; a later checkout does not
inherit physical acceptance from an older binary. Consult PR checks for live CI
status, not the dated publishing receipts.

The [workspace guide](../README.md) owns setup and commands, the
[SDK guide](react-native-sdk-implementation.md) owns the shared host boundary,
the [product scope](product-scope.md) owns agreed product and architecture
direction, and the [roadmap](roadmap.md) owns sequencing. The full app has native
Android/iOS providers, not browser or Tauri providers. The general SDK's web entry
separately uses the existing PRNS browser host.

The revised scope adds group messaging and a general messaging-library
requirement; neither is established by existing direct-LXMF or Expo SDK evidence.
Multi-device identity and state synchronization remain unresolved, and optional
profile/social work has no acceptance claim. Existing LXMF interoperability
checks continue to qualify the implementation's actual compatibility claims;
allowing future application-protocol divergence does not waive those checks.

## Evidence index

| Area | Recorded evidence | Scope and limits |
| --- | --- | --- |
| Default SDK | [Standalone qualification](../../prns-react-native/docs/qualification.md) | Detached installation, TypeScript, Android compilation, unsigned iOS simulator consumer build and bounded physical ownership/reload checks. Its JS-owned session reopens after reload; this does not prove app native-owner continuity or public distribution. |
| App SDK adoption | [September 24](../checkpoints/2026-09-24-sdk-adoption.md) | Packed aggregate TypeScript/Android compilation, 23 current-source detached checks, physical native-owner continuity across Hermes replacement and retained-data standalone cold starts. No detached aggregate iOS consumer build or recorded-release qualification. |
| Persistence | [September 29](../checkpoints/2026-09-29-mobile-persistence-recovery.md) | Sandbox save repair, retained-data upgrades, observed interval saves on both phones and source-verified two-way BLE messages. Not power-loss durability. Its failed isolated iOS restart predates the September 30 repair. |
| Bluetooth recovery | [September 30](../checkpoints/2026-09-30-mobile-liveness.md) | Isolated app restarts and saved-contact delivery in both directions; Android Stop/Start and radio recovery; iOS interface off/on; bounded off-screen receipt on both phones. Long idle, natural suspension and silent-peer expiry remain unqualified; churn and native status-8 disconnect causes are unresolved. |
| Network inspection | [September 30](../checkpoints/2026-09-30-network-inspection.md) | Source regressions plus retained-data standalone installs on both phones. Readable tabs, connections/routes/announcements, clear/reannounce and two-way delivery passed in the focused foreground journey. No new background or physical large-text qualification. |
| Mailbox review | [September 30](../checkpoints/2026-09-30-mailbox-review.md) | Automated verification-upgrade, cursor, paging, request-ownership and preview-attribution checks. Requires matching rebuilt phone binaries; earlier device evidence does not cover these fixes. |
| Contacts and discovery | [September 24 messaging](../checkpoints/2026-09-24-messaging-discovery.md) | Messaging names, explicit announces, authenticated discovery, saved-contact sends and bounded lookup. Includes subsequent source-key retention and earlier restart failures; later recovery is recorded separately above. |
| Connections UI | [September 24 Bluetooth](../checkpoints/2026-09-24-local-bluetooth.md) | Persisted app enable/disable, retained data, peer reconnect and platform-specific large-text checks. Do not transfer those layout results to every later screen. |
| Remote management | [September 21 settings](../checkpoints/2026-09-21-remote-settings-workflows.md) | Galaxy/E290 LoRa read/write and restored values, access inventory and failed Wi-Fi trials with explicit rollback. Successful new-network Keep and iOS workflow coverage remain open. |
| Import and board interaction | [September 15 integration](../checkpoints/2026-09-15-upstream-integration.md) | Android interactive import, retained-data and bounded board journeys on the recorded builds. Fresh current-build iOS import/pairing and the board navigation freeze remain separate work. |

The app's native owner survives JS replacement and lends clients without stop
authority; the standalone SDK's JS-owned session does not. Host tests,
simulator/emulator runs, packaged builds and physical observations establish
different things. The app adoption result has `releaseQualified: false`.

### Standalone phone builds and focused acceptance

The [Network checkpoint](../checkpoints/2026-09-30-network-inspection.md#standalone-phone-builds-and-focused-acceptance)
records the latest focused phone builds at source `6ad5efadb`, exact artifact
hashes, retained-data checks and observed delivery times. Those bundled Release
builds do not use Metro. They are locally signed development artifacts, not
production distribution. These results can support bounded foreground work
selected in the roadmap; broader reliability claims still require the separate
work below.

## Repeatable checks

Use the [workspace setup and checks](../README.md#generate-build-and-check) with
the versions in [the compatibility record](../release/compatibility.json).
The principal gates are:

- `verify`: generated-output/provenance, portable native, SDK, UI, route,
  configuration, and web-export checks.
- `mobility:verify -- --working-tree`: the PR gate for an isolated current-source
  export, packed SDK dependencies and native/Python LXMF exchange.
- `mobility:verify`: the separate recorded-release gate, requiring promotion of
  the [release record](../release/README.md) to a containing SDK commit.
- `native:ios:test`: explicit macOS Swift lifecycle and release-symbol checks.
- `native:android:standalone`: complete Android Release APK assembly with embedded
  JavaScript, development signing and single-image/alignment checks.
- `native:ios:build`: complete unsigned Release iOS simulator compilation with
  embedded JavaScript, scene/Bluetooth metadata and single-image checks; it does
  not install or launch the app.
- Root application-boundary, personal-path, and diff-selected pre-push checks.

The `application-mobile-build` CI matrix runs the app build commands through
`application-android-build` and `application-ios-build`, then verifies canonical
SDK and committed product generation remain unchanged. This describes the
configured coverage, not a hosted success result. It supplements standalone SDK
builds; it does not compile a detached aggregate iOS consumer or run device
acceptance.

The detached check validates extraction, not phone packaging or remote source
availability unless its recorded run actually uses a remote source. Simulator,
host tests, an APK build, and a physical journey answer different questions.
Platform procedures are in the [iOS](ios.md) and [Android](android.md) guides.

## Firmware and repository checks

Firmware changes require the resource and repository gates for their exact
source. Dated measurements, including earlier T-Echo overflows and narrow
headroom, remain in the [historical chronology](../checkpoints/2026-09-30-validation-history.md#firmware-and-repository-checks)
and its linked checkpoints. They are not current headroom or hosted-CI claims.
Keep build targets scoped to their source checkout and verify artifact provenance
before attaching a result to a branch.

## Qualification still required

- **Remote management:** successful new-network Keep, current Android/iOS
  read/write and fresh pairing, differing capabilities, pagination, denied/busy
  operations, cancellation and uncertain-write recovery. Retained grants are not
  fresh-pairing evidence.
- **Mobile lifecycle:** silent-peer expiry, long locked/unplugged idle, natural
  suspension, repeated iOS restoration and protected-data access; remaining
  permission/radio transitions and newer Android service/notification behavior.
  Current trials do not qualify App-Switcher force quit or guarantee background
  delivery. Investigate connection churn and status-8 disconnects without
  treating temporal correlation as a proven cause.
- **Product workflows:** pristine interactive iOS identity import, wider
  cancellation cases and board/transport coverage. Preserve the recorded E290
  navigation freeze/startup-notice event as unresolved; its reset reason and
  trigger were not captured.
- **Distribution and custody:** complete detached aggregate iOS compilation,
  public packaging, production signing/R8, secret custody and broader upgrade
  guarantees. After the SDK lands, promote the compatibility revision and matching
  artifacts before running the separate recorded-release gate.

The iOS app uses event-driven Bluetooth execution windows; Android uses a
foreground service. Neither promises continuous execution. Earlier startup,
restoration and delivery-delay investigations remain in the
[dated validation history](../checkpoints/2026-09-30-validation-history.md).
Reproduce old failures on relevant current builds before prescribing another fix.
