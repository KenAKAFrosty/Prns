# Prnsd Configuration

`prnsd` reads the stock Reticulum ConfigObj dialect from one extensionless file named `config`.
It does not read `config.toml`.

Pass `--config DIR` to use `DIR/config`. Without an override, Unix hosts prefer
`/etc/reticulum/config`, then `$HOME/.config/reticulum/config`, and finally
`$HOME/.reticulum/config`. Non-Unix hosts use the corresponding home-directory locations and do
not probe `/etc/reticulum`.

Settings belong under the stock sections:

- `[reticulum]` contains daemon-wide and default interface behavior.
- `[logging]` contains `loglevel` and `logtimestamps`.
- `[interfaces]` contains named `[[interface]]` stanzas.

Root-level settings are rejected. Unknown keys and sections produce source-located warnings with a
suggested spelling. Invalid values, conflicting aliases, missing required settings, unavailable
interfaces, and unavailable RNode transports fail before observability, identity loading, shared
instance election, or interface startup. Each diagnostic names the file, line, full setting path,
offending value, accepted form, and a concrete correction.

Disabled interface stanzas are skipped before `type` and medium-specific validation. This makes it
safe to retain an unavailable or incomplete stanza with `enabled = No`.

## Safe interface editor

`prnsd interfaces` opens a numbered, line-oriented editor when attached to a terminal. The same
surface is available non-interactively through `list`, `validate`, `add`, `edit`, `enable`,
`disable`, `remove`, `repair`, and `apply` subcommands. `check` remains a visible alias for
`validate`. From a checkout, `cargo prnsd interfaces ...` routes
directly to the same one-shot command without starting the managed daemon. `--config DIR` follows
the normal config discovery rules and may appear anywhere inside the `interfaces` command.

The guided screen presents configuration validity, managed-daemon state, friendly interface names,
canonical type names, and enabled state before offering an action. Selecting an interface opens a
typed settings editor containing every runtime-supported setting applicable to that interface
kind. Its initial view contains everyday settings plus any configured overrides; `All settings`
reveals announcement limits, traffic control, and other advanced values without removing them from
reach. Settings are grouped by connectivity, network access, interface behavior, discovery
publication, announcement limits, radio, and traffic control. Configured values appear first in
each group.

Each value is labelled as configured, default, effective, inactive, or required. Defaults and
effective inherited policy are read from the same planned interface used at startup instead of
being inferred from the presence of a config line. Selecting a setting shows what it controls, its
current and default values, accepted units or choices, and any prerequisite that currently leaves
it inactive. Broad stock keys that parse for every interface but have no runtime effect on the
selected kind are not offered as new settings. Existing inert values remain visible as preserved,
unused configuration and can be removed without disturbing unrelated bytes.

Multiple setting changes are validated and previewed as one candidate. RNodeMulti members have
their own nested add, edit, and remove workflow. Unknown third-party interface types remain opaque
and are never interpreted or rewritten.

Interactive output uses the Prns terminal palette for headings, success, warnings, errors, and
prompts. Styling is disabled when output is redirected or `NO_COLOR` is present; nonzero
`CLICOLOR_FORCE` enables it explicitly. The editor remains line-oriented and never clears the
terminal. `validate --details` shows exact source locations and diagnostic codes; the default TTY
view groups concise diagnostics by interface. Non-TTY validation retains detailed plain output for
automation.

Add and edit accept explicit, typed options for the selected interface kind. Inapplicable options
are rejected before an edit exists, and every complete candidate is passed through the same
authoritative planner used by daemon startup. Complete scripted mutations save directly;
`--dry-run` prints the candidate diff without writing, `remove` requires `--yes` without a terminal,
and `--apply` requests live application after a successful save. Guided mutations show a diff and
ask before saving and applying. Passphrases and RPC secrets are redacted from diffs and control
messages unless `--show-secrets` is explicitly selected for local display.

