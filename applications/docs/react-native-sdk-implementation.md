# Expo/React Native SDK implementation

Implementation branch: `prns-app`, based on `0461d7aadc571ad073499c8644f318998ea0b12f`.
This record describes working-tree changes and observed validation, not a
published release. The [mobile checkpoint](../checkpoints/2026-09-23-sdk-mobile.md)
records subsequent physical-device builds, findings and remaining limits.
The [SDK adoption checkpoint](../checkpoints/2026-09-24-sdk-adoption.md) records
the later independent SDK integration and app-owned package staging.

## Ownership delivered

| Owner | Responsibility |
| --- | --- |
| `prns-host/core` | Canonical values, validated configuration and bounded queue policy; no UniFFI/Expo dependency. |
| `prns-host/impls/native` | Shared event/resource storage, readiness, native runtime, directory exclusion, joined shutdown and bounded protocol work. C and UniFFI use these mechanics. |
| `prns-host/bindings/uniffi` | Generated host and RemoteControl transports plus a foreign-object facade. Owned sessions can stop; borrowed clients cannot. |
| `prns-react-native` / `personal-rns-expo` | Reusable Expo integration, Android service/BLE machinery, Apple authorization/restoration coordination, generated bindings and the selected single native image. |
| `tools/uniffi`, `tools/ubrn-vendor`, `vendor/ubrn` | One pinned binding toolchain/runtime distribution and shared generation/ownership checks. |
| `applications/services/lxmf` | Wire protocol, signer, durable service/mailbox and one adapter to the public Rust host client. |
| `applications/prns/native-composition`, `applications/prns/platform` | Product startup/storage/reset, native service composition, workflows, contact/mailbox presentation and platform notification copy. |

The former `applications/sdk/expo` package is now explicitly product-owned under
`applications/prns/platform`. Its separate host snapshot generator and transport
copy are removed. Product bindings import the SDK converters and borrow its
`HostClientHandle`. The app delegates runtime construction to `OwnedSession`;
its native dispatcher owns application events and drains services before host
shutdown. Offline product storage remains available while networking is stopped.

