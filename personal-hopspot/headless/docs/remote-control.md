# Controller-operated Hopspots

Hopspots do not schedule application announcements at startup, on reconnect, or on
a timer. A controller or explicit local user action chooses when to announce.
Transport discovery (Auto-WiFi beacons, DNS-SD) and responses to requested paths
remain necessary protocol operations; they are not application announcement policy.

## Provision and operate

Build the native lab controller:

```sh
cargo build --manifest-path personal-hopspot/headless/Cargo.toml --example controller
controller --state-dir /private/controller-state identity
```

Use the built example executable in place of `controller`. Retain this private
state directory: restarting the tool must not change the controller identity.
The output contains its public identity hash and full public key. Provision the
**full public key**, not a password or the hash alone, on each target:

```sh
personal-hopspot-headless --state-dir /persistent/hopspot-state \
  --listen 0.0.0.0:4242 --controller-public-key "$CONTROLLER_PUBLIC_KEY"
```

Repeat `--controller-public-key` for additional controllers. The initial grant is
Operator with `Describe`, `AnnounceSelf`, `DescribeBuild`, `InventoryInterfaces`,
`InventoryInterfaceConfig`, and `InventoryInterfacePeers`; it cannot administer grants
or mutate interfaces. For a lab controller that also needs the bounded application
probe, use `--app-controller-public-key`. For an interface-change stream, use
`--watch-controller-public-key`. These are separate permissions and neither is
implied by inspection. Passing the same public key to both options combines
them into one grant for that controller.

For a wired client to discover an unknown destination behind a Hopspot, add
`--tcp-mode gateway` to the host command. The default PointToPoint mode can serve
known routes but does not recursively forward unknown-path requests. This
forwarding choice does not grant Remote Control access. In a completely fresh
HaLoW lab, explicitly announce each node through its wired controller connection
to establish PRNS neighbor lanes before testing control through another board.
On fresh state, no option means nobody is authorized. These are initial grants;
the runtime can also restore a retained authorization snapshot. Omitting a CLI
key is not a general revocation mechanism for previously persisted grants. The
ready line reports the number of CLI grants, not an effective grant inventory.
For a managed appliance, use the manager’s explicit bounded `qualify` operation
for initial enrollment, then start the normal service without enrollment arguments.
Retained grants remain authoritative; service restarts must not reenroll a revoked
controller. The target's separate remote-control identity is
persisted under `STATE_DIR/remote_control`. Never copy private controller keys
onto targets. Back up private state securely; share only public provisioning data.

Read `target_key` from the target's `hopspot_remote_control_ready` line through a
trusted provisioning channel. Pin it when invoking the controller:

```sh
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" --action describe
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" --action announce
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" --action build
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" --action interfaces
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" \
  --action peers --interface-id "$INTERFACE_ID"
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" \
  --action config --interface-id "$INTERFACE_ID"
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" \
  --action app-message --message-hex 0101
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-key "$TARGET_PUBLIC_KEY" --action watch
```

The example demonstrates the public API sequence: provision trusted target access,
request its control endpoint path, connect and identify, then invoke one typed operation.
Output records are JSON lines on stdout; diagnostics and errors go to stderr. Inventory
and peer results are paged. `--interface-id` is an eight-byte hex identifier taken from
the interface inventory. The application probe uses version byte `01`, operation byte
`01`, and responds with those bytes followed by the verified 16-byte controller hash.
Only an explicitly app-granted controller can reach the handler. Both request and reply
are limited to 96 bytes. Other probe payloads produce an app-level `ApplyFailed` response.
The watch action emits JSON lines for admission and stream frames over 12 seconds.
An initial `ResyncRequired` asks the controller to fetch current snapshots;
later resyncs invalidate them, while heartbeats confirm the stream is live.
Cold-start control does not require the target to periodically
announce first. The operation has a timeout and closes its link afterward; orderly
storage shutdown can extend the process lifetime as described below.
The target's configured self-announcement destination is its node page. HaLoW
announcement delivery continues to use the shared physical broadcast channel;
ordinary directed traffic uses peer channels.

## Public cards and saved targets

The same controller also consumes the shared bounded RemoteControl public-card
codec used by other PRNS clients. Export a controller card for a trusted local
setup tool or an existing administrator:

```sh
controller --state-dir /private/controller-state export-controller > controller.card
```

