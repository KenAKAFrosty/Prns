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
process that joins an existing shared instance does not register it. Stock RNS 1.3.8 `rnstatus -R`
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
`announces_from_internal`, and the common IC/EC tuning values. `mode = internal` follows the RNS
1.3.8 forwarding rules. `outgoing = No` disables egress while retaining ingress.

An explicit `bitrate` overrides the medium estimate and recomputes optimized MTU. TCP client/server
`fixed_mtu` remains authoritative. Network-traversing TCP, UDP, Backbone, and WebSocket media use a
500 Mbps estimate. Auto Wi-Fi and local shared-instance transports use 1 Gbps. Serial derives its
estimate from baud, KISS and AX.25 KISS use 1200 bps, Pipe uses 1 Mbps, and RNode derives LoRa bitrate
from its radio configuration. Every RNodeMulti radio derives its own bitrate and effective policy.
Weave uses stock's 250 kbps estimate and fixed 1024-byte hardware MTU.

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

RNode Bluetooth LE uses the stock Nordic UART Service transport and accepts the same three target
forms as RNS 1.3.8:

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
