# Headless Personal Hopspot

A small host entry point for the shared Hopspot destinations and NomadNet pages.
It uses the Tokio runtime with a current-thread executor, a Reticulum TCP server,
and recipe-managed identity/route/ratchet persistence. The listen address and
private state directory must be supplied explicitly.

TCP is always available. The optional `wifi-halow` feature attaches a native
HaLoW interface on Linux; it does not configure the radio. HTTP
and remote administration are not exposed by this entry point. Auto-WiFi is
optional and requires explicit network-device selection.

## Build and run

From the repository root:

```sh
cargo build --locked --release --manifest-path personal-hopspot/headless/Cargo.toml
personal-hopspot/headless/target/release/personal-hopspot-headless \
  --state-dir ./hopspot-state --listen 127.0.0.1:4242
```

Choose an interface address reachable by your peers for a network-facing node.
The port carries Reticulum over HDLC-framed TCP, not a web interface. The
`hopspot_ready` line reports the bound address and node-page destination.
Clients can request a path to that destination; the TCP-only host does not periodically
announce to the network. Only attach it to networks you intend to participate in.

The state directory contains secret identity and vault material. On Unix the
directory is restricted to its owner. A process-held file lock prevents two
hosts from using the same state concurrently; the lock file can remain after
exit. Corrupt identity files cause startup to fail rather than silently replacing
the identity. The runtime batches retained-state writes, saves critical state,
and flushes on graceful shutdown. SIGINT and Unix SIGTERM request that shutdown.
`--run-for SECONDS` provides a positive-duration, bounded run.

Use a dedicated state directory. State in `/tmp` survives a process restart but
is lost at reboot on OpenWrt. Production installation must deliberately choose
persistent storage, available space, flash-write policy, and a service account.

## Optional WebSocket listener

Build with `--features websocket` (or `--features wifi-halow,websocket`) and opt
in to a plain WebSocket listener:

```sh
./personal-hopspot-headless --state-dir ./hopspot-state \
  --listen 127.0.0.1:4242 --websocket-listen 127.0.0.1:8081 \
  --websocket-connections 8
```

Use the board's reachable LAN address instead of loopback for remote browsers.
The listener carries one raw Reticulum packet per binary WebSocket message;
configure the browser SDK with `framing: "RawPacket"`. The
`hopspot_websocket_ready` line reports the actual bound address. It shares the
same node, destinations, routing, and persistence as TCP and HaLoW, so a browser
can request a remote HaLoW destination through this listener.

The default limit is eight total accepted sessions and pending handshakes.
Extra connections wait in the OS backlog; they are not promised an immediate
HTTP rejection. Existing ten-second handshake deadlines and frame/message bounds
still apply. Disconnecting a session releases its admission slot.

This is a public Reticulum transport endpoint, accepting browser origins; it
exposes no administrative commands or HTTP assets. It does not provide TLS.
Use only the intended network exposure. An HTTPS-hosted browser application's
ability to reach plain `ws://` and local devices needs its own browser deployment
qualification. The original smoke served assets from laptop loopback;
a [separately built static browser bundle](docs/browser-hosting.md) has also been
qualified on the G4 using its existing HTTP server and the portable crypto path.
That deployment remains temporary RAM hosting, not persistent installation.

The G4 build task enables this only with `--with-websocket`. Its server-only
feature excludes the TLS client dependencies; the existing `personal-rns/websocket`
feature still supplies the full client/server transport. See the
[browser-to-HaLoW qualification](docs/qualification/websocket-halow-2026-09-29.md)
for the reproducible smoke, measured size/RAM, and limitations.

## Optional Auto-WiFi discovery

Build with `--features wifi-auto` and repeat `--auto-wifi-device NAME` for each
intended LAN. Omitting the option leaves Auto-WiFi disabled, even when compiled in.
The G4 build task includes this only with `--with-auto-wifi`.

```sh
./personal-hopspot-headless --state-dir ./hopspot-state --listen 127.0.0.1:4242 \
  --auto-wifi-device br-lan
```

Use the actual address-owning LAN device, not an enslaved bridge port. The
`hopspot_auto_wifi_configured` line means the supervisor was attached, not that
an interface exists, discovery succeeded, or peers are connected. Devices can
appear later. Native DNS-SD and the existing Reticulum link-local multicast
protocol share the same device selection; the existing Auto-WiFi runtime owns
TCP rendezvous, peer admission, and discovery lifecycle. This does not advertise
the independent WebSocket listener or make browser mDNS available.

