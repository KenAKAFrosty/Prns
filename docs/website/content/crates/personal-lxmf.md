## When to reach for this

LXMF is the message layer on top of Reticulum — the part most apps
actually want. If you're building:

- A messaging or chat app.
- A delivery system where messages need to wait for the recipient
  to come back online.
- An app where every user has a cryptographic identity baked in.

…then this is the surface you'll be talking to. The Reticulum engine
underneath handles routing; LXMF gives you a usable conversation on
top of it.

## What you get

The same shape that Sideband and Nomadnet use:

- **Identities** — every participant is a key pair, not an account.
- **Addressing** — durable hashes you can save and share.
- **Delivery** — messages persist until the recipient receives them,
  even if both ends are intermittently online.

The surface is small on purpose; the goal is "send a message, get a
delivery callback," not a full social platform.

## Status

The LXMF layer is currently a scaffold. The shape is in place; the
behavior arrives as the engine underneath exposes the primitives it
needs (links and resources are the next pieces). Until then, treat
this crate as a marker for the API surface that's coming.