This exports only public provisioning data. A target access card returned by the
setup tool or administrator must be addressed to this exact controller identity.
Verify its source and intended target before importing it:

```sh
controller --state-dir /private/controller-state import-target \
  --card target-access.card --trusted-source
controller --state-dir /private/controller-state targets
controller --state-dir /private/controller-state invoke \
  --tcp 192.168.12.1:4242 --target-id "$TARGET_ID" --action describe
```

`--trusted-source` confirms the caller's deliberate trust decision; decoding a card
does not authenticate its source. Import checks the recipient against the running
controller and saves through `save_remote_control_target_access_card`. It waits
for the shared authorization transaction's durable storage confirmation before
making the access active. The store is the standard `NodePersistence`/`FileStore`
under `STATE_DIR/state`, not a second application registry. Repeating the same
card is allowed. An import that would change an existing target's authority or
requests returns `Conflict`; the CLI does not silently replace it.

The `target_access_saved` result reports `control_verified: false`. Saving a card
does not create a target-side grant or prove current reachability. A successful
authenticated `describe` verifies that operation with the saved identities.
Stopping a caller after admission does not undo a save, and a lost acknowledgement
can leave its outcome unknown. Inspect retained targets before retrying. The
existing `--target-key` invocation remains a volatile lab path and does not load
or modify this saved-target registry.

The 45-second operation timeout requests orderly shutdown. The process keeps its
state-directory lock until admitted storage work and the final flush settle, so
storage confirmation can extend shutdown beyond that timeout.

An administrator can grant an independent controller Operator access over TCP:

```sh
controller --state-dir /private/admin-state delegate \
  --tcp 192.168.12.1:4242 --target-id "$TARGET_ID" \
  --controller-card recipient.card --trusted-recipient --profile inspect
```

The administrator must already have imported target access permitting
`AuthorizeController`. Verify the recipient through a trusted exchange before
using `--trusted-recipient`. `inspect` requests the existing inspection and
announcement operations; `operate` requests non-administrative operations,
excluding application messages and watch streams. Both are restricted to the
target's authenticated advertised capabilities. Neither profile grants
Administrator authority. The result lists the exact requests.

Only an `Applied` response produces `controller_authorized` output containing a
`target_access_card` field. Transfer that field as text through a trusted channel;
the recipient imports it with its own state directory and verifies control when
connected. No private controller key is copied. A timeout after submitting the
grant does not prove failure, so reconcile its target-side outcome before retrying.
This CLI adds no USB/BLE installer or radio-configuration step.

## Current limits and next API work

- An unauthorized request currently manifests as an exchange timeout in the lab.
  Do not label every timeout as an authorization failure: disconnected targets and
  packet loss are also possible. Preserve silent rejection as the accepted security
  policy: `RemoteControlAdmitError` maps every denial to `Decline::Ignore`. Improve
  controller-local diagnostics without adding a denial response or claiming that
  a timeout proves an authorization failure.
- `AnnounceSelf` covers one configured application destination. It does not select
  a destination set or interfaces, and does not currently announce the separate
  delivery destination. Any broader operation needs an explicit typed API.
- The Linux host currently exposes read-only build/interface/config/peer snapshots.
  A missing radio reading is reported as `Unavailable`. The existing inventory wire
  format uses zero for an absent transfer-rate estimate; the typed API represents it
  as `None`, and the example emits JSON `null`. Do not read an available estimate as
  measured throughput. Radio configuration and service management are later slices.
- The example takes its TCP endpoint explicitly. Saved public-card targets retain
  their identity and permissions across restarts; endpoint discovery and a
  reviewed replacement/recovery UI remain separate work.
- Interface-watch streaming now includes the authenticated subscription request,
  separate grant, bounded Tokio producer and reader, and typed controller API.
  Device qualification and finer invalidation events remain. See
  [the stream contract](remote-control-streaming.md).

## Deployment status

The application, static MIPS build, HaLoW adapter, WebSocket listener and Auto-WiFi
transport have bounded qualifications. This controller workflow replaces the lab
announcement ticker; it is not yet a persistent appliance installer. Remaining
work includes signed manager/bootstrap distribution, physical power-loss and
manager-update qualification, resource qualification, and automated web/helper
transport. The [guided application flow](../../appliance/docs/guided-installation.md)
now connects explicit enrollment to signed slots, service and radio owners. ESP-NOW
interoperability remains separate qualification work. Do not infer completed
radio management or browser provisioning from the available transport APIs.
