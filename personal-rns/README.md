# Personal RNS

`personal-rns` is the application-facing Rust crate for Prns. It curates the
pure protocol engine, high-level Tokio and Embassy node runtimes, storage
profiles, and interface families behind one feature-selected API. It is a
workspace crate and is not currently published, so depend on it by path from
this clone.

All public packages use the same engine, release version, and dual
MIT/Apache-2.0 license. The hosted reference at
[reticulum.rs](https://reticulum.rs) supplements the guidance kept in this
clone.

## Run the executable contract

```console
cargo tools guide rust
```

The checked example starts two nodes with identities minted from OS entropy.
Node A exposes only a TCP server on `127.0.0.1` with an OS-selected port. Node B
connects through an explicit TCP client, observes Node A's real announce,
prints the ingress interface, and exits. A ten-second deadline produces a clear
failure instead of hanging.

The command expands to:

```console
cargo run --locked -p personal-rns --example node_basics \
  --features tokio-host,tcp,wifi-auto,usb,bluetooth-auto
```

Add `-- --with-auto` to either form to attach Node B's Wi-Fi, USB, and Bluetooth
auto interfaces. This is opt-in because it may multicast, open compatible USB
devices, and advertise or request Bluetooth permission. TCP still owns the
success condition.

## Choose features intentionally

| Need | Features |
| --- | --- |
| Pure engine and standard-library storage | defaults |
| Native async application node | `tokio-host` |
| Embedded async node | `embassy-host` |
| TCP, UDP, serial, or radio family | its named feature, such as `tcp` |
| Native automatic media | `wifi-auto`, `usb`, `bluetooth-auto` |
| Bounded embedded storage profile | `external-alloc` or the relevant storage type |

Interface features select code; they do not silently attach hardware. A node
recipe's `interfaces` field owns attachment policy.

## Read a node recipe

`PrnsNodeRecipe` makes the application contract visible in one value:

- `transport_identity` decides whether the node routes on behalf of others.
- `pre_configured_destinations` owns application identities, names, proof
  policy, link policy, ratchets, and request handlers.
- `storage` selects the memory/storage profile.
- `routes` declares request routes.
- `app_state` and `on_event` connect protocol events to application state.
- `interfaces` attaches explicit interfaces or a reusable attachment intent.

`PrnsNode::new(recipe)` returns the running node boundary. `node.handle()` is a
cloneable control and inspection handle: issue engine commands, add or remove
interfaces, supervise listener families, and read live interface snapshots.
`node.run().await` owns the runtime tasks and normally runs for the lifetime of
the application.

`PrnsEvent::Message` carries application data. `PrnsEvent::Diagnostic` carries
owned observations such as announces, command settlement, route changes, and
link lifecycle. The example listens for
`Diagnostic::AnnounceHeard { destination, source_interface, .. }`.

## Build your own consumer

For another crate inside this repository:

```toml
[dependencies]
personal-rns = { path = "../personal-rns", features = ["tokio-host", "tcp"] }
```

Adjust the relative path for the consumer's location. Keep identities outside
source control; the example intentionally creates ephemeral identities, while
long-running applications should use the runtime's load-or-create identity
helpers with private application storage.

Generate and open the local API reference:

```console
cargo doc --locked -p personal-rns --features tokio-host,tcp --open
```

For a managed general-purpose node instead of an application runtime,
use the [Prnsd guide](../prnsd/README.md).

For a real `no_std` consumer, follow the
[embedded guide](../docs/embedded.md). It builds the XIAO ESP32-C6 Hopspot and
walks from the minimal board entrypoint through hardware bring-up, fixed
storage, `PrnsNodeRecipe`, and concrete USB/radio interfaces.
