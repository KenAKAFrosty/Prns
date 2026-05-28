## When to reach for this

`personal-rnsd` is the way to get a Reticulum node running on Linux
without writing any code.

- Run a Reticulum router on your laptop, server, or Raspberry Pi.
- Bridge interfaces — pair a LoRa radio over USB with a WiFi or
  Ethernet link, so your mesh reaches further than any one device.
- Develop and test apps that talk to a real node over the local
  socket, the way Sideband and Nomadnet already do.

If you're embedding Reticulum *inside* your own application instead
of talking to a separate node, you want [the engine
crate](./personal-rns) directly.

## What you get

- A daemon binary you can launch with one command.
- A library crate (`StdHost`) you can depend on directly if you're
  writing your own std-based host — a Tauri app, a server-side
  bridge, or a more opinionated supervisor.

The daemon is deliberately thin. It owns sockets, threads, a
real clock, signals, and logging — and nothing else. Reticulum
itself lives in the engine; this crate is the smallest complete
example of how to bring that engine to life on a real OS.

## Status

The daemon's interface and config layout are still settling. Treat
it as a moving target for now — pin a version if you build against
it.
