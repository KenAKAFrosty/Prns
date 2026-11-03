# PRNS chat application

The React Native app combines named identities, signed contacts, group messaging,
saved connections, authorized node management and network activity. It selects
one Rust image, `prns_chat_app`, and consumes the public domain APIs generated
from that image.

Start with [product scope and limits](docs/product-scope.md), the repository's
[capability overview](../docs/composable-features.md), and the
[validation guide](docs/validation.md).

## Source map

| Owner | Entry points | Responsibility |
| --- | --- | --- |
| Product construction | [`prns/chat-app`](prns/chat-app/src/lib.rs) | Installation bootstrap, private paths, pinned installation keys, resource policy and selected service image |
| Mobile integration | [`prns/platform`](prns/platform/README.md) | Expo configuration, explicit Create/Open/Stop, permissions and native process restoration |
| Generated APIs | [`prns/bindings`](prns/bindings/README.md) | Canonical domain namespaces and product API-version check |
| Image tooling | [`tools/native-bindings`](tools/native-bindings/README.md) | Image selection, complete generated-output ownership and SDK staging |
| Installation and navigation | [`app/`](prns/app/app/_layout.tsx), [`runtime/`](prns/app/src/runtime/InstallationProvider.tsx), [`Shell.tsx`](prns/app/src/ui/Shell.tsx) | Expo routes, responsive navigation, retained local owner, independent network status and identity activity |
| Identities and contacts | [`Identities.tsx`](prns/app/src/Identities.tsx), [`IdentityPanel.tsx`](prns/app/src/IdentityPanel.tsx), [`ContactList.tsx`](prns/app/src/ContactList.tsx) | Identity-scoped services, signed contact exchange, verified discovery and contact presentation |
| Conversations | [`messaging/`](prns/app/src/messaging/Messaging.tsx) | Bounded catalog/detail/member reads, immutable pending input, drafts, history and lifecycle actions |
| Network | [`screens/`](prns/app/src/screens/InterfacesScreen.tsx), [`nodes/`](prns/app/src/nodes/Nodes.tsx), [`NetworkInspection.tsx`](prns/app/src/NetworkInspection.tsx) | Saved Connections, permission recovery, capability-driven Nodes controls and read-only Inspection |

The product crate depends on [`prns-service-runtime`](../prns-service-runtime/README.md)
and its generated adapter. Domain owners remain independently usable; the product
adds no second group or identity engine. Bootstrap retains separate controller
and management-target keys, distinct from messaging identities; its Remote
Control configuration grants no initial controllers and does not self-announce.
The general
[React Native SDK](../prns-react-native/README.md) supplies platform mechanics and
native-image packaging without importing this application.

## Follow a request

1. The platform selects a private installation root and explicitly prepares new
   or existing local data. It awaits local readiness before requesting optional
   permissions or starting the network. A network refusal leaves saved contacts
   and history readable through the same owner.
2. JavaScript obtains that owner through `appOwner()` and observes its status.
   Readiness and completed cleanup are native facts; a returned handle alone
   does not establish either.
3. `IdentityPanel` obtains the selected identity and group clients. Messaging
   subscribes before its first snapshot. The conversation list loads bounded
   summary and invitation pages; action history also loads those pages for
   labels. Focused routes read their selected conversation, invitation or message
   without loading unrelated lists or scanning unread messages.
4. Before submission, the UI saves one immutable identity-bound input using the
   generated intent codec. Native retained operations own accepted work and its
   result. An uncertain response keeps the same input for reconciliation.
5. Stop fences network work and waits for retained native network cleanup before
   releasing transports. Local data stays open. React component destruction is
   not native shutdown; full shutdown belongs to the installation owner.

The shell provides Conversations, Contacts, Nodes and More, with identity-scoped
detail routes and explicit return links. Compact layouts use bottom navigation;
wide layouts use a side rail. Theme and keyboard behavior live in `src/ui/`.
Unread and invitation summaries are app projections over canonical bounded group
reads. Identity announcement preferences use the shared durable native scheduler.

Contacts separates the saved address book, Find people, reviewed Add flow and
signed My card. Find people consumes a bounded native event window, verifies each
compatible signed card against its service destination and accepted public key,
and requires deliberate review before saving. Existing contacts open directly
with their private names. Public names are signed natively from committed identity
metadata; private labels are never announced. Announce yourself uses the shared
scheduler and its cooldowns. Recent discoveries expire after a day and are cleared
when the network generation changes.

Page headers keep common actions above their content; navigation uses list rows,
secondary actions use quiet controls, and occasional actions use context menus.
More has one Connections entry; Settings owns phone setup, diagnostics and local
data. Nodes includes a read-only This phone view alongside managed remote nodes.
Conversation routes separate creation, recipient selection, invitation review,
conversation info, message details, connection activity and action history. The recipient picker
supports saved-contact search or an explicit identity address; it does not silently
save a contact or enroll the recipient. Invitation review displays expiry,
joining guidance and an exact conversation link.

Opening a message reads its exact retained membership and record, independently
of the loaded history page. Details show the sender’s timestamp, device
confirmations, delivery-window and membership states, and relevant retained
events in local order. Per-device receipt and removal facts come from bounded
native queries; a removed device can retain its earlier confirmation. Event
history can be pruned and has no event timestamps. Connection activity describes
current group peers; it is not a selected message’s send-attempt or route history.

