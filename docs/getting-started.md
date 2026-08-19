# Getting Started

This guide takes you from a fresh clone of the repo, to a running Reticulum node, in just a few minutes. 

Everything here works on an ordinary laptop or desktop. You don't need special hardware.

On Windows, enable long paths once before cloning — `git config --global core.longpaths true` — or clone into a short directory such as `C:\prns`. Some benchmark evidence files in the repository exceed Windows' default 260-character path limit, and without this setting the clone stops partway through checkout.

## Check your setup

From the repository root:

```console
./tools/prns doctor getting-started
```

The doctor checks for the handful of tools this guide uses, and tells you what's missing and how to get it. It only reports; it never installs or changes anything on your machine. (On Windows, use `.\tools\prns.cmd` instead of `./tools/prns`.)

This is also your first look at `./tools/prns`, the repository's task runner. `./tools/prns list` shows everything else it can do. None of that additional functionality is necessary for this guide, though.

## Hear your first announce

Once your Rust toolchain is set up, you can use the `cargo tools` shortcut for the same tools runner.

```console
cargo tools guide rust-basics
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

Each of the [six terms](../README.md#new-to-reticulum) you learned did its job, live, on your machine.

## Read the code that did it

The whole program is one file: [`personal-rns/examples/node_basics.rs`](../personal-rns/examples/node_basics.rs). It's worth five minutes, because its shape is the shape of every Prns app:

- A `PrnsNodeRecipe` declares everything the node is: its destinations, its storage, its event handler, its interfaces. Every field is required, so if it compiles, nothing was forgotten.
- `PrnsNode::new` returns a node based on that recipe. The node instance can then provide you with a handle. 
- The node is what runs; the handle is how the rest of your program talks to it, from issuing commands to attaching interfaces mid-flight.
- Events arrive as plain values in your `on_event` function. Node B's entire success condition is listening for `AnnounceHeard` and checking who it heard.

Describe the node, run it, react to events, issue commands. That's the foundation everything else builds on.

## Drop the wires

That first run stays on localhost on purpose, and its code wires Node B to Node A's port by hand. The follow-up example, [`auto_discovery.rs`](../personal-rns/examples/auto_discovery.rs), deletes that wiring. Neither node is given any address; both simply turn on Wi-Fi auto-discovery:

```console
cargo tools guide rust-auto-discovery
```

The run succeeds when Node B hears Node A's announce anyway. 

On one machine they meet through a local rendezvous port, which is the same mechanism a second Prns app on your device would use to join the shared instance. (This is why you'll see a 'TcpClient' interface still; it's an automatic one, not the explcitly-named one in the basic example above)

Across two machines it's genuine multicast discovery over your LAN. 

After its first local-only success, the example keeps listening for a minute, so you can run the same command on a second computer on the same network and watch each machine print the other's announce. (Your OS may ask you to approve local-network access the first time.)

## Choose your path

You've now seen a node born, announced, and heard. Where next depends on what you're building:

- **Building an app?** The [example catalog](examples.md#example-catalog) ladders up from here: request and response, resource transfer, changing interfaces on a live node, and the same recipe in every SDK language. [Choose an SDK](sdks.md) for exact readiness and installation guidance. Rust and TypeScript/JavaScript are the paved application paths; the other implemented adapters are source-ready previews whose public packaging is active work.

- **Running a transport node for yourself or the ecosystem?** Take a look at [`prnsd`](../prnsd/README.md), the daemon. It owns the interfaces on a machine, and every Reticulum app on that machine shares its one instance.

- **Putting it on hardware?** [Flash a Hopspot](https://reticulum.rs/flash) in minutes, or work through the [embedded guide](embedded.md) to build board firmware from source.


Want to help make Prns itself even better? See [CONTRIBUTING.md](../CONTRIBUTING.md)
