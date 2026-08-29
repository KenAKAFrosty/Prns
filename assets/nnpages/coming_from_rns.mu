# prnsd:managed:coming-from-rns
#!bg=000
>Coming from RNS
>>

Your config, your identity file, and your apps carry over unchanged. The interoperability suite proves it against real RNS 1.4.2 nodes in CI. Everything below is what's new.

>>`!Brand-new interfaces`!

`F6eb•`f `F6eb`!PrnsBluetoothAuto`!`f
>>>
Zero-config Bluetooth. It's one of the most ubiquitous interfaces, available on nearly every platform, acting as a "lingua franca" for interop between devices.

Every platform uses GATT for the control plane, and for data as a baseline/fallback. But when a pair of peers can both upgrade to an L2CAP channel, they do so for improved throughput and message latency.

It also includes a compatibility layer for the ble-reticulum protocol, so Columba peers connect too. Designed with energy costs in mind (thoughtful scan intervals, power usage settings, etc.)

>>
`F6eb•`f `F6eb`!PrnsWebSocketClient / PrnsWebSocketServer`!`f
>>>
Reticulum over WebSockets, dialing out or accepting connections. This gets Reticulum more compatible with the vast web-based ecosystem. And since WebSocket traffic is ordinary web traffic, it passes through free tunnel services like Cloudflare Tunnel: you can host a public node from home with no port forwarding, no exposed home IP, and no rented server.

>>
`F6eb•`f `F6eb`!AutoInterface, upgraded`!`f
>>>
The `B333AutoInterface`b you already run gains other rendezvous mechanisms on top of stock multicast discovery, so peers still find each other on more restricted networks, especially those that filter multicast (mobile hotspot, guest Wi-Fi, browser instances on LAN, etc.).

>>
`F6eb•`f `F6eb`!PrnsUsbAuto`!`f
>>>
A USB port becomes a Reticulum interface. The daemon scans for peers, handshakes capabilities, and links with whatever Prns node answers (usually an embedded board, or phone). Those nodes speak the same protocol so it's a zero-config, plug-and-play connection. Helps keep the common USB-connected case lower friction.

>>
`F6eb•`f `F6eb`!A browser tab on the mesh`!`f
>>>
A `B333prnsd`b instance automatically supervises a rendezvous gateway. A Prns node running on WebAssembly in a browser tab can discover and connect to it with WebSockets, empowering Auto-Wifi behavior even in a local browser. No config needed.

>>
>>`!A built-in operator CLI`!

The Prns daemon, `B333prnsd`b, not only runs the high-performance transport node, but has built-in CLI tooling to help manage your node.

`F6eb•`f `F6eb`!The stock utilities as subcommands`!`f
>>>
The daemon binary is also the utility toolkit. You just use `B333prnsd`b + `B333status`b, `B333path`b, `B333probe`b, `B333id`b, `B333cp`b, and `B333x`b to run the equivalents of stock rn* utilities.

>>
`F6eb•`f `F6eb`!A managed lifecycle`!`f
>>>
Running `B333prnsd`b starts the daemon or attaches to the one already running. Ctrl-C detaches without stopping it, and `B333logs`b, `B333restart`b, and `B333stop`b round it out. A service file is not needed just to try it.

>>
`F6eb•`f `F6eb`!A system tray`!`f
>>>
When running in its default non-containerized environment (usually, your own computer), the daemon also sits in the system tray with a live status readout. One click each for network status, the interface editor, the configuration folder, or a terminal, and stopping the daemon is right there too. Headless machines skip the tray and run on undisturbed.

>>
`F6eb•`f `F6eb`!An interactive interface CLI editor`!`f
>>>
`B333prnsd interfaces`b opens a guided editor in the terminal for you to use interactively. But every verb in it is also scriptable (`B333list`b, `B333validate`b, `B333add`b, `B333edit`b, `B333enable`b, `B333disable`b, `B333remove`b, `B333repair`b, `B333apply`b). Changes print a diff, then save atomically with a backup of the previous file. Comments and unrelated settings stay put, and `B333--dry-run`b stops at the diff.

>>
`F6eb•`f `F6eb`!Live apply`!`f
>>>
`B333apply`b hands a change to the running daemon, which reconciles interfaces in place: changes do not require a full node restart. A change that fails to come up rolls back. And a prnsd running as a shared-instance client forwards the request to the routing owner.

>>
`F6eb•`f `F6eb`!Diagnostics that name the fix`!`f
>>>
A broken config tells you the line, what it found, what it accepts, and what to do:

