## When to reach for this

Use `personal-rns` directly when you're writing a Rust application that
needs to speak Reticulum — and you don't want a daemon process or a
server in the loop.

- A background service inside a desktop app.
- A bare-metal firmware build for a microcontroller.
- A simulation, test harness, or research tool that wants
  deterministic packet flow.
- A library that hides Reticulum behind your own API.

If you'd rather have a node already running and just talk to it,
reach for [the daemon](./prnsd) instead.

## What you get

A small Rust crate that owns the wire contract, routing, announces,
and links — and exposes the engine through explicit packet/time calls:

```rust
ingest_packets(state, inbound_packets, jitter_seed) -> counters;
tick(state, now, jitter_seed) -> egress directives;
```

You feed it packets, time, and per-cycle entropy already drawn by the host;
it tells you what to send out next and what its routing knows.

No global state. No I/O. No threads. `no_std + alloc` ships, and a
smoke script verifies it stays that way. Whatever clock and wire
your platform has, you bring them; the engine is the part that
doesn't care.

## Status

The engine is under active development as the wire contract is
re-grown from the Reticulum reference. Today it handles the
announce path and routing table on real traffic across Linux and
embedded hosts. Links and resources are the next layer landing.