RNodeMulti creation and radio replacement use a repeatable typed
`--radio NAME:VPORT:FREQUENCY:BANDWIDTH:TXPOWER:SPREADING_FACTOR:CODING_RATE` option. The parent
still takes its serial device through `--port`; each emitted child has the canonical required
RNodeMulti settings and is validated with the complete candidate before it can be saved.

The editor preserves comments, blank lines, ordering, newline style, unknown sections, aliases,
disabled third-party interfaces, and all unrelated source bytes. It re-reads the source immediately
before writing to reject concurrent edits. An existing config is copied to
`config.prns-backup` with its permissions preserved, then the replacement is synced and atomically
installed from the same directory. Editing an installation that still uses the built-in default
materializes `DIR/config` as an owner-only file. The command never elevates privileges.

`prnsd interfaces repair --safe` applies only unambiguous semantic repairs, such as disabling an
invalid enabled interface so the rest of the daemon can start. Guided repair can present choices
for ambiguous values and interface types. Malformed ConfigObj syntax is reported with its exact
line and remains untouched. Managed startup never prompts or rewrites configuration; on failure it
prints the matching repair command.

RNS injects `name`, `selected_interface_mode`, and `configured_bitrate` into its in-memory
interface dictionaries. NomadNet can persist those runtime-only fields when writing an interface
back to disk. Prns reports them as cleanup items rather than generic unknown settings; guided
repair groups them into one explained action, and `repair --safe` removes them. Other unknown keys
remain conservative guided repairs and are preserved by default.

`prnsd interfaces apply` sends only the managed generation, a request identifier, and the expected
SHA-256 digest through the private control directory. The daemon re-reads its active config path,
verifies the digest, reparses it, and refuses non-interface changes with `RestartRequired`.
Unchanged interface units remain running; RNodeMulti members reconcile as one hardware unit;
changed listeners are detached before their replacements bind. Failure during construction restores
the previous runtime interface set while leaving the valid on-disk edit intact. Discovery
publication, bootstrap lifecycle, endpoint reservations, and interface-failure monitoring refresh
with the accepted plan. No file watcher is installed: saving and applying remain explicit actions.

Live apply is available only to a managed daemon that owns the routing tables. A shared-instance
client reports that the owning process must apply the change, and an unmanaged foreground process
cannot receive an external request. Exit status 3 means no managed daemon is running. Semantic
validity does not require serial hardware, radios, peers, routers, or remote listeners to be online;
retry-capable interfaces can remain visibly degraded after a valid apply.

## Daemon behavior

Prnsd applies transport enablement and identity policy independently, shared-instance type/name/data
and control ports, RPC key, forced shared bitrate, randomized local hop count, link MTU discovery,
proof form, interface discovery policy, default announce pacing, ingress control, path-request egress
control, every configured `ic_*` and `ec_pr_freq` value, and authenticated remote management.

`panic_on_interface_error` defaults to `No`. With the default, a failed interface remains visible as
degraded while retry-capable interfaces continue supervising themselves. Set it to `Yes` to request
a controlled daemon shutdown after an initial startup failure or a later configured-interface
failure.

The built-in config enables transport routing explicitly. In an operator-supplied config, omitting
`enable_transport` retains stock RNS's `No` default.

Log levels map as follows: 0–1 `error`, 2 `warn`, 3–4 `info`, 5–6 `debug`, and 7 `trace`.
`RUST_LOG` overrides the configured level. `logtimestamps = No` removes daemon-provided timestamps.

Set `enable_remote_management = Yes` to expose the stock
`rnstransport.remote.management` destination with `/status` and `/path` handlers. Every identity in
`remote_management_allowed` must be a 32-character hexadecimal identity hash. Both handlers require
the peer to identify over the link as one of those identities; an empty list permits nobody. The
service is owned only by a standalone daemon or the process that wins shared-instance election. A
process that joins an existing shared instance does not register it. Stock RNS 1.3.9 `rnstatus -R`
and the table/rate forms of `rnpath -R` use these endpoints.

