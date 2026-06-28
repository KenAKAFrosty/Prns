# Navigation
nav-contributing = Contributing
nav-crates = Crates
nav-api = API Reference

# Footer
footer-tagline = Brought to you by the Personal team.

# Landing
# `landing-kicker` is the full eyebrow, used as-is by every non-English locale.
# en-US renders an animated variant: `landing-kicker-prefix` followed by a final
# word that rotates through several qualities and rests on "yours". The rotating
# words live in src/pages/landing.rs (English-only, since the trick is word-order
# specific).
landing-kicker = Mesh networking that's yours
landing-kicker-prefix = Mesh networking that's
landing-title = A high-performance port of Reticulum (RNS). Runs on any device.
# en-US renders the title on two lines, the second ("Runs on any device.") in
# the accent green, matching the OG card. Other locales use landing-title as-is.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Built for the performance, stability, and energy efficiency every Reticulum node needs, from a five-dollar microcontroller to a cloud server cluster. One engine and one API, the same on embedded, desktop, mobile, games, and the web.
landing-cta-ethos = Find your path in Prns
landing-cta-contributing = Contributing

# Pull quote
landing-quote-label = What we're building toward
landing-quote-body = Reticulum is the foundational communication infrastructure of a bright future we can have, as long as we all build it. This is the Personal team's effort to put RNS into the hands of more builders, to help realize that future.

# Interface highlights
interfaces-section-label = Interfaces
interfaces-section-title = Where the mesh meets the world
interfaces-section-lead = Prns keeps the RNS-compatible interfaces builders already know, then expands the map with native links for new devices and networks.
interfaces-section-hot-note = Prns interfaces are hot-swappable: add, remove, or change an interface without a node restart.

interfaces-radio-label = Radios
interfaces-radio-headline = Proximity links for devices and boards
interfaces-radio-body = BLE Auto-interface, ESP-NOW, and LoRa bring nearby devices, board fleets, and long-range RF links into one Reticulum mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Auto-discovered local-link peers
interfaces-lan-body = Wi-Fi Auto-interface uses multicast, mDNS, and gateway rendezvous to find nearby nodes and fold a local network into the mesh.

interfaces-cable-label = Wires + packet radio
interfaces-cable-headline = Cables, TNCs, and radio modems
interfaces-cable-body = USB Auto-interface, serial framing, KISS, AX.25, and RNode bridge small devices and packet-radio hardware into the same mesh.

interfaces-host-label = Routed IP
interfaces-host-headline = Internet, WAN, and backbone links
interfaces-host-body = TCP client/server, UDP, and Backbone let distant peers participate in the mesh across private WANs, VPNs, and public Internet relays.

# What you can count on (standards callout)
standards-section-label = Our standards
standards-section-title = What you can count on
standards-license-label = License
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dual-licensed and permissive. No copyleft or commercial restrictions.
standards-safety-label = Safety
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = The personal-rns engine contains zero unsafe, enforced by the compiler. The unsafe inside dependencies is audited with cargo-geiger and checked for UB under Miri.
standards-correctness-label = Correctness
standards-correctness-headline = Diff-tested against RNS
standards-correctness-body = Every change is checked against the reference, then put through unit, property, fuzz, and mutation tests, with Kani proofs where they matter.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Measured, not just claimed
standards-benchmarked-body = Performance is tracked in the open, measured by a harness you can run yourself.
standards-benchmarked-cta = See the benchmarks →

# Where do I start? (use-case cards on landing)
start-section-label = Routes in
start-section-title = What are you here to do?
start-section-lead = Choose the path that matches how Prns fits into your work: hardware you flash, infrastructure you run, or software you build.

start-daemon-headline = Run a daemon
start-daemon-body = Install a fast Reticulum daemon for desktops, LXMF apps, backbone VPSs, etc.
start-daemon-code = Installation
    Compatibility
    Benchmarks
start-daemon-target = Run Prnsd

start-mobile-headline = I'm building a mobile app
start-mobile-body = Kotlin (.aar), Swift (.xcframework), or Python (.whl) — the same engine your daemon runs, embedded directly inside your app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = I'm shipping in a game
start-game-body = C# / .NET bindings for Unity, Godot, and MonoGame. Multiplayer without standing up a server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = I'm building for the web or edge
start-web-body = A WebAssembly build that runs in the browser and on edge runtimes like Cloudflare Workers, Fastly, and Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Build on Reticulum
start-rust-body = Use the engine and bindings to add mesh networking to apps, tools, services, or games.
start-rust-code = Quickstart
    API examples
    Bindings
