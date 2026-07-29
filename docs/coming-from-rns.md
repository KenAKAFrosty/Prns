# Coming from RNS

Your config, your identity file, and your apps carry over unchanged. The interoperability suite proves it against real RNS 1.4.0 nodes in CI. Everything below is what's new.

## Brand-new interfaces

- **`PrnsBluetoothAuto`**

    Zero-config Bluetooth. It's one of the most ubiquitous interfaces, available on nearly every platform, acting as a "lingua franca" for interop between devices.

    Every platform uses GATT for the control plane, and for data as a baseline/fallback. But when a pair of peers can both upgrade to an L2CAP channel, they do so for improved throughput and message latency.

    It also includes a compatibility layer for the ble-reticulum protocol, so Columba peers connect too. Designed with energy costs in mind (thoughtful scan intervals, power usage settings, etc.)

- **`PrnsWebSocketClient` / `PrnsWebSocketServer`**

    Reticulum over WebSockets, dialing out or accepting connections. This gets Reticulum more compatible with the vast web-based ecosystem.

- **AutoInterface, upgraded**

    The `AutoInterface` you already run gains other rendezvous mechanisms on top of stock multicast discovery, so peers still find each other on more restricted networks, especially those that filter multicast (mobile hotspot, guest Wi-Fi, browser instances on LAN, etc.).

- **`PrnsUsbAuto`**

    A USB port becomes a Reticulum interface. The daemon scans for peers, handshakes capabilities, and links with whatever Prns node answers (usually an embedded board, or phone). Those nodes speak the same protocol so it's a zero-config, plug-and-play connection. Helps keep the common USB-connected case lower friction.

- **A browser tab on the mesh**

    A `prnsd` instance automatically supervises a rendezvous gateway. A Prns node running on WebAssembly in a browser tab can discover and connect to it with WebSockets, empowering Auto-Wifi behavior even in a local browser. No config needed.


## A built-in operator CLI

The Prns daemon, `prnsd`, not only runs the high-performance transport node, but has built-in CLI tooling to help manage your node.

- **The stock utilities as subcommands**

    The daemon binary is also the utility toolkit. You just use `prnsd ` + `status`, `path`, `probe`, `id`, `cp`, and `x` to run the equivalents of stock `rn*` utilities.

- **A managed lifecycle**

    Running `prnsd` starts the daemon or attaches to the one already running. Ctrl-C detaches without stopping it, and `prnsd ` + `logs`, `restart`, and `stop` round it out. A service file is not needed just to try it.

- **A system tray**

    When running in its default non-headless environment (usually, your own computer), the daemon also sits in the system tray with a live status readout.

    One click each for network status, the interface editor, the configuration folder, or a terminal, and stopping the daemon is right there too.

    Headless machines (typically containerized instances, often in the cloud) skip the tray and run on undisturbed.

- **An interactive interface CLI editor**

    `prnsd interfaces` opens a guided editor in the terminal for you to use interactively. But every verb in it is also scriptable (`list`, `validate`, `add`, `edit`, `enable`, `disable`, `remove`, `repair`, `apply`). Changes print a diff, then save atomically with a backup of the previous file. Comments and unrelated settings stay put, and `--dry-run` stops at the diff.

- **Live apply**

    `apply` hands a change to the running daemon, which reconciles interfaces in place (changes do **not** require a full node restart). A change that fails to come up rolls back. And a prnsd running as a shared-instance client forwards the request to the routing owner.

- **Diagnostics that name the fix**

    A broken config tells you the line, what it found, what it accepts, and what to do:

    ```
    config:5: error[missing_required_key] [interfaces] > [[Home TCP]] > target_port: enabled interface is missing a TCP target port; accepted: a TCP target port; fix: add `target_port = 4242` under [[Home TCP]]
    ```

    `validate` runs the same checks without starting anything, and `repair` walks through the safe corrections.

- **Secure defaults**

    `cp --listen` and `x --listen` permit nobody until an identity is allowed. Remote management is off unless enabled, and its empty allow-list denies everyone.

- **I2P with tooling**

    `prnsd i2p doctor` checks your SAM bridge, and `prnsd i2p setup` walks the setup.



## Observability out of the box

Visibility is built in, from readable logs to a metrics dashboard.

- **Structured logs, human or machine**

    Every log line is a structured event with a stable name and typed fields. The default output is human-readable, `--log-format json` writes the same events as JSON, and the daemon keeps a rotated pair of log files either way.

- **Filter by subsystem**

    The `RUST_LOG` environment variable narrows output per target (`prnsd`, `prns.runtime`, `prns.interface`, and friends), so you can turn one subsystem up for debugging without drowning in the rest.

- **Metrics and traces, with a dashboard included**

    The standard release build includes an OTLP exporter. Point it at any OpenTelemetry collector, or use the one that ships in the repository: `cargo observability` brings up a local Grafana + collector stack with a prnsd dashboard already built.

Once you've got this repo cloned and your toolchain set up, see [the observability guide](observability.md). That guide goes into more details, from log filters to the collector stack.


## Performance you can measure yourself

The benchmark harness runs Prns and stock RNS side by side on your machine, under identical workloads, and records conformance, throughput, latency, CPU, memory, and optionally energy.

```console
./tools/prns doctor benchmarks
cargo benchmark --smoke
cargo benchmark
```

The doctor checks your toolchain and installs nothing. The smoke run proves the machinery with reduced work, and the full run works the whole matrix one cell at a time, provisioning the pinned RNS reference environment itself. Every number it prints was made on your hardware, against the same RNS version the interoperability suite pins.

## Serve NomadNet pages directly from the daemon

Drop `.mu` files into `pages/` inside your Reticulum config directory, and prnsd becomes a page host: it announces a `nomadnetwork.node` destination and serves each file at the `/page/` paths NomadNet browsers already request, with `index.mu` as the landing page. A NomadNet process does not need to run for your node to have a presence on the network.

Pages are read from disk per request, so edits go live immediately. Prnsd lightly rescans the directory every five minutes to register additions and retire deleted or renamed paths; `prnsd pages refresh` applies the same reconciliation on demand.

This pairs naturally with a hosted node: the same daemon routing traffic in the cloud carries your pages with it.

## Beyond the daemon

prnsd is one face of a larger engine, and that engine goes everywhere: onto a $5 microcontroller ([flash a Hopspot](https://prns.dev/flash)), into a browser tab, and inside your own software through [SDKs for Rust, TypeScript, Python, and more](../README.md#what-is-prns). Anything you build against one lane meshes with the rest, including the RNS network you already run.

## Verify it yourself

*(jot: the receipts shelf — run the interoperability suite against real stock RNS 1.4.0 nodes on your own machine; measure both implementations with the benchmark suite. Inventory grammar, link text = verbs.)*

*(drafting guardrail, delete before landing: PARITY LEDGER — features stock RNS 1.4.0 also has, so they stay OUT of the gains sections or the page overclaims: remote management, probe responder, blackhole publish/subscribe/exchange, management announcements, announce ingress/egress controls, interface modes matrix, rnid identity signing, bz2 compression, RNode incl. BLE and multi, Weave. The opener's "applies your stanzas as written" is where parity lives.)*