Set `respond_to_probes = Yes` to expose the stock `rnstransport.probe` destination. It refuses link
requests and proves every successfully delivered probe packet. Shared-instance clients never own the
responder. Management destinations announce after 15 seconds and every two hours thereafter,
matching the stock transport lifecycle.

Set `publish_blackhole = Yes` to expose the stock `rnstransport.info.blackhole` destination and
its public `/list` handler. `blackhole_sources` accepts comma-separated 32-character identity
hashes; only a standalone daemon or the shared-instance winner imports those sources. The updater
waits 20 seconds before its first pass, retries unavailable paths every minute, and uses
`blackhole_update_interval` in minutes (60 by default; values below 2 select stock's two-minute
minimum). Imported lists are persisted under `storage/blackhole/<source identity>`, reloaded in
configured order after the local list, and included in this daemon's own published aggregate.
Shared-instance clients neither publish nor import.

## I2P readiness check

From a source checkout, run `cargo prnsd i2p doctor` before enabling peers on an `I2PInterface`.
An installed executable provides the same check as `prnsd i2p doctor`. The doctor connects to the
default SAM bridge at `127.0.0.1:7656`, negotiates SAM 3.1, creates a one-time transient session,
and immediately releases it. It does not persist or print the generated destination credentials. A
successful result proves that the router and SAM session path are available; it does not claim that
the I2P network has finished warming up or that a particular peer is reachable.

Connection failures at the default endpoint distinguish a missing local Java I2P router from a
router whose local console is available but whose SAM bridge is not accepting connections. Protocol
and session failures separately identify an incompatible SAM service or a router that is not yet
ready to create sessions.

Use `cargo prnsd i2p doctor --sam-bridge HOST:PORT` from the checkout, or the equivalent installed
command, for a custom endpoint. Prnsd refuses non-loopback SAM addresses by default because SAM is
plaintext and carries I2P destination credentials. Prefer a loopback endpoint or a secure tunnel to
loopback. `--allow-remote-sam` explicitly acknowledges the risk for a trusted private path; it does
not add encryption or authentication.

Run `cargo prnsd i2p setup` for a non-mutating guided setup. It detects the native operating system,
architecture, and Debian-family Linux where applicable; reruns the doctor; prints the appropriate
official Java I2P installation or SAM-enablement guidance; and emits a validated `I2PInterface`
stanza to place beneath `[interfaces]`. Add repeatable `--peer NAME_OR_DESTINATION` values and
`--connectable` to shape that stanza. An outbound-only stanza without peers is valid but remains
idle. `--open` explicitly opens only the applicable official download page or the local Java I2P
SAM configuration page.

The setup command does not download or execute installers, add package repositories, elevate
privileges, install services, edit configuration, or change router and firewall settings. It keeps
the official artifact, signature, and platform instructions visible for operator review. A
connectable interface creates persistent I2P destination credentials when Prnsd runs; protect and
back up the Prns storage containing them.

## Common interface behavior

Every enabled interface applies `mode`, `outgoing`, `bitrate`, announce cap and rate controls, IFAC
network name/passphrase/size, ingress and egress controls, `recursive_prs`,
`announces_from_internal`, and the common IC/EC tuning values. Interface-mode behavior follows RNS
1.3.9. `outgoing = No` disables egress while retaining ingress.

An explicit `bitrate` overrides the medium estimate and recomputes optimized MTU. TCP client/server
`fixed_mtu` remains authoritative. Network-traversing TCP, UDP, Backbone, and WebSocket media use a
500 Mbps estimate. Auto Wi-Fi and local shared-instance transports use 1 Gbps. Serial derives its
estimate from baud, KISS and AX.25 KISS use 1200 bps, Pipe uses 1 Mbps, and RNode derives LoRa bitrate
from its radio configuration. Every RNodeMulti radio derives its own bitrate and effective policy.
Weave uses stock's 250 kbps estimate and fixed 1024-byte hardware MTU.

