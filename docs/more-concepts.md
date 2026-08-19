# More Key Concepts

## The essentials

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

- **Transport node**: A node that forwards traffic between its interfaces on behalf of others. In the [packet chain example](../README.md#a-packet-on-your-phone-could) from the README, the laptop, desktop, and embedded device were all doing exactly this.

- **Link**: A lightweight end-to-end encrypted session between two destinations. It's what your app will often talk over, and is required for any payloads that won't fit in a single packet.

## Packet

The unit everything else is made of. A packet is a small typed header (flags, hop count, the destination hash, a context byte) followed by the payload; the baseline MTU is 500 bytes, small enough that even a slow LoRa radio carries a whole packet in a couple of air frames. Announces, link handshakes, requests, proofs: all of it moves as packets.

Notably, there is no source address field. Reticulum doesn't need one to route, and that's where its "initiator anonymity" comes from. A packet never names its originator.

## Path/Route (and hops)

A path (or route) is how the mesh reaches a destination that isn't a direct neighbor; which interface to send out of, and how many transport nodes stand along the way (each one is a hop). Transport nodes learn routes from the announces they relay, and keep them in a routing table. That allows transport nodes to route traffic and answer path requests.

Applications don't have to manage any of this. You address the destination, and the mesh does the walking. (Though, the API does allow you to introspect things like the routing table, if your app *does* want to know)

One naming note: where RNS says *path*, Prns's own code and configuration generally say *route*. Same concept; when you see "route", read "path", and vice-versa.

## Path Request

Sometimes, an app may already know of a destination before hearing an announce for it (known from a previous session, exchanged out of band, etc.).

A path request asks the nearby network directly: "does anyone know a way to this destination?" Transport nodes that do indeed know will respond, and traffic can flow without waiting for the next announce.

---


> Everything below this point rides on the Link primitive. These are higher-level abstractions that Reticulum defines and Prns also provides, so you don't have to build them yourself. You are not limited to, nor required to use, the following APIs.

## Request and Response

The RPC shape, carried over a link. A destination registers named endpoints, and a peer asks one of them and gets typed data back, or a failure it can handle at the call site. Requests are for small asks (they ride in packets). When the answer is bulk data, the Response hands off to a Resource. 

If you've built against HTTP endpoints, this will feel familiar, minus the server in the middle.

## Resource

Bulk transfer over a Link. The sender splits the data into parts sized for the Link, compresses them when that actually helps, and hashes everything. The receiver acknowledges, reassembles, and verifies before your application ever sees the data. Settlement is explicit. The transfer either proves delivery or reports why it couldn't.

Again, you as an app developer probably won't ever have to touch those internals. You'll only need to understand that when transferring large data over a Link, the Resource is an out-of-the-box API you have the option of using.


## Channel

A reliable, ordered message pipe over an established Link. A Channel gives numbered messages, retransmits what gets lost, and delivers everything in order for as long as the Link lives. If you've built against WebSockets, this will feel familiar (a persistent, two-way conversation in discrete messages).

## Buffer / ByteStream

A continuous stream of bytes over a channel, read and written like a file or a socket, for data that's a flow rather than a set of discrete messages. Same naming split as paths and routes: the Reticulum ecosystem calls this a *Buffer*, while in Prns you'll see it as a ByteStream (`ByteStreamReader` and `ByteStreamWriter`). One concept, just two names for it.

## Daemon and shared instance

One device often runs several Reticulum apps, and it would be wasteful for each to open its own radios and sockets. Instead, one program owns the interfaces and the routing view (the shared instance), and every other app on the device attaches to it locally as a client. 

A daemon is that program run deliberately. It is headless and long-lived, there before your apps start and after they exit. Prns ships `prnsd`; the reference implementation ships `rnsd`. 

Note that Prns takes the driver's or the passenger's seat automatically. It will serve as the shared instance if the device lacks one, or attaches as a client if one is already running.

---

> Items below this point aren't directly related nor strictly necessary to learn. They're just likely terms you'll hear related to Reticulum. These items may be added, changed, or removed over time.

## LXMF and its apps

Reticulum moves bytes between destinations. It doesn't define what a "message" is.

[LXMF](https://github.com/markqvist/LXMF) (Lightweight Extensible Message Format) is a separate, optional message standard & related Python library built on top of Reticulum/RNS. It's authored by Mark Qvist, the same author of RNS itself.

Sideband and Columba are examples of apps that speak LXMF. NomadNet speaks it too, while also adding its own standard for serving and browsing small, basic pages over Reticulum directly.

Prns sits below all of this, at the Reticulum/RNS layer. Because LXMF rides entirely on top of Reticulum, a running Prns daemon provides transport for those LXMF apps out of the box. 


## Hardware you'll hear about

- **LoRa**: long-range, low-power radio in unlicensed bands. Kilometers of range on milliwatts of power, at very modest bandwidth. The current workhorse medium for off-grid Reticulum.
- **RNode**: the Reticulum community's open radio device design & accompanying firmware. Connect one to a computer or mobile device that runs Reticulum, and it becomes a LoRa interface.
- **Hopspot**: Prns's ready-to-flash firmware for supported radio boards. One cheap board becomes a self-contained transport node that you can put on a windowsill, in a bag, or on a rooftop. [Flash one here](https://reticulum.rs/flash).

That's the working vocabulary! Next stop: [Getting Started](getting-started.md)
