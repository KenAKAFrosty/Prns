# Application validation

The [application walkthrough](../README.md) and [product scope](product-scope.md)
describe the code under test. The [validation registry](../../validation/manifest.toml)
owns runnable suites; reusable-library checks stay with their libraries.

## Owning checks

Run from the repository root with the pinned toolchains and installed dependencies
specified in the [image recipe](../tools/native-bindings/README.md) and
[SDK setup](../../prns-react-native/README.md#source-quickstart).

Application dependency installation runs the app-owned runtime patch command. If
you used `--ignore-scripts`, run `npm --prefix applications run patch:runtime`
before native validation. See the [setup guide](../README.md#build-and-change-boundaries).

| Suite | Entry point | What it checks |
| --- | --- | --- |
| `app-native` | `cargo test --locked --manifest-path applications/Cargo.toml --workspace --all-targets --all-features` | Product bootstrap, strict create/open, selected policy and retained owner lifecycle |
| `app-native-clippy` | `cargo clippy --locked --manifest-path applications/Cargo.toml --workspace --all-targets --all-features -- -D warnings` | Product Rust lint contract |
| `app-generated` | `npm --prefix applications run api:check` | Selected image and complete generated-output ownership |
| `app-ui` | `npm --prefix applications run app:verify` | Handwritten TypeScript and UI/state/recovery behavior |
| `app-image-swift` | `npm --prefix applications run api:image:test` | Real native image, generated Swift namespaces, API version and native owner behavior |
| `app-android-consumer` | `python3 applications/tools/native-bindings/check_android.py --offline` | Selected Android image, SDK/product compilation, maintained unit tests and unsigned APK structure |

Focused tooling and generated TypeScript checks are also available through
`npm --prefix applications run api:test` and
`npm --prefix applications run api:typecheck`.

Use `npm --prefix applications run api:generate` after changing canonical APIs.
It stages the matching SDK as well as tracked language outputs. `sdk:stage`
materializes that SDK only after checking the tracked bindings. Never hand-edit
generated files or package a second copy of the native image.

The Android checker can run on macOS; label that result separately from its
registered Linux CI lane. `--inspect-only` checks existing artifact structure,
not whether it was built from current source. Omit `--offline` only when the
required dependencies are not cached. See the owning recipe for prerequisites
and the report format.

## Mobile acceptance

Source checks, host foreign consumers, simulator builds, packaged artifacts and
physical trials establish different things. None substitutes for another.
For a distribution candidate, retain its source revision, selected image,
toolchain, package hash, signing configuration and device/OS alongside results.
Keep logs, screenshots and per-run narratives in CI/release or external trial
artifacts, rather than in these source guides.

Acceptance on the intended mobile build covers:

- Explicit Create/Open, stop during startup, pending-observer Stop, local history
  after network Stop or admission failure, reopen and
  strict failure on damaged or unsupported persisted state.
- Identity selection, signed contact exchange, invitation consent, text/receipt
  exchange and pending-action reconciliation after interruption.
- Contact discovery between running apps: announce, observe a compatible signed
  card automatically, review/save, and open an already saved contact with its
  private name intact. Check that clearing network announcements and clearing
  recent discoveries are independent of each other and the saved address book.
- Message details for incoming, outgoing and older-membership records: exact
  owner/group/record navigation, device confirmation/removal facts, missing or
  pruned event history and return to the same conversation. Sender time and
  current connection activity must not become invented receipt or attempt times.
- Remove/Leave/Close/Delete and their distinct effects, including retained cleanup
  after visible local deletion. A reset is not evidence of successful recovery.
- Permission changes, Bluetooth/radio transitions, LAN/TCP selection, natural
  suspension and OS restoration where those behaviors are claimed.
- Routed contact-to-invitation journeys, identity switching, other-identity
  activity, light/dark layouts and Back navigation. Inspect actual narrow-screen
  title/menu placement and the full message input and Send control above the
  keyboard, including its suggestion bar; element existence alone is insufficient.
  The “This phone” screen must remain accessible with networking stopped.
- Identity-file review without exposing key bytes to JavaScript; explicit reset,
  rejected cleanup and interrupted-reset recovery on disposable test data.
- Fresh installation and an in-place signed update of supported state, using the
  actual selected development or production identifiers and entitlement profile.

An unsigned APK or host Swift consumer does not prove phone messaging, radio
behavior, signing or background delivery. Platform capability advertisements and
logical resource budgets also do not establish those results.

## Capacity and distribution limits

The alpha does not require full-capacity stress runs. Representative workload
measurements and the optional large qualification commands in the
[group guide](../../prns-groups/README.md) have separate scope from ordinary
correctness checks. Host timings and memory measurements are not mobile capacity
guarantees, and qualification targets are not domain count caps.

The [release policy](../../docs/release.md) and owning dependency/attribution
checks apply independently. Passing source or package checks does not waive
unresolved dependency-policy findings or qualify public distribution. The
[SDK qualification guide](../../prns-react-native/docs/qualification.md) describes
its source-preview and independent-consumer limits.
