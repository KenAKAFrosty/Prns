# Standalone SDK qualification

This record covers the default `prns_host_mobile` provider prepared on
2026-09-24 from upstream `100583182d9ac02103fe7eb59148657545b518a2`.
The source checkout contains no PRNS app tree. It does not reuse the earlier
app aggregate image's device results as evidence for the default provider.

## Passed locally

- Canonical contract generation, shared generator/vendor checks and strict Clippy
  for the native host, UniFFI facade and default image.
- Strict SDK and standalone example TypeScript; session, iterator, provider and
  package checks; Apple lifecycle helpers; 35 Android platform JVM tests.
- 52 shared native-host tests, two upstream snapshot tests, eight UniFFI
  transport tests, and generated Python host/Remote Control conformance.
- 15 C ABI tests and existing C/C++ and Python persistent host journeys.
- Six Remote Control, nine journal-delivery and 53 link-establishment regressions.
- Android arm64 and x86_64 default images with 16 KiB ELF alignment.
- Standalone Android Debug and Release APKs, including the Release Hermes bundle.
- iPhone arm64 and Apple Silicon simulator frameworks.
- Standalone iPhone Debug app compile and signing with an existing device profile.
- Detached npm archive installation and strict TypeScript for Android and iOS;
  Android SDK compilation and a complete unsigned iOS simulator consumer build.
  Selected Android libraries and Apple framework slices retain their source hashes.
- Scene-enabled example startup on the iOS 27 simulator. The detached iOS gate
  checks the compiled Expo scene manifest.
- Actual simulator Hermes session checks with the default image and no app policy:
  `bigint` revision values, bounded numeric uptime, exclusive application-event
  claims, closing a pending reader, shared/idempotent stop and released clients.
- A real Hermes reload with a pending diagnostic reader; a new JavaScript runtime
  reopened the persistent host with the same identity and reclaimed both streams.
  This checks restart/reopen behavior, not retained native-owner continuity.
- Validation/task/workflow registries, scoped license policy, Linux/Android/iOS
  dependency license checks and the updated unsafe inventory.

## Physical device checks and limits

The default-provider Debug example passed on Galaxy S9+ (Android 10/API 29)
and MetalbeardMobile (iPhone 14 Pro, iOS 27.0), using source commit
`f03be48d2f2e66f1fde5ef39e6fe75ddae65ead4`. Both physical Hermes runtimes confirmed
the selected image and absence of app-policy modules, exclusive ownership of the
application stream, pending-reader closure, shared stop completion and released
clients. Each kept a
pending diagnostic reader across a real runtime reload, then reopened the same
persisted identity and reclaimed both streams. The iPhone example's Open/Stop UI
also passed. These checks cover JavaScript-owned sessions, not native app-owner
continuity.

The Galaxy also passed a same-signature Release upgrade and cold startup with
Metro stopped and no USB port reversals. Its bundled JavaScript opened a native
host, displayed the snapshot and stopped the host through the example UI. The
Release example was left stopped with its data retained.

A direct Wi-Fi TCP connection from the iPhone to the Galaxy passed announcement
routing, encrypted link establishment, a random nonce packet and exact-byte echo,
schema-2 delivery fields and `bigint` traffic counters. The forward command
settled with `ExplicitProof`. The reverse packet reached the iPhone but its send
settled with `DeliveryTimedOut`, matching the existing core contract: the link
initiator emits no automatic packet proof. The test now checks that distinction
explicitly. See [the SDK guide](../README.md) before treating delivery timeout as
proof that a peer received no bytes.

Android reload logged a React Native soft exception for an AppState emission
after instance destruction; native teardown and all post-reload probes passed.
No fatal native crash was observed. Tests used the separate
`rs.reticulum.prns.host.example` bundle, without resetting or replacing existing
PRNS app installations or data.

The CI definitions are checked locally; hosted CI has not run on this branch.
The detached consumer pins the reviewed runtime archives. It does not establish
installation using only public registry packages. Registry publication, extended
background operation, radio/permission transitions and OS restoration require
separate qualification.

## App adoption follow-up

The app in [PR #197](https://github.com/KenAKAFrosty/Prns/pull/197) still
selects its aggregate image in the tracked SDK output directory. Restacking that
PR must keep the canonical SDK files on the default provider and generate the
app-selected SDK package into app-owned staging through the shared recipe. Both
SDK and app checks must pass in one checkout, and app builds must leave tracked
SDK outputs unchanged. That app migration is separate from this SDK preparation.

## Repeating the checks

Follow the source setup and native build commands in the [SDK guide](../README.md).
The validation registry includes `react-native-sdk`, `react-native-generated`,
`host-native-session`, `host-uniffi-session`, `host-uniffi-conformance`,
`react-native-android-image`, `react-native-android-consumer`,
`react-native-apple-platform`, `react-native-apple-image`,
`react-native-mobile-archive` and `react-native-apple-consumer`.
The mobile suites expect the platform toolchain and previously built artifacts
specified by their CI jobs.

For physical runtime acceptance, install the independent example, confirm the
selected image is `prns_host_mobile`, and exercise public SDK sessions: exact
`bigint` revision values, exclusive event-lane claims, cancellation with a pending
reader, client release without shutdown authority, and idempotent owner stop.
Hold a pending reader across a real Hermes reload, then reopen a persistent host
and verify its identity. A JavaScript-owned standalone session restarts after
reload; retained native-owner continuity belongs to a separately configured
native composition.