`Faaa`B333
`=
config:5: error[missing_required_key]
[interfaces] > [[Home TCP]] > target_port
enabled interface is missing a TCP target port
accepted: a TCP target port
fix: add target_port = 4242 under [[Home TCP]]
`=
``

`B333validate`b runs the same checks without starting anything, and `B333repair`b walks through the safe corrections.

>>
`F6eb•`f `F6eb`!Secure defaults`!`f
>>>
`B333cp --listen`b and `B333x --listen`b permit nobody until an identity is allowed. Remote management is off unless enabled, and its empty allow-list denies everyone.

>>
`F6eb•`f `F6eb`!I2P with tooling`!`f
>>>
`B333prnsd i2p doctor`b checks your SAM bridge, and `B333prnsd i2p setup`b walks the setup.

>>
>>`!Observability out of the box`!

Visibility is built in, from readable logs to a metrics dashboard.

`F6eb•`f `F6eb`!Structured logs, human or machine`!`f
>>>
Every log line is a structured event with a stable name and typed fields. The default output is human-readable, `B333--log-format json`b writes the same events as JSON, and the daemon keeps a rotated pair of log files either way.

>>
`F6eb•`f `F6eb`!Filter by subsystem`!`f
>>>
The `B333RUST_LOG`b environment variable narrows output per target (`B333prnsd`b, `B333prns.runtime`b, `B333prns.interface`b, and friends), so you can turn one subsystem up for debugging without drowning in the rest.

>>
`F6eb•`f `F6eb`!Metrics and traces, with a dashboard included`!`f
>>>
OTLP-capable builds can export to an OpenTelemetry collector. The source includes the complete local stack: `B333cargo observability`b brings up Grafana, a collector, and a prnsd dashboard.

>>
>>`!Performance you can measure yourself`!

The benchmark harness runs Prns and stock RNS side by side on your machine, under identical workloads, and records conformance, throughput, latency, CPU, memory, and optionally energy. The published results peak at 89× the throughput, 48× smaller peak-memory footprint, and 33× the energy efficiency of stock RNS 1.4.2. The harness provisions the pinned RNS reference environment, and every number it prints is made on your hardware.

>>`!Serve NomadNet pages directly from the daemon`!

You're reading one right now. The prnsd that owns your routing tables can host your NomadNet pages too, without a separate NomadNet process. Drop `B333.mu`b files into `B333nnpages/pages/`b under its active Reticulum configuration directory (folders become path segments), and each one is served from the node's stable `B333nomadnetwork.node`b destination, with `B333index.mu`b as the landing page.

The `B333nnpages/files/`b directory serves downloads. The bootstrapped install starts you with three `B333.mu`b pages: `B333index.mu`b is yours to edit from the first moment, while the others use a first-line marker. Delete the marker and that page becomes yours too.

Every request opens the file fresh from disk and streams it in bounded pieces, so edits and deletions take effect on the very next request and a large download never has to fit in memory.

New pages and files are picked up by a light reconciliation every five minutes, or immediately with `B333prnsd nnpages refresh`b.

Announcements are automatic: every six hours by default, and only while there is an `B333index.mu`b to serve. `B333nnpages/settings.toml`b governs announcement policy and nothing else, with `B333announce`b and `B333announce_interval_minutes`b as the knobs. Turning automatic announcements off leaves serving on, and `B333prnsd nnpages announce`b still announces on demand. Settings changes apply at the next five-minute reconciliation, or immediately with `B333prnsd nnpages refresh`b.

You can change your NomadNet node's name with `B333prnsd nnpages rename "My Node"`b. Changes to the name are live immediately, and persisted to disk.

At any point you can use `B333prnsd nnpages seed`b to re-generate the starter pages along with a default `B333settings.toml`b (without touching anything you've already made yours). Source hosting is opt-in: include `B333--source`b to stage the exact shipped source code bundle. Once staged, a prnsd-managed archive and checksum advance automatically with newer source-bearing prnsd releases; operator-owned archives stay untouched.

By default, prnsd will find the active Reticulum configuration automatically; pass `B333--config`b for a nondefault foreground or container daemon.

>>`!Beyond the daemon`!

prnsd is one face of a larger engine, and that engine goes everywhere: onto a $5 microcontroller, into a browser tab, and inside your own software through SDKs for Rust, TypeScript, and many more. Anything you build against one meshes with the rest, including any RNS network you already participate in.

>>`!Continue`!

Everything above is verifiable: the interoperability suite, the benchmark harness, and the full implementation ship in the source.

`F6eb`_`[Download the source`:/page/source.mu]`_`f `F999or visit`f `B6eb`F222 https://github.com/KenAKAFrosty/Prns `f`b

<
-

`F6eb`_`[Back to index`:/page/index.mu]`_`f