The standalone SDK image is `prns_host_mobile`; the aggregate image is
`prns_app`. The canonical `prns-react-native` package keeps the standalone
provider. App generation uses the shared recipe to stage `personal-rns-expo` in
`applications/target/react-native-sdk`; all app consumers resolve that package.
Its handwritten sources come from the canonical SDK, and its generated bindings
and native library select the app aggregate. The product binding package
contains no second Rust library. The SDK has no dependency on `applications/`,
LXMF or Effect. The SDK is reviewed in
[PR #251](https://github.com/KenAKAFrosty/Prns/pull/251), independently of app
adoption in [PR #197](https://github.com/KenAKAFrosty/Prns/pull/197).

## Compatibility and lifecycle

Host schema version 2 adds delivery destination association and receive time.
C ABI layout remains version 1; the existing exact schema-version check rejects
older capsules before decoding the new fields. The canonical schema file keeps
its repository path. Host and RemoteControl fingerprints plus UniFFI namespace
checksums guard generated calls. App Expo runtime compatibility uses fingerprint
policy, so native changes require a matching binary.

All stop waiters observe the actual joined outcome. Persistence failure remains
typed and preserves the app's reset protection. Cancellation releases a foreign
wait without undoing accepted native work. Event readiness does not consume;
close releases the exclusive claim. Native services retain the app's lane across
JavaScript loss. Received resources retain their bytes independently of the
event stream. Persisted host directories remain locked until runtime shutdown
has joined.

The app retains the current PRNS/BLE identity and database layout. LXMF consumes
authenticated announce facts and routes link deliveries by local destination;
its signer stays in Rust. General RemoteControl exchanges, pairing and access
operations project the existing protocol types, while combined app queries,
prompts and Wi-Fi workflows remain product concerns.

The generated RemoteControl surface covers 217 protocol types, 30 request cases,
31 response cases and 11 pairing/access operations. Its native work shares the
bounded host command and event machinery. Resource-backed request failures now
settle the matching request waiter, including invalid or closed links. App
startup failures enter the same LXMF drain and joined host-stop paths as normal
shutdown.

Authenticated announce observation and prepared transport attachment are public
native Rust embedding hooks. They are not foreign SDK event/attachment APIs.
Destination public-key lookup is available through native Rust and UniFFI;
existing C-backed and cooperative/browser SDKs do not expose that extension.
The native extension inventory records these boundaries separately from the
cross-target host contract. Stage 4's additions therefore do not imply that every
backend exposes every extension: native LXMF uses the shared authenticated
callback directly, and foreign authenticated-announce observation is not provided.

## Reproducible checks

From the repository root:

```sh
python3 tools/repo/generate-host-contract.py --check
python3 tools/repo/check-host-uniffi.py
npm --prefix applications run bindings:aggregate:test
npm --prefix applications run api:check
npm --prefix applications run bindings:test
npm --prefix applications run native:test
npm --prefix applications run typecheck
npm --prefix applications run test
npm --prefix prns-react-native ci
npm --prefix prns-react-native run verify
npm --prefix prns-react-native run test:consumer
npm --prefix applications run native:android:client
npm --prefix applications run native:ios:client
```

Default and aggregate generation write to separate packages. Both checks must
pass in the same checkout, with app builds leaving tracked SDK outputs unchanged.
`test:consumer` expects a built default image. For the built aggregate, run:

```sh
node prns-react-native/tools/detached-consumer.mjs --android --aggregate \
  --sdk applications/target/react-native-sdk \
  --bindings applications/prns/native-composition/bindings --keep
npm --prefix prns-react-native run pack:check -- --aggregate \
  --sdk applications/target/react-native-sdk --keep
```

Keep recorded-release qualification distinct from explicit current-working-tree
export checks: a release source pin must identify an actual matching commit.

## Validation record

Observed during implementation, with the scope of each check retained:

- Shared Rust: 51 native host tests, 15 C tests, 49 host-core tests, eight UniFFI
  transport tests and nine request-journal tests pass. Core/engine `no_std`
  checks and native/C lint checks pass. The C tests include rejecting schema 1.
- Existing projections: C-backed Python and Swift smoke/conformance pass,
  including Swift's persistent two-node journey; the complete Go suite passes
  against the current C library. JVM main/test Kotlin sources compile with
  warnings as errors. N-API and WASM compile. JVM runtime tests could not run
  offline without the uncached JUnit engine/launcher; .NET 8 packs and Julia
  are unavailable locally, so those smoke tests are not claimed.
- Canonical generation: 29 generator tests and stale-output checks pass.
  Both actual generated Python journeys pass: persistent two-node host operation
  and general RemoteControl pairing/access/request operation. They exercise
  cancellation, claim replacement, resource ownership and invalid-link failure.
- App Rust: the full 185-test unit suite and four integration tests passed;
  after the final startup-drain correction, the new startup-failure regression
  and the existing admitted-insert/drain regression pass. LXMF's ten unit and
  31 durable-service tests, packet-layering test and live-peer example compile
  also passed during the cutover.
- Product JavaScript: 61 platform tests and 325 app tests pass. Strict SDK/app
  TypeScript, formatting/lint checks and generated SDK/product Swift checks pass.
  Shared Apple coordinator tests and UIKit simulator typecheck pass.
- Aggregate generated Python bindings share the app's live host, reject a
  competing application-event claim, preserve identity over restart and reset.
- Android: SDK and aggregate arm64 builds pass with exactly one selected Rust
  image and 16 KiB alignment. The independent Expo sample APK built during the
  cutover. SDK JVM tests (35) and product JVM tests (eight) pass. Two generated
  Kotlin persistence tests pass on the Pixel 10 emulator. Those emulator tests
  preceded the final invalid-link and startup-drain fixes.
- Final standalone distribution: the current schema-2/RemoteControl/runtime-fix
  Android image was packed and installed outside the repository. Strict
  TypeScript and actual packaged Kotlin compilation pass. Installed bytes match
  the staged binary; the archive contains only `libprns_host_mobile.so`.
  This final check compiled the package, without another standalone APK/device run.
- Final aggregate distribution: the actual SDK, core and product binding archives
  install in a detached consumer. Strict TypeScript including the product native
  entry and packaged SDK Kotlin compilation pass. The consumer uses one SDK
  installation and exactly `libprns_app.so`; installed/source hashes match.
  The reusable consumer tool now checks either provider explicitly and rejects
  mixed images. Ten stream/consumer tests pass.
- iOS: the final aggregate development client builds and installs on the iPhone
  17 Pro simulator. Its generated Swift smoke passes two starts, five snapshots,
  two stops and malformed-identity failure/reset. This is native binding evidence,
  not a Hermes replacement or physical restoration test.
- Web: both Expo exports build. The independent SDK sample opens a persistent
  browser host, stops, reopens and stops again in Brave without console errors;
  15 focused browser tests pass. It uses the existing WASM/browser backend.
- Source mobility: 24 tests pass. A detached export containing the final Rust
  startup-drain correction passes locked, offline aggregate compilation. Packed
  external app/platform TypeScript also passed on the earlier API-identical
  snapshot. These are explicit working-tree checks, not release qualification.

Retained local evidence includes
`/tmp/prns-sdk-default-current-qlu7d648/artifact-manifest.json` and
`/tmp/prns-application-working-tree-compile2/qualification.json`. The latter
records source snapshot
`f66a6299fcb4ecc5ebd8d9f016f3f7732670aa1444f08d7809282b128f1382c1`
and `releaseQualified: false`. Disposable exported sources and compiler caches
were removed to reclaim disk space; external compact evidence and package
archives remain. Cargo cleanup also removed `applications/target`, including its
build/test logs and Xcode derived app. Results above record checks observed before
cleanup; their detailed output is no longer retained in that directory. The
packaged SDK Android/iOS libraries and Android APK remain outside the Cargo cache.
The aggregate packed-consumer receipt, compilation log and package archives are
retained in
`/private/var/folders/gx/fwg565ys3177ht4tnx40wqv80000gn/T/prns-expo-consumer-d92jI6/`.
Existing Go/JVM binding checks and unavailable-toolchain details are retained in
`/tmp/prns-affected-sdk-*.log`.

The validation/task registries validate. A broader validation-runner unit run
passed 46 of 47 tests; its remaining failure is the existing ESP32 target symlink
in this checkout. No all-repository or all-release-gates success is claimed.

## Review cleanup

The follow-up review fixes retain the ownership boundaries above:

- The npm allowlist includes the selected iOS XCFramework. A regression test
  packs a real fixture using the production allowlist and ignore rules. The
  packed-image gate checks Android and Apple image selection, framework metadata
  and binaries. Its receipt distinguishes packaging from compilation/runtime
  evidence.
- Joined SDK shutdown failures release JS clients, streams, uploads and session
  handles while all callers retain the same failed outcome. Unavailable native
  ownership and unknown transport failures retain handles for a retry. The
  existing generated error variants carry this distinction; no schema or API
  signature changed. Snapshot `Busy` and `Stopped` remain typed errors.
- Aggregate foreign-object sharing runs as part of `native:verify` and the
  current-source detached gate. SDK `verify` includes strict TypeScript checks
  as well as its tests, runs in the hosted application-scaffold CI job, and uses
  its own locked development install without depending on `applications/`.
- LXMF destination construction is shared by the app and the existing dedicated
  identity helper. A live shared-host regression checks both identity modes and
  direct-only request policy. Obsolete raw-announcement adapters are removed;
  LXMF consumes authenticated facts from the native host.
- Ownership and validation documentation now identifies the SDK, shared native
  host and app responsibilities. Superseded app-owned Android/iOS libraries and
  their obsolete ignore entries are removed; the SDK owns the selected image.
- Generator helpers are registered with the task inventory. Rust emitters share
  one formatting helper, so checked-in output passes the same formatting gate as
  handwritten source. Formatting leaves schemas, fingerprints and TypeScript
  projections unchanged.

Cleanup validation and remaining device limitations are recorded separately
from the implementation results above. App Rust tests pass: 186 unit tests and
four integration tests. LXMF's 33 library tests and two focused facade tests
pass. SDK verification passes strict TypeScript, 18 Node tests and three Python
provider tests, including production session-wrapper regressions at the generated
binding boundary. A clean source export with neither `applications/` nor existing
dependencies passes the locked core/SDK install and SDK verification. Both
application CI jobs independently install SDK dependencies; the standard checkout
install and platform TypeScript check also pass. These workflow changes have been
checked locally; no hosted CI run is claimed.

Actual aggregate foreign-object sharing, canonical and platform generated-output
checks, 29 generator tests, app/UniFFI Rust formatting, application dependency
boundaries, validation/task registries and whitespace checks pass. Clean-install
evidence is retained in `/tmp/prns-sdk-clean-locked-evidence.json` and
`/tmp/prns-sdk-standard-checkout-install-evidence.json`.

At the cleanup checkpoint, the actual aggregate npm archive contained the
selected Android library and iOS framework. The extracted Podspec resolved that
framework, and its binary matched the staged source bytes. That packaging check
used the existing simulator-only framework. The standard `pod` command was
blocked by the local Ruby OpenSSL dependency; the installed CocoaPods core
evaluator checked the extracted Podspec instead. Packaging evidence is retained in
`/tmp/prns-sdk-pack-evidence.json` and
`/private/var/folders/gx/fwg565ys3177ht4tnx40wqv80000gn/T/prns-expo-consumer-fifK5L/`.
The final package-content check after the development-setup changes also passes;
its archive and receipt are retained in
`/private/var/folders/gx/fwg565ys3177ht4tnx40wqv80000gn/T/prns-expo-consumer-TDejh8/`.

At the cleanup checkpoint, the mobile libraries and APK predated these source
changes. Subsequent physical-device builds include the Rust error mapping and
LXMF refactor; their evidence and additional findings are recorded in the
[mobile checkpoint](../checkpoints/2026-09-23-sdk-mobile.md). Cleanup logs live under `/tmp/prns-sdk-*-tests.log` rather than a
disposable Cargo target directory.

## Follow-up mobile findings

The physical builds include the cleanup's Rust and LXMF changes. Actual Hermes
checks then exposed two further binding-integration defects:

- The product facade's second lazy import pointed outside Expo's application
  server root. Its native entry now re-exports the SDK's `borrowHost`, so the
  already-loaded entry supplies it without another lazy chunk. The facade's
  browser-safe lazy boundary remains in place; 63 platform tests pass.
- Hermes has no `FinalizationRegistry` on these phones. Generated object handles
  therefore survived JavaScript replacement even though pending Rust futures
  were freed. The shared pinned UniFFI player now owns native object guards,
  schedules ordinary GC release on the JS queue, and releases abandoned object
  references at queue-safe teardown before disarming native modules. All SDK
  object wrappers are regenerated from that shared template; there is no app
  cleanup workaround. Six Hermes reload fixtures pass in both normal and
  joined-queue teardown modes. The new object fixture also passes five runtime
  iterations, and disabling its native guard reproduces the missing release.

The runtime archives, source record, build receipt and all three npm consumer
locks were refreshed together. Installed package files match those archives.
These fixture results establish the binding-runtime behavior; the mobile
checkpoint records the separate phone reruns and transport acceptance.

## Required external qualification

The Galaxy S9+, MetalbeardMobile and one E290 became available for the
[September 23 mobile checks](../checkpoints/2026-09-23-sdk-mobile.md). That
checkpoint supersedes the earlier device-availability limitation and records
exactly which retained-data, lifecycle and runtime-replacement checks ran.
Long idle, natural suspension, OS restoration and a mobile-to-desktop
conformance-peer run remain separate acceptance gates.

The packaged standalone iOS consumer and complete detached aggregate release
pipeline have not been qualified. The actual app's aggregate provider now has
physical iPhone Debug and Release build/install evidence, in addition to the
earlier simulator build. That does not qualify an independently packaged SDK
consumer or mark the full Stage 7 exit criterion complete.

These validation runs preceded the implementation commits and branch publication.
Historical application release metadata has not been promoted to the new SDK;
promotion requires a reviewed source revision and corresponding release artifacts.
