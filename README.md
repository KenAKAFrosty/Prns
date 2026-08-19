# Prns

<p align="center">
  <a href="https://reticulum.rs" target="_blank">
  <img src="docs/website/public/assets/og.png" alt="Prns: high-performance Reticulum (RNS), built to run on any device." width="800" />
  </a>
</p>

[![CI](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml/badge.svg)](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.90-orange.svg)](#minimum-supported-rust-version)
![no_std](https://img.shields.io/badge/no__std-core-success.svg)

## What *is* Prns?

Prns is a ground-up implementation of Reticulum, written in Rust. It's highly focused on performance, energy efficiency, compatibility, and developer experience. 

Prns is built on a unified core engine that is `no_std` (no `alloc` required either), so it runs on nearly anything, whether that's a $5 microcontroller, a web browser, a native smartphone app, a personal laptop, or a backbone cloud server.

Its application SDKs come in two stages today:

- **Paved:** [Rust](personal-rns/README.md) · [TypeScript / JavaScript](prns-js/README.md) (browsers, Node.js, and Bun)
- **Previews:** [Python](prns-host/bindings/python/README.md) · [.NET & C#](prns-host/bindings/dotnet/README.md) · [Go](prns-host/bindings/go/README.md) · [Swift](prns-host/bindings/swift/README.md) · [Kotlin / Java / Android](prns-host/bindings/jvm/README.md) · [Julia](prns-host/bindings/julia/README.md) · [C & C++](prns-host/abi/c/README.md)

**Previews** are not stubs. Each drives the same native Rust engine, with generated types and a live conformance suite.

What's still young is the packaging. Today they run straight from this repository. A proper package on each language's registry, prebuilt binaries included, is the road still being paved.

If one of these ecosystems is home for you, shaping its consumer API and packaging is some of the most valuable [contribution work](CONTRIBUTING.md) in Prns right now.

**Paved** means the direction is set and a public package is up. Set is not sealed, though. API design input and contributions are still welcome.

[Choose an SDK and see its exact readiness and installation path](docs/sdks.md).

If you're already familiar with Reticulum, you can [jump to here](#coming-from-rns). 

If you came here to put Reticulum on an embedded device, [flash a Hopspot here](https://reticulum.rs/flash).

## Wait, what's *Reticulum*?

Reticulum is a powerful networking stack in which an address is based on a cryptographic identity. Addresses aren't assigned by a provider, nor are they tied to a location, and they're reachable only through end-to-end encryption. 

It runs over anything that moves bytes, from LoRa radios and serial lines to TCP across the ordinary internet. Nodes automatically mesh across whatever links they happen to have.

## Okay, but what's the benefit?

Reticulum gives you highly resilient networks that **you own** outright. 

Two cheap LoRa radios kilometers apart can form an encrypted link with no carrier or service provider required between them. Add a handful more and you have a mesh network that covers a small town. There is no server or internet connection needed.

But it's not limited to niche hardware. The phone in your pocket is already enough. If you take two phones right out of the box, technically they can already communicate over Bluetooth across a room or two; over a local Wi-Fi hotspot across a building; and over TCP across the world. 

The problem is apps, and tools for people who build them, have always treated these like separate lanes. 

Reticulum knocks down the walls between them. It treats each of those mediums as what they are: just another pipe to exchange data over.

### A packet on your phone could
  1) Leave your phone via Bluetooth
  2) Travel through a nearby laptop's Bluetooth
  3) Relay back out on that laptop's local WiFi connection
  4) Travel through a desktop computer on that same local WiFi hotspot
  5) Get relayed back out over USB to a LoRa radio device attached to the desktop
  6) Travel through the attached device then relay back out over LoRa radio
  7) Get received by a LoRa radio in an embedded device on a rooftop miles away
  8) Relay back out over that embedded device's Bluetooth
  9) Land at the target address on the other phone, on its Bluetooth


With a fully end-to-end encrypted link the entire time. 
Not a single other device along that chain can see the contents of the traffic (not even a source address; this is also called "initiator anonymity").

All of this happens without any extra work from the app developer. An app simply "dials" an address, and the mesh finds the best way to get it there.

The ability to digitally communicate becomes something people & devices *have*, not a service they're *temporarily granted access to*.



## What can I do with it?

What opens up to you is software that brings its own network. 

Five people on a hike together *should* be able to play a game with each other on their mobile phones, even if some or all of them are out of cell service. 

Then they each part ways and head home, and they *should* be able to keep playing over the stable internet that's now returned.  

And most importantly: that game's developer *shouldn't* have to manage any of that. 

Reticulum finally makes that possible.