start-rust-target = Choose a developer path

start-lxmf-headline = I want to send messages over the mesh
start-lxmf-body = LXMF on top of Reticulum — identities, addresses, delivery. The layer Sideband and Nomadnet sit on.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Platforms ("Runs on") - hero marquee label + CTA, and the dedicated page
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-legend-runtime = Runtime platform
platforms-legend-bringup = Active bring-up
platforms-legend-roadmap = Roadmap
platforms-runtime-title = Runtime support quick view
platforms-runtime-lead = Microcontrollers list silicon and radio families here; exact boards, flashing readiness, and interfaces live in the board catalog.
platforms-board-support-link = Specific board support →

# Flash a Hopspot page
flash-back = Platforms
flash-kicker = Supported boards
flash-title = Flash a Hopspot
flash-lead = Pick a specific board, compare radio and battery tradeoffs, then flash or build the dedicated Hopspot firmware path.
flash-note = Hosted builds can download firmware artifacts directly. When this same docs site is served from a Hopspot, artifact actions should stay disabled and point back to the online flasher or local build path.
flash-board-title = Select a board
flash-board-lead = Choose a flashable target to load its board-specific flasher. Bring-up and roadmap boards stay visible here, but cannot be selected yet.
flash-picker-change-title = Change board
flash-interfaces-label = Interfaces
flash-interfaces-pending = Interfaces pending board bring-up
flash-card-action = Flash
flash-card-selected = Selected
flash-ready-kicker = Ready target
flash-ready-title = Web flashing
flash-ready-action = Connect and flash
flash-ready-action-pending = Firmware artifacts are not wired into this build yet.
flash-local-title = Local build
flash-local-body = Fully offline? Build this repo locally and flash the board-specific Hopspot target from a developer machine.
flash-unavailable-title = Not flashable yet
flash-unavailable-body = This target is listed for bring-up or roadmap tracking, but it does not have a public web-flash artifact yet.
flash-missing-title = Board not found
flash-missing-body = Pick a supported board from the catalog.

# Benchmarks page
benchmarks-kicker = Performance
benchmarks-title = Benchmarked in the open
benchmarks-lead = We treat performance as a number, not an adjective. Every figure here comes from a deterministic harness in the repo, measured on real hardware and checked against the RNS reference where the comparison is fair. The numbers are landing as the suite stabilizes; below is the methodology they hold to.

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Third-party logos, trademarks, and product images belong to their respective owners. They are shown only to identify platforms, hardware, and compatibility targets. No endorsement is claimed or implied.

# Contributing page
contributing-kicker = The bar
contributing-title = Contributing
contributing-lead = How to contribute — what we value, the conventions your code follows, and the standard every change clears. For human and automated contributors alike.

# Crates index
crates-kicker = The pieces
crates-title = Pick what matches what you're building.
crates-lead = Each crate is built to be useful on its own, even if you don't pull in the rest. The engine is the substrate; everything else stacks on top, and more pieces are landing as the suite grows.
crates-card-cta = What it does →
crates-back = All crates
crates-not-found = No crate by that name

# Per-crate cards (consumer-framed)
crate-rns-role = The engine
crate-rns-blurb = Drop Reticulum into any Rust project. Deterministic, no_std, alloc-free; no global state, no built-in I/O — bring your own clock and wire.
crate-rnsd-role = The daemon
crate-rnsd-blurb = A drop-in for rnsd that runs anywhere Linux runs. Same wire as the RNS reference; use it alongside or in place of the nodes you already have.
crate-lxmf-role = Messaging
crate-lxmf-blurb = LXMF on top of Reticulum — the layer Sideband and Nomadnet sit on. Identities, addresses, message delivery.
crate-ffi-role = Mobile + Python bindings
crate-ffi-blurb = One uniffi interface generates Kotlin (.aar), Swift (.xcframework), and Python (.whl). Use Reticulum from Android, iOS, or a Jupyter notebook — same shape, same engine.

# 404
not-found-title = There's nothing here yet.
not-found-cta = Back to home
