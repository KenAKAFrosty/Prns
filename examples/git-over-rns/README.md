# git over RNS

A real `git clone` carried over a Prns **ByteStream** — Buffer over Channel over
Link over Reticulum. Every byte of git's pack protocol rides the stream, and git is
none the wiser: its built-in `ext::` transport bridges straight onto it, so there's
no custom remote helper.

Two things make it feel like magic:

- **You clone a Reticulum destination, not an IP.** `clone` names the server's
  `git/serve` destination hash; the path is discovered.
- **The peers find each other with no address configured.** `serve` and `clone`
  bring up the host **auto-interfaces** — WiFi/LAN multicast discovery and USB — so
  two machines on the same network (or USB-tethered) just see each other.

## Try it on one machine

```sh
cargo run
```

Needs `git` and `nc` on `PATH`. This is the self-contained smoke: both nodes in one
process over loopback, building a throwaway repo, cloning it over a ByteStream,
printing the cloned history, cleaning up. (It uses an explicit loopback interface —
auto-discovery is device-to-device, so it can't pair two processes on one host.)

## Across two machines on the same network

On the serving machine, point `serve` at a repo:

```sh
cargo run -- serve --repo /path/to/repo
# prints:  destination: <32-hex-char destination hash>
```

On the other machine, clone that destination — no address, just the hash:

```sh
cargo run -- clone <destination-hash> ./into
```

The cloning node brings up the same auto-interfaces, hears the server's announce over
WiFi/LAN multicast, learns the route, and runs `git clone` over the ByteStream.

**macOS:** the first run prompts once for *"allow local network access"* — macOS
gates multicast egress behind it. Approve it, or no peer can hear this machine.

## What it shows

- Standing up a `Prns` node from a recipe (`PrnsRecipe`) with a served destination.
- Bringing up the host auto-interfaces (`handle.supervise(AutoWifi::new())` and
  `handle.add_interface(UsbAutoHost::new(...))`) — see `src/host.rs`.
- Reaching a peer by `handle.establish_link(dest).await` — addressing a destination,
  not a socket.
- Opening a bidirectional byte stream with `handle.byte_stream(link, rx, tx).await`
  and treating each half as an ordinary `AsyncRead` / `AsyncWrite`.
- Bridging an arbitrary stdio protocol (here, git's) onto the stream with nothing
  but `tokio::io::copy`.

## Notes for going further

- The demo identity is a fixed secret so the destination hash is stable and
  printable. A real deployment would load a persistent identity from a vault, so the
  hash survives restarts without being a known constant.
- Bringing up "all the host auto-interfaces" is boilerplate (`src/host.rs`) that the
  desktop app and other consumers each hand-wire today — a good candidate for a host
  convenience so it becomes one call.

This is an example, not a packaged tool: read it, copy from it, point it at your own
repository.