Connection to the network becomes a gradient instead of a switch. There's no "offline/online" binary split. Every medium, from packet radio to fiber, just adds reach, reliability, and capability.

Some obvious examples (you can probably come up with even better):
- Make an app to sync notes between your laptop and your phone directly, whether or not the Wi-Fi network they share has internet behind it.
- Ship a game where two people can play against each other anywhere they meet, internet or not.
- Scatter a field of sensors that report over LoRa to one solar-powered board, and read them from town across the mesh. And with no proprietary code locking you in to some vendor.

A separate backend server is simply not needed; two installs of your app already *are* the network infrastructure. 

And if you *want* to add your own internet-reachable TCP server to act as a stable relay for your users? Trivial! But never *necessary*.

No obligation for a backend means no day-one cloud bill that grows with user activity, and no end-of-service announcement looming over your future.  


## Where to begin?

Prns joins an ecosystem that's been running for years on [`RNS`](https://github.com/markqvist/Reticulum) (Reticulum's reference implementation in Python). RNS goes where Python goes, Prns carries the same protocol the rest of the way. 

If you've already been using RNS, you can skip to [Coming from RNS](#coming-from-rns).


## New to Reticulum

Make sure to read the first portion of this README. Once you have, all that's left is vocabulary, and a little practice.

1) Six key terms is all it takes to get you going.
   - **Identity**: A pair of cryptographic keys your device generates for itself, and their ability to sign payloads. Everything else is built on top of it.

   - **Destination**: The thing you send to or receive on; an address deterministically computed from the combination of:
      - An Identity
      - An app's self-chosen name for destinations
      - An optional list of an app's self-chosen classifier strings called "aspects"
    
      It's sort of like a URL nobody can sell, squat, or revoke (though it doesn't look like the URLs you're used to).

   - **Announce**: A small broadcast packet that says "this destination exists", proven by signing the packet with the Identity the destination is derived from. Every node that passes it along remembers which way it came from, and that's largely how the mesh learns its routes.

      Announcing is under your app's control. A destination only announces when told to, and you can attach a small piece of app data when announcing (a display name, a version, whatever's useful). 
      
      Apps can also *listen* for announces from the kinds of destinations they care about.
      
      Announces serve as both a key part of routing and a built-in discovery mechanism that needs no lobby server or registry.

   - **Interface**: One attachment to one medium, e.g., a TCP connection, the Bluetooth radio, a connected USB device. A single node can carry several at once, and most do.

   - **Transport node**: A node that forwards traffic between its interfaces on behalf of others. In the "[A packet on your phone could..](#a-packet-on-your-phone-could)" example above, the laptop, desktop, and embedded device were all doing exactly this.

   - **Link**: A lightweight end-to-end encrypted session between two destinations. It's what your app will often talk over, and is required for any payloads that won't fit in a single packet.


   (If you want to keep reading first, [there's a second helping of concepts here](docs/more-concepts.md#packet).)

2) [Follow the Getting Started guide](docs/getting-started.md#getting-started) and experience one real result at a time. (Cloning on Windows? Run `git config --global core.longpaths true` first — some benchmark evidence paths exceed the default 260-character limit.)
3) [Browse the example catalog](docs/examples.md) for the next step up.

## Coming from RNS

Your network and apps don't change, just your daemon does.

The Prns daemon, `prnsd`, takes the role `rnsd` holds today. It handles your current config and identity, and your apps carry over unchanged.

Among what you gain: brand-new interfaces, a built-in operator CLI, observability out of the box, and [up to 89× the throughput](benchmarks/RESULTS.md) in published benchmarks you can rerun yourself. [Here's the full before-and-after](docs/coming-from-rns.md).

> 89× is the best published result (single-packet throughput on macOS), but most scenarios land between 3× and 20× depending on host and workload

#### Looking for something specific?

- [Start prnsd](prnsd/README.md) for a high-performance shared instance on your machine, which works with Sideband, NomadNet, MeshChat, etc.
- [Flash a Hopspot](https://reticulum.rs/flash) to get self-contained Reticulum running on your embedded devices.
- [Put your own high-performance backbone online](prnsd/README.md#deploy-it) with Docker or Railway.
- [Measure both implementations side by side](benchmarks/README.md) with the benchmark suite.
- [Verify the interoperability yourself](docs/validation.md), against real RNS nodes on your own machine.



## Minimum supported Rust version

The workspace's declared and CI-tested MSRV is Rust **1.90**. Development builds use the stable channel configured in [rust-toolchain.toml](rust-toolchain.toml).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and the [testing guide](docs/testing.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
