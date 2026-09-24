# LXMF wire and interoperability provenance

The bounded parser, MessagePack scanner, composer, stamp/ticket primitives,
and corpus tests were selectively transplanted from
`reticulum-rs-firmware` commit
`a333f703f08c4fd03c1d1f2576d74b26b70a6e26`. The initial bounded wire work
entered that history in commit
`8abd2efe6c88df71bec09e102376414939df03a0`. Product routing, persistence,
Sideband fields, UI, and product attempt/receipt policy were intentionally not
copied.

The behavior oracle is Python LXMF 1.1.0 at
`795fdaa2b0777c13033787d933d1afc94a2377cb` with Python Reticulum 1.5.2 at
`ea98db4f53dcf0defc0e71a16e60d28b1229c4e6`. The generator records and checks
the exact `LXMessage.py`, `LXStamper.py`, and vendored `umsgpack.py` digests.
The generated vectors contain public deterministic test identities only.

Python LXMF and Reticulum are governed by their upstream Reticulum License.
They are used as independently installed test authorities; their source is not
vendored into this crate.
