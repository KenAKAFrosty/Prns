# PRNS app platform

This private Expo package exposes only installation and platform actions:

```ts
installationPresent(): Promise<boolean>
start(mode: "create" | "open", bluetooth = false): Promise<void>
startImported(reviewId: string, bluetooth = false): Promise<void>
stop(): Promise<void>
prepareReset(): Promise<void>
resetPreferences(): Promise<void>
bluetoothSelected(): Promise<boolean>
bluetoothAuthorization(): Promise<"notDetermined" | "restricted" | "denied" | "allowed">
status(): Promise<PlatformStatus>
requestBluetoothPermission(): Promise<PlatformStatus>
requestNotificationPermission(): Promise<PlatformStatus>
openSettings(destination: SettingsDestination): Promise<void>
setBluetoothEnabled(enabled: boolean): Promise<void>
bluetoothDiscoveryGroups(): Promise<BluetoothDiscoveryGroups>
setBluetoothDiscoveryGroups(groups: string[]): Promise<BluetoothDiscoveryGroups>
```

Presence checks the installation directory selected by canonical
`appInstallationDirectory()` below `prns-app`, including a partial or invalid
installation. It does not certify integrity. Create and Open are explicit; Open
never creates replacement controller keys or silently resets damaged state.
The platform derives the private root. The generated Rust startup hooks are used
by these native delegates; product JavaScript does not choose paths. Reviewed
reset executes in the native product owner.

Cold startup prepares the local installation and awaits data readiness before
requesting optional radio permissions or foreground-service admission. A network
refusal therefore leaves local contacts and history available. `startImported`
accepts only the ID of an exact process-retained reviewed identity. Preparation
consumes that capability; the subsequent network start and every restoration
strictly Open. No private key, imported secret or implicit Create is persisted in
platform restoration preferences.

After startup the UI uses generated `app_owner()` and the owning identity, group,
and host clients directly. This package adds no domain methods or copied status
models. `ServiceRuntime.status()` describes the local data owner;
`networkStatus()` describes networking independently. Startup awaits both local
readiness and `networkReady()` before attaching optional Bluetooth. The product selects the native owner's
`RecoverCurrent` startup policy: saved Enabled identities and already accepted
lifecycle work are scheduled through canonical recovery before Ready is published.
Ready confirms construction and recovery admission, not that every identity is
Running. The UI reads each identity snapshot for actual activation or failure.
Unconfirmed Reserved operations remain inert, and missing configured schemas are
never initialized by ordinary reopen.

Expo owns scene startup and forwards application lifecycle events to the
platform's app-delegate subscriber. The process owner remains independent of
scene destruction. The config plugin sets the app's deployment minimums and
backup policy.

The iOS process coordinator and Android foreground service survive React reloads.
Stop invalidates pending platform starts, fences networking, and observes its
retained `ServiceRuntimeNetworkChange`. The local data owner remains open for
contacts, identities and conversation history. A full installation shutdown is a
separate native operation. Failed network cleanup retains ownership; dropping a
JS promise does not abandon startup or teardown. iOS protects file access before
unlock, gates Bluetooth restoration separately from explicit startup, and only
retries a protected-data refusal that occurred before native construction.
Android saves a bounded nonsecret reopen/transport selection; malformed saved
input fails instead of becoming Create. Its SDK delegate observes async native
network readiness/drain without blocking the lifecycle lane.

Reset is an explicit reviewed native operation. `prepareReset()` captures the
platform’s private root without opening data or starting networking, including
after a process restart during interrupted cleanup. After Stop and successful native
installation deletion, `resetPreferences()` clears only this product's Bluetooth
selection, discovery groups and restoration input. It refuses an active platform
operation or registered local owner. Failed persistence remains visible and can
be retried; OS permission history and unrelated preferences are retained.

The native profile owner restores saved LAN/TCP intent and owns their attachment
and teardown. Platform startup never inserts an unconditional AutoLAN interface.
The LAN monitor still forwards network changes to the reusable native adapter.
Bluetooth is attached only when explicitly selected. Explicit platform actions
request permission; reading status never opens a permission prompt. Android also
reports notification permission and channel blocking, and offers system settings
recovery. iOS reports radio availability as unknown when no live observation is
available. A Bluetooth error leaves the ready node and saved connections available
and rejects that Start request with the transport error.

Changing Bluetooth selection or discovery groups drains only the captured host's
Bluetooth attachment before replacing its platform resources and reattaching.
LAN/TCP and local data remain available. Stop invalidates queued changes; a stale
continuation cannot acquire a replacement host. Discovery names use the canonical
transport validator and are saved before applying. A failed apply can therefore
leave a new saved preference with `applied: null`; the UI rereads both facts before
retrying. Saved preferences do not authorize background startup. Neither
permission status nor a successful attach proves peer connectivity.

Native packaging depends on the staged `personal-rns-expo` SDK selected for
`prns_chat_app`. That SDK compiles all owning Swift/Kotlin namespaces and links one
native image. The generated files in `@prns/app-bindings` remain review artifacts
for native languages and are not compiled into a second library by this package.
The app config preserves development and production install identifiers; its
production iOS multicast selection must match the signed entitlement. No physical
phone, protected-data restoration, radio, or signing acceptance is implied by
host tests or source compilation.

The saved Bluetooth selection survives Stop and JavaScript reload. It is distinct from permission. A successful explicit native startup records either choice, including false; restoration does not silently opt in.

Android saves the explicit Bluetooth choice together with successful startup input. Stop removes restoration input but preserves the separate nonsecret choice; remembering a choice does not authorize a background start.
