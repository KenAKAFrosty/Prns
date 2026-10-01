# Expo/React Native SDK implementation

The reusable SDK lives below app services and policy. The app composes the shared
host and SDK in one native image, using app-owned package staging. This is a
development implementation, not a release-qualified distribution.

The [product scope](product-scope.md) separately requires an app-independent Rust
messaging crate/library for use by any application that needs messaging over
Reticulum. The Expo host SDK supplies runtime and platform mechanics; it does
not fulfill that messaging-library requirement. The existing LXMF service is
reusable groundwork, while the broader direct/group messaging boundary and
multi-device model remain design work. Protocol choices follow the product
scope; this guide describes the implementation that exists today.

The [SDK adoption checkpoint](../checkpoints/2026-09-24-sdk-adoption.md) records
independent SDK integration and retained-data app builds. The completed
[investigation, migration plan and cutover receipts](../checkpoints/react-native-sdk/README.md)
are design history, not additional active implementation plans.

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

The generated RemoteControl surface follows upstream protocol types and covers
30 request cases, 31 response cases and 11 pairing/access operations. Its native
work shares the bounded host command and event machinery. Resource-backed
request failures now settle the matching request waiter, including invalid or
closed links. App startup failures enter the same LXMF drain and joined host-stop
paths as normal shutdown.

Authenticated announce observation and prepared transport attachment are public
native Rust embedding hooks. They are not foreign SDK event/attachment APIs.
Destination public-key lookup is available through native Rust and UniFFI;
existing C-backed and cooperative/browser SDKs do not expose that extension.
The native extension inventory records these boundaries separately from the
cross-target host contract. These additions therefore do not imply that every
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

## Current qualification

The default SDK and app aggregate have separate evidence:

| Provider | Completed evidence | Remaining boundary |
| --- | --- | --- |
| Standalone `prns_host_mobile` | Detached npm installation and strict TypeScript on Android/iOS, Android SDK compilation, complete unsigned iOS simulator consumer build, and physical Android/iOS Hermes ownership/reload checks. Android also passed standalone Release startup; direct TCP exchange between the two platforms passed. | JS-owned sessions reopen after reload; this does not prove retained native-owner continuity. Public-registry distribution, extended background operation, radio/permission transitions and OS restoration remain open. |
| App aggregate `prns_app` | Packed aggregate TypeScript and Android compilation, all 23 current-source detached checks, physical Android/iOS native-owner continuity across Hermes replacement, and final standalone Release cold starts with retained data. | The aggregate detached check did not compile an external iOS consumer. Current-source qualification has `releaseQualified: false`; it does not promote the recorded release or establish production distribution. |

See the [standalone SDK qualification](../../prns-react-native/docs/qualification.md)
and [app adoption checkpoint](../checkpoints/2026-09-24-sdk-adoption.md) for source,
artifact and platform-specific scope. The adoption run did not repeat the earlier
BLE messaging or board/firmware trials in the
[mobile checkpoint](../checkpoints/2026-09-23-sdk-mobile.md). Bounded physical
success does not complete the plan's broader lifecycle or distribution criteria.

## Remaining qualification

- Qualify extended idle, natural suspension, repeated OS restoration,
  permission/radio recovery and the planned mobile-to-desktop conformance journey
  on the relevant default or aggregate build. Earlier bounded checks cover only
  their recorded cases.
- Compile the complete detached aggregate iOS consumer and qualify public package
  distribution. The standalone SDK's packaged iOS consumer build has passed;
  it does not substitute for the aggregate build or qualify public distribution.
- After the SDK lands, promote the app's historical compatibility record to a
  reviewed containing commit with matching artifacts, then run recorded-release
  qualification. Current-source extraction and development-signed Release app
  builds do not perform that promotion.

Track product acceptance in [validation and limits](validation.md) and the
[current roadmap](roadmap.md); consult PR checks for current hosted CI status.
