# G4 DNS-SD and IPv6 rendezvous qualification

Native DNS-SD browse/resolve and complete Hopspot page transfers to the advertised
IPv6 endpoint passed on the G4 with macOS as the independent client. Existing
IPv4 rendezvous also passed. [Structured evidence](auto-wifi-dnssd-g4-2026-09-29.json)
contains build identity, filtered before/after packet captures, native discovery
output, and the final exact-page results.

## Faults and fixes

The vendor image gives `eth0` and the intended `br-ahwlan` bridge the same IPv6
link-local address. The previous mDNS configuration selected an address;
mdns-sd resolves that selector to the first matching OS interface. The baseline
capture showed IPv6 service probes and announcements leaving raw `eth0`, while
IPv4 queries used the intended bridge. Earlier bridge-only captures therefore
missed the announcements.

The native discovery adapter now retains both the address and device policy in
an interface predicate. Duplicate addresses cannot select an excluded bridge
port. The final bridge capture contains PTR/SRV/TXT/AAAA publications for both
`_reticulum._tcp.local.` and `_reticulum._udp.local.`.

The DNS-SD contract publishes IPv6 link-local endpoints, but the automatic TCP
rendezvous listener previously bound only IPv4. It now claims separate IPv4 and
IPv6 sockets on port 42699 and accepts either through the existing shared
admission budget. Link-local admission checks the receiving interface scope;
IPv4-mapped peers from caller-supplied listeners normalize to IPv4.

A single dual-stack socket was insufficient on macOS: it could coexist with an
incumbent IPv4 listener, violating central/satellite ownership. Separate claims
preserve that ownership rule and release IPv4 if IPv6 binding fails. The existing
central/satellite takeover test now passes. Caller-supplied listeners retain
the caller's address-family choice. Automatic binding requires both families;
an IPv6-disabled kernel is not qualified by this run.

## Final hardware run

Application built from `e31518647` plus these changes, using the existing pinned
Rust/Zig build task with `--with-auto-wifi --with-websocket --with-probe`.
Static MIPS32r2 O32 validation passed. Application: **5,334,876 bytes**, SHA-256
`a11baef5eaec55c06bd4d6f0bcab50f5049cf94ed84d92f1172142036ac790ee`.

The application ran from a private `/tmp` directory with an explicit
`--auto-wifi-device br-ahwlan` and a 100-second automatic exit deadline. No radio,
UCI, firewall, or persistent-service changes were made.

The independent Mac used native `dns-sd` to browse `_reticulum._tcp.local.`,
resolve the advertised instance, and obtain its AAAA address and receiving
interface index. The resulting target was
`[fe80::e638:19ff:fe1f:5248%7]:42699`. The index is local to that Mac, not the G4.
The existing `fetch_page` probe verified all **3,542 bytes** of the shared page
through this endpoint, then repeated the transfer to IPv4 `192.168.12.1:42699`.

Final probe-reported request RTTs were **107 ms (IPv6)** and **120 ms (IPv4)**.
An earlier two-listener run returned 571/1,514 ms while temporary artifacts were
accumulating; a prior implementation smoke returned 73/53 ms. These short runs
had uncontrolled host load and are not a performance comparison; no latency or
throughput improvement is claimed. Browser assets and WebSocket listening were
not required for this test.

The temporary process flushed persistence and stopped. Its ordinary TCP,
rendezvous, discovery, and data listeners were gone at cleanup; original services
remained and no pending UCI changes were reported. The pre-existing TCP Hopspot
also passed its exact-page probe afterward (3,542 bytes, 81 ms).

Repeated RAM uploads accumulated several 5.3 MB executables. One final-package
upload failed and a short SSH connection timed out; a longer connection succeeded.
Removing only stopped lab executables and unused browser assets restored space.
The exact packaged binary then passed the final run above, and its stopped copy
was removed too. After cleanup, `/tmp` had 23,788 KiB free and reported available
RAM was 12,060 KiB. No reboot or deletion of identity/state was required.
This is direct evidence that temporary installations also need a storage budget.

## Reproduce

Build through `./tools/prns run build.hopspot.g4` with the qualified toolchain,
verify the uploaded hash, and start a bounded RAM host:

```sh
./personal-hopspot-headless --state-dir ./state --listen 192.168.12.1:4345 \
  --auto-wifi-device br-ahwlan --run-for 100
```

On macOS, use `dns-sd -B _reticulum._tcp local.`, then `dns-sd -L INSTANCE
_reticulum._tcp local.` and `dns-sd -G v6 HOSTNAME` using the returned values.
Stop each discovery command after collecting its result. Use a terminal/PTY;
plain captured stdout can remain buffered and appear empty on termination.
Pass the returned IPv6 address and numeric receiving interface index to
`fetch_page --target '[ADDRESS%INDEX]:42699' --destination NODE_PAGE`.
NODE_PAGE comes from the running host's ready line; DNS-SD advertises a transport,
not a Reticulum destination identity. Repeat with the IPv4 target for compatibility.

The board lacks `timeout`; bound captures with their own PID, shell timer, and
SIGINT, and bound the host with `--run-for`. Do not kill unrelated services.

## Regression coverage and limits

The Tokio interface crate passed **80 Auto-WiFi unit tests**, with one existing
IPv4 UDP contract test still ignored. New coverage checks duplicate-address
selection, both listener families, exclusive ownership, partial-bind rollback,
IPv6 interface scope, and mapped IPv4 admission. The public discovery snapshot
and central/satellite lifecycle integration test passed; all-target Clippy passed
with warnings denied. Tests ran on macOS with local socket access. The synthetic
LAN fallback fixture now excludes unrelated host NICs whose indices could collide.

Commands used `cargo test --offline --locked --manifest-path
prns-interfaces/impls/tokio/Cargo.toml --features wifi-auto-mdns --lib wifi_auto`,
the same manifest/features with `--test wifi_auto_discovery`, and Clippy with
`--all-targets -- -D warnings`. Debug symbols/incremental output were disabled
to fit the development disk. The Linux, Windows, and Android suites were not run.

This qualifies native OS discovery followed by a real Reticulum transfer to the
resolved endpoint, not full unattended Prns peer selection on the board. The
vendor bridge still includes Ethernet, HaLoW, and the inactive 2.4 GHz AP port.
Separate AP/client networking, multicast isolation, two-client traffic, automatic
peer churn, service withdrawal, sustained resource bounds, and permanent
installation remain follow-up work. See [networking](../networking.md).
