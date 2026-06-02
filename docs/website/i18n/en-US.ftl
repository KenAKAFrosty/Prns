# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Footer
footer-tagline = Brought to you by the Personal team.

# Landing
landing-kicker = Unstoppable mesh networks, for the people
landing-title = A production-grade port of Reticulum (RNS) written in Rust.
landing-subtitle = A deterministic, no_std, alloc-free core. Covers RNS and LXMF in full. Built for the performance and battery life every Reticulum stack needs, from a five-dollar microcontroller to a cloud node.
landing-cta-ethos = Pick a crate
landing-cta-crates = How we build it

# Pull quote
landing-quote-label = What we're building toward
landing-quote-body = Reticulum is the foundational communication infrastructure of a bright future we can have, if we build it. This is our effort to bring it into the hands of more developers, to help realize that future.

# What you can count on (standards callout)
standards-section-label = Our standards
standards-section-title = What you can count on
standards-license-label = License
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dual-licensed and permissive. No copyleft, no non-commercial restrictions.
standards-coverage-label = Coverage
standards-coverage-headline = Full RNS and LXMF
standards-coverage-body = Not RNS-only. Not LXMF-on-the-side. Both, in full.
standards-core-label = Core
standards-core-headline = no_std, alloc-free
standards-core-body = A deterministic core that runs where allocators can't.
standards-verification-label = Verification
standards-verification-headline = Diff-tested against RNS
standards-verification-body = Every change checked against the reference; formal proofs where they matter.

# Where do I start? (use-case cards on landing)
start-section-label = Routes in
start-section-title = Where do I start?
start-section-lead = Pick the path that matches what you're building. Each one lands on a single crate today; more guides will land alongside them.

start-daemon-headline = I want a Reticulum node running
start-daemon-body = Pre-built daemon. Drop-in for rnsd. Run it next to the nodes you already have.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = I'm building a mobile app
start-mobile-body = Kotlin (.aar), Swift (.xcframework), or Python (.whl) — the same engine your daemon runs, embedded directly inside your app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = I'm shipping in a game
start-game-body = C# / .NET bindings for Unity, Godot, and MonoGame. Multiplayer without standing up a server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = I'm targeting microcontrollers
start-embedded-body = The engine plus a Host trait of three methods. ESP32-C6 is the reference; S3, nRF, RP2040, and STM32 are next.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = I'm building for the web or edge
start-web-body = A WebAssembly build that runs in the browser and on edge runtimes like Cloudflare Workers, Fastly, and Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = I'm embedding in a Rust app
start-rust-body = A complete RNS runtime out of the box, or the pure core to build your own runtime around.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = I want to send messages over the mesh
start-lxmf-body = LXMF on top of Reticulum — identities, addresses, delivery. The layer Sideband and Nomadnet sit on.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = The discipline
ethos-title = How we build this
ethos-lead = An engineer-to-engineer note on the discipline behind this project — pure engine, alloc-free core, every change verified against the RNS reference. Skim it before you depend on this; we want you to know what you're getting into.

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
crate-rvt-role = Visual debugger
crate-rvt-blurb = Watch packets move across simulated nodes on a virtual clock. Deterministic — same scenario, same trace, every time.
crate-esp32c6-role = ESP32-C6 firmware
crate-esp32c6-blurb = Bare-metal host adapter for the ESP32-C6. No OS, no allocator — proof the engine runs on a five-dollar RISC-V chip with built-in radios.

# 404
not-found-title = There's nothing here yet.
not-found-cta = Back to home
