# Navigation
nav-contributing = Contributing
nav-api = API Reference

# Footer
footer-tagline = Brought to you by KenAKAFrosty and the Personal/Prns team.

# Landing
# `landing-kicker` is the full eyebrow, used as-is by every non-English locale.
# en-US renders an animated variant: `landing-kicker-prefix` followed by a final
# word that rotates through several qualities and rests on "yours". The rotating
# words live in src/pages/landing.rs (English-only, since the trick is word-order
# specific).
landing-kicker = Mesh networking that's yours
landing-kicker-prefix = Mesh networking that's
landing-title = High-performance Reticulum (RNS), built to run on any device.
# en-US renders the title on two lines, the second ("built to run on any device.") in
# the accent green, matching the OG card. Other locales use landing-title as-is.
landing-title-lead = High-performance Reticulum (RNS),
landing-title-accent = built to run on any device.
landing-subtitle = Built for the performance, stability, and energy efficiency every Reticulum node needs, from a five-dollar microcontroller to a cloud server cluster. One engine and one API, the same on embedded, desktop, mobile, games, and the web.
landing-cta-ethos = Find your path in Prns
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
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW, and LoRa bring nearby devices, board fleets, and long-range RF links into one Reticulum mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Auto-discovered local-link peers
interfaces-lan-body = Wi-Fi Auto-interface uses multicast, mDNS, and gateway rendezvous to find nearby nodes and fold a local network into the mesh.

interfaces-cable-label = Wires + packet radio
interfaces-cable-headline = Cables, TNCs, and radio modems
interfaces-cable-body = USB Auto-interface, serial framing, KISS, AX.25, and RNode bridge small devices and packet-radio hardware into the same mesh.

interfaces-host-label = Routed IP
interfaces-host-headline = Internet, WAN, and backbone links
interfaces-host-body = TCP client/server, UDP, WebSocket, and Backbone let distant peers participate in the mesh across private WANs, VPNs, public Internet relays, and browser integrations.

# What you can count on (standards callout)
standards-section-label = Our standards
standards-section-title = What you can count on
standards-license-label = License
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dual-licensed and permissive. No copyleft or commercial restrictions.
standards-safety-label = Safety
standards-safety-headline = Enforced, then audited
standards-safety-body = In the engine, panics, unwraps, and unjustified unsafe never compile. What can't be forbidden is audited: dependency unsafe with cargo-geiger, undefined behavior under Miri, advisories with cargo-deny.
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
start-daemon-code = Drop-in for stock apps
    Reads ~/.reticulum
    Live interface edits
    Built-in metrics
start-daemon-target = Run Prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Use the browser node playground
start-web-body = Try the TypeScript API with the shared Rust engine in WebAssembly, connect through Auto Wi-Fi or USB Auto, and watch live node activity locally.
start-web-code = WebAssembly runtime
    Auto Wi-Fi + USB Auto
    TypeScript example
start-web-target = Open playground

start-rust-headline = Build on Reticulum
start-rust-body = Use the engine and bindings to add mesh networking to apps, tools, services, or games.
start-rust-target = Read the README
start-rust-target-source = Download the source

# Platforms ("Runs on") - hero marquee label + CTA, and the dedicated page
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

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

# 404
not-found-title = There's nothing here yet.
not-found-cta = Back to home