Device selection scopes discovery and local-subnet admission; it is not a
firewall. The rendezvous service binds separate IPv4 and IPv6 wildcard sockets. Keep OpenWrt firewall
policy explicit, especially on routed or overlapping networks. Selecting a bridge
includes its whole broadcast domain, including any attached HaLoW radio.
See [AP/client networking](docs/networking.md) before enabling it on the G4.
The [initial hardware smoke](docs/qualification/auto-wifi-g4-2026-09-29.md) passed
a direct rendezvous transfer. The [DNS-SD follow-up](docs/qualification/auto-wifi-dnssd-g4-2026-09-29.md)
qualifies native browse/resolve and transfer to the advertised endpoint.

## Experimental HaLoW attachment

Build with `--features wifi-halow` (the G4 build task includes it). On an already
configured HaLoW device:

```sh
./personal-hopspot-headless --state-dir /tmp/hopspot-halow-state \
  --listen 127.0.0.1:4343 --halow-device wlan0 --halow-scope primary-halow \
  --halow-peers 16 --halow-idle-seconds 900
```

This requires Linux and `CAP_NET_RAW` (the vendor lab OS runs as root). Device and
scope must be supplied together. The scope is a stable, local 1–64-byte name;
preserve it across Linux interface renames. Omitting both retains TCP-only operation.
Socket binding fails startup before touching persistent state. The experimental
EtherType is `0x88b6` with the versioned Prns HaLoW envelope; both endpoints must
use this implementation. No radio profile or vendor service is modified.

Application announcements require an explicit Remote Control API command; there is
no startup, reconnect, or periodic announcement policy. See [remote control](docs/remote-control.md)
for controller provisioning and cold-start commands. Idle HaLoW peers expire after
`--halow-idle-seconds` (default 900), checked every five seconds. The peer cap defaults to 16. Each peer additionally owns bounded
runtime queues, so raise this cap only with a measured memory budget. Pacing estimates
are 7.3 Mbps unicast and 4 Mbps broadcast for the measured MCS2 profile. These are
payload estimates, not commands that set MCS or transmit power.

The `fetch_page` probe can use the radio instead of TCP:

```sh
./fetch_page --halow-device wlan0 --halow-scope probe-halow \
  --destination "$HOPSPOT_DESTINATION"
```

It waits for a radio peer, requests a path, establishes a link, and verifies the
whole page within 60 seconds. In a bounded lab test, attach the probe, then issue
`AnnounceSelf` from the authorized controller. A G4 development bundle can include
the probe with `./tools/prns run build.hopspot.g4 -- ... --with-probe`.

## Verify a running host

Build and run the companion probe from a separate machine, substituting the
host's advertised destination hash:

```sh
cargo run --locked --manifest-path personal-hopspot/headless/Cargo.toml \
  --example fetch_page -- --target 192.168.12.1:4242 \
  --destination "$HOPSPOT_DESTINATION"
```

Within a 60-second deadline it connects, requests the destination's path,
establishes a Reticulum link, requests the index, and compares the complete
MessagePack response with the shared Hopspot page. Use matching source versions
on both ends; a changed page should fail this exact-content check.

```sh
cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml
cargo clippy --locked --manifest-path personal-hopspot/headless/Cargo.toml --all-targets -- -D warnings
```

Lifecycle tests exercise real localhost sockets, clean shutdown, identity
retention, rejection of concurrent writers, and preservation of a damaged
identity file. They require permission to bind localhost sockets.

## HaLoW Linux appliances

The [web installation guide](docs/g4-installation.md) is also embedded in the
website's `/flash/thinknode-g4` page. The
[Heltec HT-HD01-V2 guide](docs/heltec-installation.md) is embedded at
`/flash/heltec-ht-hd01-v2`. The [shared deployment specification](docs/halow-deployment.md)
describes the next application-installation transaction and recovery gates. The
[HaLoW integration contract](docs/halow-integration.md) records the proposed
transport semantics, radio default, additional interfaces, and qualification work.

Each of those pages leads with a short version ([G4](docs/g4-quick-installation.md), [Heltec](docs/heltec-quick-installation.md), [shared steps](docs/halow-quick-installation.md)) and keeps the full guide beneath it as the extended guide. The full guides stay authoritative: change a procedure there first, then carry it into the short version.

See the [bring-up and installer procedure](docs/thinknode-g4.md) for the tested
cross-build, hardware evidence, temporary deployment, and remaining requirements
before offering persistent installation or firmware flashing to other users.
