## When to reach for this

Use the Reticulum Visual Toolkit when you want to *see* what's
happening on the mesh — or when you want to reproduce a bug.

- Diagnose why an announce isn't reaching a node.
- Compare two routing scenarios side by side.
- Reproduce a packet sequence deterministically, the same way
  every time.
- Develop the protocol itself with a tight feedback loop.

It's not a production tool; it's a microscope you point at the
protocol.

## What you get

A multi-node simulator that runs many engine instances on one
thread, on a virtual clock, with a virtual wire between them.
The engines are the real ones — same code, same `ingest` and
`tick` — just driven by a deterministic harness.

Because the clock is virtual, replay is exact. The same scenario
produces the same trace, byte for byte. That's what makes the
toolkit a *debugger* and not just a demo.

## Status

Sim mode runs today as a Dioxus desktop app. The UI has no
desktop-only APIs, so the same view will ship to the web as a
zero-install URL once the build is wired. A live mode — same UI,
real nodes on a real network — is queued behind it.
