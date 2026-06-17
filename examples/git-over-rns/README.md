# git over RNS

A real `git clone` carried over a Prns **ByteStream** — Buffer over Channel over
Link over Reticulum. Every byte of git's pack protocol rides the stream, and git is
none the wiser: its built-in `ext::` transport bridges straight onto it, so there's
no custom remote helper.

The thing a clone **addresses is a Reticulum destination** — the server's
`git/serve` destination hash — never an IP. An IP appears only as the *interface
wire* (`--listen` / `--connect`): how two RNS instances physically find each other,
a stand-in for a radio or a LAN. Swap loopback for a real address and the exact same
code spans two machines.

## Try it on one machine

```sh
cargo run
```

Needs `git` and `nc` on `PATH`. This is the self-contained smoke: it stands up both
nodes in one process over loopback, builds a throwaway repo, clones it over a
ByteStream, prints the cloned history, and cleans up.

## Across two machines

On the serving machine, point `serve` at a real repo and let it listen:

```sh
cargo run -- serve --repo /path/to/repo --listen 0.0.0.0:4252
# prints:  destination: <32-hex-char destination hash>
```

On the other machine, **clone the destination hash** — the `--connect` address is
only how this node reaches the first one's interface (its LAN IP and port):

```sh
cargo run -- clone <destination-hash> ./into --connect <serving-host>:4252
```

The clone hears the server's announce to learn the route, brings up a link to that
destination, and runs `git clone` over the ByteStream. It never needs the server's
IP as a *git* address — only the destination hash.

## What it shows

- Standing up a `Prns` node from a recipe (`PrnsRecipe`) with a served destination.
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
- The interface here is a serial framing over a TCP socket — fine between two Prns
  nodes. The fully-Reticulum version peers both nodes with a shared transport node
  instead, so neither needs the other's IP at all: the path is discovered, and only
  the destination hash is ever named.

This is an example, not a packaged tool: read it, copy from it, point it at your own
repository.
