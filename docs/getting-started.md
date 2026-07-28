# Getting Started

This guide takes you from a fresh clone to a running Reticulum node, one real result at a time. Everything here works on an ordinary laptop or desktop. You don't need special hardware.

## Check your setup

From the repository root:

```console
./tools/prns doctor getting-started
```

The doctor checks for the handful of tools this guide uses, and tells you what's missing and how to get it. It only reports; it never installs or changes anything on your machine. (On Windows, use `tools\prns.cmd` instead of `./tools/prns`.)

This is also your first look at `./tools/prns`, the repository's task runner. `./tools/prns list` shows everything else it can do. None of that additional functionality is necessary for this guide, though.

## Hear your first announce

Once your Rust toolchain is set up, you can use the `cargo tools` shortcut for the same tools runner.

```console
cargo tools guide rust
```

The first build may take a few minutes (incremental builds after the first one are fast). The run itself is over in about a second. You should see something like:

```console
Node A: TCP server listening on 127.0.0.1:51990
Node B: TCP client only (no radio or USB discovery)
Success: Node B observed Node A's real Reticulum announce on InterfaceId([1, 14, 21, 39, 95, 182, 20, 1]) (Some(TcpClient)).
Node B interface inventory:
  InterfaceId([1, 14, 21, 39, 95, 182, 20, 1]) connection=Connected rx=188 tx=0
```

Let's break those down a bit:
- The example created two nodes
- Each node generated a fresh Identity on the spot. 
- Node A registered a Destination and began announcing it. 
- Node B, which connected over a localhost TCP Interface, heard the signed Announce, verified it, and reported which interface carried it. 

Each of the [six terms](/README.md#new-to-reticulum) you learned did its job, live, on your machine.

## Read the code that did it

The whole program is one file: [`personal-rns/examples/node_basics.rs`](../personal-rns/examples/node_basics.rs). It's worth five minutes, because its shape is the shape of every Prns app:

- A `PrnsNodeRecipe` declares everything the node is: its destinations, its storage, its event handler, its interfaces. Every field is required, so if it compiles, nothing was forgotten.
- The recipe yields a node and a handle. The node runs; the handle is how the rest of your program talks to it, from issuing commands to attaching interfaces mid-flight.
- Events arrive as plain values in your `on_event` function. Node B's entire success condition is listening for `AnnounceHeard` and checking who it heard.

Describe the node, run it, react to events. That's the posture everything else builds on.

## Drop the wires

That first run stays on localhost on purpose, and its code wires Node B to Node A's port by hand. The follow-up example, [`auto_discovery.rs`](../personal-rns/examples/auto_discovery.rs), deletes that wiring. Neither node is given any address; both simply turn on Wi-Fi auto-discovery:

```console
cargo run --locked -p personal-rns --example auto_discovery --features tokio-host,wifi-auto
```

The run succeeds when Node B hears Node A's announce anyway. On one machine they meet through a local rendezvous port, the same door a second Prns app on your device would use to join the mesh. Across two machines it's genuine multicast discovery over your LAN: after its first success, the example keeps listening for a minute, so run the same command on a second computer on the same network and watch each machine print the other's announce. (Your OS may ask you to approve local-network access the first time.)

## Choose your path

You've now seen a node born, announced, and heard. Where next depends on what you're building:

- **Building an app?** The [example catalog](examples.md) ladders up from here: request and response, resource transfer, changing interfaces on a live node. Then [application integration](application-integration.md) covers owning a node inside a long-lived program, in Rust or any of the SDK languages. When you're ready to leave the clone, the crate is a `cargo add personal-rns` away.
- **Running a node for the ecosystem?** [`prnsd`](../prnsd/README.md) is the daemon: it owns the interfaces on a machine, and every Reticulum app on that machine shares its one instance.
- **Putting it on hardware?** [Flash a Hopspot](https://prns.dev/flash) in minutes, or work through the [embedded guide](embedded.md) to build board firmware from source.

Working on Prns itself? [CONTRIBUTING.md](../CONTRIBUTING.md) is your door.
