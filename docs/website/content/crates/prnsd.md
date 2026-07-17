## When to reach for this

`prnsd` is the way to get a Reticulum node running on macOS, Linux, or Windows
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

- A self-managing daemon binary you can start, reattach to, inspect, and stop
  with one command surface.
- Operator-ready human or JSON events, plus bounded OTLP metrics and
  traces for any compatible backend.
- A library crate (`StdHost`) you can depend on directly if you're
  writing your own std-based host — a Tauri app, a server-side
  bridge, or a more opinionated supervisor.

The daemon is deliberately thin. It owns sockets, threads, a
real clock, signals, and logging — and nothing else. Reticulum
itself lives in the engine; this crate is the smallest complete
example of how to bring that engine to life on a real OS.

## Observable without tracing every packet

Lifecycle and failure events carry stable names and low-cardinality
fields. Traces cover bounded operations such as requests, links,
resources, persistence, and interface connections; packet and crypto
hot loops do not create spans. Fixed metrics expose node health, traffic,
links, operation outcomes, resource and link failures, rejected ingress,
egress failures, and inbound and outbound announce flow without unbounded
labels. OTLP export is non-default and bounded, while embedded builds
compile the instrumentation out unless their host explicitly selects it.
The repository includes a disposable Grafana, Prometheus, Loki, and Tempo
stack with a failure-first health dashboard. [Try the setup
guide](https://github.com/KenAKAFrosty/Prns/blob/main/docs/observability.md).

## Status

The daemon's interface and config layout are still settling. Treat
it as a moving target for now — pin a version if you build against
it.
