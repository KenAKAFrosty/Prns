# Controlled two-hop Hopspot qualification

The regression in [`halow_multihop.rs`](../../tests/halow_multihop.rs) runs three
real Hopspot nodes, each with a distinct identity, the production HaLoW supervisor,
production peer/broadcast policies, and the shared node-page Resource handler.
Only the datagram medium is simulated. Its topology has A–B and B–C edges, with
no A–C edge, for both broadcast and unicast. The same relay uses a single HaLoW
supervisor for both neighbors; this exercises forwarding between peer channels
on one radio, rather than relying on two unrelated interfaces.

The test verifies:

- With the relay disconnected, an endpoint announce admits no remote endpoint
  peer, and a path request cannot complete within the negative-control window.
- With the relay connected, both endpoints concurrently retrieve the exact
  shared Hopspot page, including its packed response header and Resource transfer.
- Each endpoint's sole HaLoW peer ID is derived from the relay MAC and local
  instance scope. The destination identity is not mistaken for its next hop.
- Ordinary relayed announces use broadcast; non-announce traffic uses unicast,
  and the relay sends direct traffic toward both endpoints.
- Disconnecting the relay again prevents a fresh transfer from completing even
  with previously learned routes.

The bounded medium fails the test if its queue fills rather than quietly dropping
traffic. A twenty-second overall deadline catches hangs. The negative checks use
short timeouts; they prove no immediate bypass in this controlled topology, not
production route-expiry timing or failure-recovery performance.

Run with:

```sh
cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml \
  --features wifi-halow --test halow_multihop
```

The existing `validation/platforms/linux-halow.sh` suite includes this test through
its headless `--all-targets` command. The initial qualification ran on macOS;
this medium does not exercise Linux AF_PACKET, Morse firmware, RF collisions,
retries, hidden terminals, or physical range. The preceding three-board captures
qualify the real packet adapter and radio delivery separately.

## Remaining hardware step

Require independently verified endpoint isolation before treating a bench chain
as a forced radio route. Merely seeing a nonzero announce hop count in a fully
connected mesh is insufficient. A filtering experiment must first prove that
neither endpoint can deliver any Prns EtherType traffic directly to the other,
including broadcast, while each still reaches the relay. Then repeat both page
transfers, capture both relay legs, and remove the relay as a negative control.
Use fresh state to exclude retained routes, preserve wired management, and keep
independent rollback on all boards. Physical shielding/attenuation or separation
can establish a real RF chain; software isolation alone cannot qualify RF range
or hidden-terminal behavior.