### Interface modes

These descriptions and the announce matrix follow the RNS 1.3.9
[`Interface` policy](https://github.com/markqvist/Reticulum/blob/1.3.9/RNS/Interfaces/Interface.py)
and [`Transport` implementation](https://github.com/markqvist/Reticulum/blob/1.3.9/RNS/Transport.py).

Use the canonical RNS names and `mode` configuration values below. Prns also accepts
`interface_mode` as a compatibility key, but new configuration should use `mode`.

| Canonical mode | Configuration | Meaning |
| --- | --- | --- |
| Full | `full` | The default: ordinary announce propagation and seven-day paths. It does not recursively search for unknown paths unless `recursive_prs = Yes`. |
| Point-to-Point | `pointtopoint` or `ptp` | Behaviorally identical to Full in RNS 1.3.9. It identifies the intended topology but imposes no additional routing restriction. |
| Access Point | `access_point` or `ap` | Suppresses automatic announce broadcasts on the interface, recursively resolves paths for attached clients, and expires learned paths after one day. |
| Roaming | `roaming` | Serves a physically mobile segment. It uses six-hour paths, recursively resolves unknown paths, waits an additional 1.5 seconds before answering path requests, refuses to answer with a route learned on the same Roaming interface, and propagates announces conservatively. |
| Boundary | `boundary` | Connects a significantly different or external segment. It controls outbound announce propagation and failed-path recovery; it does not block inbound announce learning. |
| Gateway | `gateway` or `gw` | Uses Full announce and path-expiry behavior while recursively resolving unknown paths on behalf of nodes facing this interface. |
| Internal | `internal` | Represents the inside counterpart to Boundary. It rejects Boundary-sourced announce propagation, recursively resolves outward paths, and can be filtered with `announces_from_internal = No` on other interfaces. |

The following matrix covers automatic propagation of non-local announces. Rows identify the mode of
the interface on which the route was learned; columns identify the outgoing interface mode.

| Learned on / sent out | Full | Point-to-Point | Access Point | Roaming | Boundary | Gateway | Internal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Full | Yes | Yes | No | Yes | Yes | Yes | Yes |
| Point-to-Point | Yes | Yes | No | Yes | Yes | Yes | Yes |
| Access Point | Yes | Yes | No | Yes | Yes | Yes | Yes |
| Roaming | Yes | Yes | No | No | No | Yes | Yes |
| Boundary | Yes | Yes | No | No | Yes | Yes | No |
| Gateway | Yes | Yes | No | Yes | Yes | Yes | Yes |
| Internal | Yes | Yes | No | Yes | Yes | Yes | Yes |

Locally originated announces use every outgoing mode except Access Point. Setting
`announces_from_internal = No` on an interface changes the Internal row to `No` for that outgoing
interface without changing path-request resolution.

Interface modes are Transport policy. They are not authentication, an ingress firewall, or a
substitute for an IFAC network name and passphrase. In particular, Boundary interfaces still learn
valid inbound announces; the mode controls where those announces can be propagated afterward.
`recursive_prs = Yes` enables recursive unknown-path discovery on any mode.

RNS 1.3.9 preserves an explicitly configured Gateway, Access Point, or Internal mode when interface
discovery is enabled. Other discoverable radio interfaces are automatically configured as Access
Point, while other discoverable interfaces are configured as Gateway.

Any future CLI helper or generated example configuration that offers a mode choice should show the
canonical name, accepted configuration value, and corresponding description together. Ordinary
status output remains compact and displays only the canonical mode name.

## Existing interface backends

| Stock interface | Applied configuration |
| --- | --- |
| `AutoInterface` | Group ID, multicast scope/address type, discovery and data ports, allowed and ignored devices, and common policy. |
| `TCPClientInterface` | Target, port, KISS framing, I2P socket discipline, connect timeout, reconnect limit, fixed MTU. |
| `TCPServerInterface` | Port aliases, address/device binding, IPv6 preference, KISS framing, I2P socket discipline, fixed MTU. Accepted members inherit the full policy and IFAC access. |
| `UDPInterface` | Receive-only, send-only, or bidirectional endpoints; shared port alias; device broadcast resolution. |
| `BackboneInterface` / `BackboneClientInterface` | Listener or client role, aliases, listener address/device binding and IPv6 preference, plus client I2P socket discipline, timeout, and retry limit. |
| `SerialInterface` | Device, speed, data bits, parity, and stop bits. |
| `KISSInterface` | Serial line, TNC timing/CSMA, READY flow control, and station identification. |
| `AX25KISSInterface` | KISS settings plus validated callsign and SSID. |
| `RNodeInterface` | Serial, `tcp://host`, or Bluetooth LE RNode transport; radio settings, READY flow control, station identification, and airtime limits. TCP uses stock's fixed port 7633; `tcp://` selects loopback. |
| `RNodeMultiInterface` | One serial device with nested, independently routed radio interfaces; per-radio LoRa settings, READY flow control, airtime limits, policy, and coordinated reconnect. |
| `PipeInterface` | Parsed subprocess command and typed respawn delay. |
| `I2PInterface` | Validated `.i2p` names or base64 destinations in `peers`, optional inbound reachability through `connectable`, and common policy and IFAC access. |
| `WeaveInterface` | A 3,000,000-baud 8N1 serial WDCL connection with authenticated device discovery, one inherited-policy member per device endpoint, peer timeout, and multipath deduplication. |

Enabled interface types without a backend fail with “not available in this build.” RNodeMulti
remains a local serial-device interface.

## Prns-owned host interface backends

Prnsd also accepts these canonical interface types, which stock RNS 1.3.9 does not implement:

| Canonical type | Applied configuration |
| --- | --- |
| `PrnsUsbAuto` | Discovers and supervises supported USB CDC devices. Only common interface policy is required. |
| `PrnsBluetoothAuto` | Discovers and supervises Prns Bluetooth LE peers. Only common interface policy is required. |
| `PrnsWebSocketClient` | Connects to the required plain `ws://` URL in `target` and retries after disconnects. |
| `PrnsWebSocketServer` | Listens on `port` or `listen_port`, with optional `listen_ip`, `device`, and `prefer_ipv6`. Accepted members inherit the full policy and IFAC access. |

Prns-owned type values are ASCII case-insensitive. The explicit `Interface` suffix is accepted as
an alias, and `Bluetooth` and `Ble` are interchangeable. For example, `prnsusbauto`,
`PrnsUsbAutoInterface`, `PRNSBLEAUTO`, and `PrnsBleAutoInterface` normalize to the canonical names
above. Stock interface type values remain case-sensitive. USB Auto and Bluetooth Auto each permit
one enabled stanza because each stanza represents the host's single aggregate device fleet;
WebSocket client and server stanzas may be repeated.

```ini
[interfaces]
  [[USB devices]]
    type = PrnsUsbAuto
    enabled = Yes

  [[Nearby Prns peers]]
    type = PrnsBluetoothAuto
    enabled = Yes

  [[WebSocket uplink]]
    type = PrnsWebSocketClient
    enabled = Yes
    target = ws://peer.example:4242/prns

  [[WebSocket listener]]
    type = PrnsWebSocketServer
    enabled = Yes
    listen_ip = ::
    listen_port = 4242
    prefer_ipv6 = Yes
```

Stock RNS treats these names as external interface module names. If no matching module file exists,
it logs the missing module, skips the stanza, finishes bringing up its other interfaces, and keeps
running. Auto Wi-Fi continues to use stock `AutoInterface`. Wi-Fi Direct and Wi-Fi Aware do not yet
have accepted config types because Prnsd cannot construct their required platform adapters.

RNode Bluetooth LE uses the stock Nordic UART Service transport and accepts the same three target
forms as RNS 1.3.9:

```ini
port = ble://
port = ble://RNode 1234
port = ble://AA:BB:CC:DD:EE:FF
```

The empty target selects the first paired device advertising the RNode service whose name starts
with `RNode `. A name target must match exactly. A hexadecimal address target is supported on Linux
and Windows; macOS Core Bluetooth does not expose device MAC addresses, so macOS configurations must
use automatic or exact-name selection. Pair the RNode with the operating system before starting
Prnsd and grant the daemon Bluetooth access when the platform asks. Missing adapters, permissions,
pairing, services, and characteristics produce repair-focused interface errors. With the default
`panic_on_interface_error = No`, Prnsd remains degraded and retries the connection every five
seconds.

RNodeMulti radios are nested beneath their physical device. Each enabled child requires a unique
`vport` and complete radio configuration:

```ini
[interfaces]
  [[Dual Radio]]
    type = RNodeMultiInterface
    enabled = Yes
    port = /dev/ttyACM0

    [[[Sub-GHz]]]
      interface_enabled = Yes
      vport = 0
      frequency = 868000000
      bandwidth = 125000
      txpower = 7
      spreadingfactor = 8
      codingrate = 5

    [[[2.4 GHz]]]
      interface_enabled = Yes
      vport = 1
      frequency = 2400000000
      bandwidth = 812500
      txpower = 10
      spreadingfactor = 7
      codingrate = 6
```

AutoInterface defaults to group `reticulum`, link scope, temporary multicast addressing, discovery
port 29716, and data port 42671. `discovery_scope` accepts `link`, `admin`, `site`, `organisation`,
or `global`; `multicast_address_type` accepts `temporary` or `permanent`. A custom group changes
both the multicast group and peer-authentication token. `devices` is an allowlist when present,
`ignored_devices` always wins, and loopback devices are never selected.

An interface with `bootstrap_only = Yes` starts normally while no auto-connected discovered
interface is available. When the configured `autoconnect_discovered_interfaces` limit is full,
Prnsd retires all bootstrap-only interfaces. It restores them after every auto-connected interface
is gone. As in RNS, this lifecycle is inactive when discovery auto-connect is disabled.

Weave uses a serial device path in `port`, not a numeric network port:

```ini
[interfaces]
  [[Weave]]
    type = WeaveInterface
    enabled = Yes
    port = /dev/ttyACM0
```

Prnsd authenticates WDCL discovery with an ephemeral Ed25519 identity, creates one routed member
for each endpoint reported by the attached Weave device, and supervises reconnects. Members inherit
the parent interface's common policy and IFAC access. Without an attached device, the default
`panic_on_interface_error = No` behavior keeps the daemon visibly degraded and retrying.

## Explicit follow-ons

Recognized settings that belong to planned follow-on work emit `unsupported_setting` warnings at
their exact source lines. They are never silently ignored:

- `ignore_config_warnings` is not honored; Prnsd always reports configuration problems.

Role-inapplicable Backbone settings also warn instead of disappearing. Listener stanzas do not use
client-only `target_port`, `i2p_tunneled`, `connect_timeout`, or `max_reconnect_tries`; client stanzas
do not use listener-only `listen_ip`, `listen_port`, `listen_on`, or `device`.
Discovery publication details similarly warn when `discoverable` is absent or set to `No`.

Unavailable backends are not partial plans: enabling one is a configuration error until that
backend exists.

## Minimal router

```ini
[reticulum]
  enable_transport = Yes
  share_instance = Yes
  panic_on_interface_error = No

[logging]
  loglevel = 4
  logtimestamps = Yes

[interfaces]
  [[LAN]]
    type = AutoInterface
    enabled = Yes

  [[Uplink]]
    type = TCPClientInterface
    enabled = Yes
    target_host = peer.example.com
    target_port = 4242
    connect_timeout = 5
```