Read views update automatically through native observations and visible health
reconciliation. Read failures offer Retry. Scanning, announcements, connection
retries and remote-node reads remain explicit commands because they initiate work
rather than refresh an existing local view.

Delivery details consumes bounded, temporary synchronization observations through
the group client. Missing or stale membership observations are unknown, while
durable message receipts retain their separate meaning. Pairing reads an atomic
live status and attempt-specific last outcome from the host; saved-target
inventory confirms availability before the UI offers management. Pairing results
are volatile and do not replace that inventory after process restart.

Setup can review a raw private identity file for the phone's controller identity.
The native importer reads the file; JavaScript handles only its cache path, public
hash and process-retained selection ID. Import requires a new installation and
does not restore messaging state. Reset requires an opaque native review and
explicit confirmation. It drains native ownership before deleting the selected
installation, then clears platform preferences and app presentation keys. A
pending marker blocks fresh setup until interrupted cleanup is resolved.

Presentation persistence has separate owners: `view-state.ts` stores navigation
and form choices, `messaging/drafts.ts` stores each conversation draft, and
`messaging/input.ts` stores the one action awaiting acknowledgement. These are not
copies of the native catalog, history or operation journal. Unreadable pending
input blocks fresh actions rather than silently replacing uncertain work.

## Source quickstart

Run these commands from the repository root. Use Node 24.18.0, npm 11.16.0,
Python 3.11+ and Rust 1.98.1 through rustup. The app uses Expo 58 and requires a
custom native build; Expo Go cannot load its Rust image. On Linux, host binding
generation also needs `libdbus-1-dev` and `pkg-config`.

First build the JavaScript contract, stage the app's selected SDK, then install
the application workspace:

```sh
npm install --global npm@11.16.0
rustup toolchain install 1.98.1 --profile minimal
export RUSTUP_TOOLCHAIN=1.98.1
npm --prefix prns-js ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-js run build:code
npm --prefix prns-js run check:built-contracts
npm --prefix applications run sdk:stage
npm --prefix applications ci
```

Staging checks the tracked bindings and creates
`applications/target/react-native-sdk` before npm resolves that local dependency.
Both the platform package and JavaScript select this same SDK image. The
application's `postinstall` applies the required runtime patch to the exact
published `@ubjs/react-native` `0.31.0-6` package. With lifecycle scripts disabled,
run `npm --prefix applications run patch:runtime` after installation and before
building native code. See the [runtime patch guide](../prns-react-native/README.md#required-runtime-patch).

For Android, install JDK 21 and Android command-line tools, set `ANDROID_HOME`
to your SDK directory, and accept its licenses. Build an arm64 image before
launching on an arm64 emulator or connected device:

```sh
sdkmanager "platforms;android-37.0" "build-tools;37.0.0" "ndk;27.1.12297006"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked --version 4.1.2
npm --prefix applications run bindings:android -- --targets arm64-v8a
(cd applications/prns/app && npx expo run:android --device)
```

For an x86_64 emulator, add the `x86_64-linux-android` Rust target and build with
`--targets x86_64` instead. For iOS, use macOS with Xcode 27.1 and CocoaPods 1.16.2
on a supported modern Ruby. Build for an Apple Silicon simulator, then launch:

```sh
rustup target add aarch64-apple-ios-sim
npm --prefix applications run bindings:ios -- --sim-only --targets aarch64-apple-ios-sim
(cd applications/prns/app && npx expo run:ios)
```

For a physical iPhone, also add `aarch64-apple-ios` and build with
`bindings:ios -- --targets aarch64-apple-ios,aarch64-apple-ios-sim`, then use
`npx expo run:ios --device` from the app directory. Device installation requires
your own Apple signing configuration. The default variant is **PRNS Chat Dev**;
`PRNS_APP_VARIANT=production` selects the production identifiers and iOS multicast
entitlement. Production iOS signing must include that entitlement.

After the native app is installed, `npm --prefix applications/prns/app start`
starts Metro for JavaScript changes. Rebuild the selected image and native app
after changing Rust exports or native platform code.

## Validation and change boundaries

After installation, run the focused JavaScript checks:

```sh
npm --prefix applications run api:typecheck
npm --prefix applications run app:verify
```

The [image recipe](tools/native-bindings/README.md) describes generation, cache
selection, native API checks and the unsigned Android build gate. Those checks
and phone acceptance establish different things; see the
[validation guide](docs/validation.md) before interpreting a result.

Rebuild `prns-js` with `build:code` and `check:built-contracts` after changing a
host or Remote Control contract. Metro checks both public exports against their
generated source and rejects missing or stale compiled code. Binding generation
and TypeScript checking alone do not rebuild `prns-js/dist`. The installed app
also retains its native contract checks.

Edit domain behavior in its owning library, regenerate through `api:generate`,
and update handwritten consumers through the generated APIs. Edit product policy
in `prns/chat-app/src/policy.rs`; platform choices belong in `prns/platform` and
`prns/app/app.config.ts`. Expo's generated `ios/` and `android/` projects and the
staged SDK are disposable build outputs. Each source owner carries its checks in
the [validation registry](../validation/manifest.toml).
