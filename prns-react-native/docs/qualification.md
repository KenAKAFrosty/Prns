# SDK qualification

Qualify the default `prns_host_mobile` provider independently of application
compositions. A source check, a detached package build and an installed phone
build establish different things; record the source commit, selected provider,
artifact hashes, device/OS and commands with each result.

## Source and package checks

Follow the [SDK guide](../README.md#source-quickstart) for toolchains and native
builds. Its [qualification section](../README.md#qualification) documents
`verify`, generated-binding checks, `test:consumer` and `pack:check`, including
Android, Apple and aggregate-provider options.

The [validation registry](../../validation/manifest.toml) owns the suite list:

| Area | Suites |
| --- | --- |
| Public contract and lifecycle | `react-native-sdk`, `react-native-generated`, `host-native-session`, `host-uniffi-session`, `host-uniffi-conformance` |
| Android image and package | `react-native-android-image`, `react-native-android-consumer` |
| Apple integration and package | `react-native-apple-platform`, `react-native-apple-image`, `react-native-apple-consumer` |
| Archive contents | `react-native-mobile-archive` |
| Selected native attribution and exact runtime source custody | `react-native-notices`, package `notices:inputs` |

The Android consumer also runs the maintained SDK unit tests against the packed
production module; test sources remain outside the shipped archive.

Native consumer checks require matching built images and platform toolchains.
Keep their receipts and logs with CI or release artifacts, outside the source
tree. A simulator-only framework qualifies simulator packaging; a detached
consumer build does not qualify an installed APK or iOS device build.

## Phone acceptance

Use the standalone example and confirm that it selects `prns_host_mobile`
without application-policy modules. On Android and iOS, verify:

- Exact `bigint` revisions, exclusive event-lane ownership and cancellation
  while a reader is waiting.
- Client release without shutdown authority, shared/idempotent owner stop,
  and reopening the same persisted identity.
- A real Hermes reload with a pending diagnostic reader, followed by reopening
  the host and reclaiming both event streams.
- A standalone release build that starts with Metro stopped, plus a signed
  upgrade that preserves its state.
- Two endpoints exchanging announcements, encrypted Link traffic and exact-byte
  replies. Check the SDK's [delivery-proof contract](../README.md#open-a-host):
  a reply can arrive while the sender reports `DeliveryTimedOut` when the Link
  initiator does not emit an automatic packet proof.

A JavaScript-owned session restarts after a reload. Retained native-owner
continuity requires its own composition and recovery tests. Exercise permission
changes, radio transitions, background operation and OS restoration separately;
a foreground demonstration does not qualify those lifecycle paths.

## Application compositions and distribution

Generate each composition into its own destination and select that same SDK
image in every consumer. Check the default SDK and the app together; app builds
must preserve the tracked default-provider outputs. Qualify application services
and native-owner continuity against the app's actual binary.

The SDK remains a source preview. Its runtime peers use exact public npm versions;
the consuming app applies the SDK-shipped text patch before native compilation,
as described in the [runtime setup](../README.md#required-runtime-patch). Publishing
the SDK itself still requires a clean consumer that resolves published packages
without local overrides. Release qualification must refer to the actual packaged bytes;
older local checks or another provider's phone trial do not qualify a new build.

The runtime npm packages currently carry an unresolved MPL-2.0 approval
finding in the production npm audit. Successful source, image or detached
consumer checks do not qualify distribution while that finding remains open.
