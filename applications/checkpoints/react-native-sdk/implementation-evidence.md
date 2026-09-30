# SDK implementation and cutover evidence

This preserves the initial cutover and follow-up observations previously recorded
in the SDK implementation guide, as of September 30, 2026 at `32dc36b13`.
The cutover began at `0461d7aadc571ad073499c8644f318998ea0b12f`.
Counts, local receipt paths and device results belong to those recorded runs;
archiving this account did not rerun any checks. Local temporary paths are
provenance, not files expected in a fresh clone.

Current ownership and outstanding qualification live in the
[SDK implementation guide](../../docs/react-native-sdk-implementation.md);
later app adoption is recorded in the
[September 24 checkpoint](../2026-09-24-sdk-adoption.md).

## Historical implementation validation

Observed during the initial cutover, before independent SDK qualification and
app-owned staging. Counts, binaries and local evidence paths below belong to
those runs, not a current-source test report:

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

## Historical review cleanup

The follow-up review fixes retain the SDK and app ownership boundaries:

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
[mobile checkpoint](../2026-09-23-sdk-mobile.md). Cleanup logs live under `/tmp/prns-sdk-*-tests.log` rather than a
disposable Cargo target directory.

## Historical mobile findings

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
