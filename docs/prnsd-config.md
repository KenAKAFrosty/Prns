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
control, and every configured `ic_*` and `ec_pr_freq` value.

`panic_on_interface_error` defaults to `No`. With the default, a failed interface remains visible as
degraded while retry-capable interfaces continue supervising themselves. Set it to `Yes` to request
a controlled daemon shutdown after an initial startup failure or a later configured-interface
failure.

The built-in config enables transport routing explicitly. In an operator-supplied config, omitting
`enable_transport` retains stock RNS's `No` default.

Log levels map as follows: 0–1 `error`, 2 `warn`, 3–4 `info`, 5–6 `debug`, and 7 `trace`.
`RUST_LOG` overrides the configured level. `logtimestamps = No` removes daemon-provided timestamps.

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
| `RNodeInterface` | Serial RNode radio settings, READY flow control, station identification, and airtime limits. |
| `RNodeMultiInterface` | One serial device with nested, independently routed radio interfaces; per-radio LoRa settings, READY flow control, airtime limits, policy, and coordinated reconnect. |
| `PipeInterface` | Parsed subprocess command and typed respawn delay. |
| `I2PInterface` | Validated `.i2p` names or base64 destinations in `peers`, optional inbound reachability through `connectable`, and common policy and IFAC access. |

Enabled interface types without a backend fail with “not available in this build.” RNode `tcp://`
and `ble://` URIs fail similarly; the current RNode and RNodeMulti backends require a local serial
device.

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

## Explicit follow-ons

Recognized settings that belong to planned follow-on work emit `unsupported_setting` warnings at
their exact source lines. They are never silently ignored:

- `ignore_config_warnings` is not honored; Prnsd always reports configuration problems.
- Remote management and ACL settings, probe responses, and network blackhole exchange remain
  separate daemon services.

Role-inapplicable Backbone settings also warn instead of disappearing. Listener stanzas do not use
client-only `target_port`, `i2p_tunneled`, `connect_timeout`, or `max_reconnect_tries`; client stanzas
do not use listener-only `listen_ip`, `listen_port`, `listen_on`, or `device`.
Discovery publication details similarly warn when `discoverable` is absent or set to `No`.

Weave, RNode TCP/BLE transport, and other unavailable backends are not partial plans: enabling one
is a configuration error until that backend exists.

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
