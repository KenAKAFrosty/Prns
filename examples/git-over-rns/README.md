# git over RNS

A real `git clone` carried over a Prns **ByteStream** — Buffer over Channel over
Link over Reticulum.

Two nodes stand up over one loopback link. One serves a throwaway git repository
through stock `git-upload-pack`; the other clones it. Every byte of git's pack
protocol rides the stream, and git is none the wiser: its built-in `ext::`
transport bridges straight onto it, so there's no custom remote helper.

## Run it

```sh
cargo run
```

Needs `git` and `nc` on your `PATH`. The example builds its own demo repository and
clone target under a temp directory and removes them when it finishes. On success
it prints the cloned history:

```
OK: git clone over ByteStream pulled the repo. Cloned history:
4c181f8 first commit, carried by ByteStream over RNS
```

## What it shows

- Standing up a `Prns` node from a recipe (`PrnsRecipe`), with a pre-configured
  served destination.
- Bringing a link up with `handle.establish_link(dest).await`.
- Opening a bidirectional byte stream with `handle.byte_stream(link, rx, tx).await`
  and treating each half as an ordinary `AsyncRead` / `AsyncWrite`.
- Bridging an arbitrary stdio protocol (here, git's) onto the stream with nothing
  but `tokio::io::copy`.

It runs both nodes in one process over loopback. The same two halves — a serving
node and a cloning node — split cleanly across two machines once each side dials a
real address instead of `127.0.0.1`.

This is an example, not a packaged tool: read it, copy from it, point it at your
own repository.
