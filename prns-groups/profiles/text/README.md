# Group text profile

`prns-group-text` is an allocation-free `no_std` profile over generic authenticated
group records. It depends only on the allocation-free group core. It creates no
storage, task, transport, encryption state or application owner.

Version 1 uses namespace `0x74657874` and schema `1`. Its exact body is `GTX`,
version byte `1`, a two-byte big-endian UTF-8 byte length, then 1–2048 text bytes.
Parsing borrows the input; encoding writes to a caller-owned buffer and checks
capacity before modifying it. Unknown versions, invalid UTF-8, truncation and
trailing bytes fail explicitly. Text is preserved, including Unicode, whitespace
and line breaks. Applications decide presentation and composition rules.

```rust
use prns_group_text::{Text, MAX_ENCODED_BYTES};
let mut buffer = [0; MAX_ENCODED_BYTES];
let encoded = Text::new("Meet at the trailhead")?.encode(&mut buffer)?;
assert_eq!(Text::decode(encoded)?.as_str(), "Meet at the trailhead");
# Ok::<(), prns_group_text::Error>(())
```

The enclosing record provides the group, message ID, authenticated author endpoint,
publication time and original delivery targets. This body never overrides those
facts. The native runner adds a conversation projection and uses the publication
nonce as the caller's idempotency key. Local sequence numbers order insertion on
one endpoint; they do not claim a shared total order. Device receipt counts are
not human-read receipts.

Attachments, edits, reactions, names, typing state, read markers and cross-device
draft synchronization are outside this first profile. A text body is not LXMF.
Other application namespaces continue to use the generic service unchanged.

Tests cover a stable Unicode fixture, exact byte limits, non-mutating output
failure, borrowed decoding and malformed inputs. Check the embedded boundary with:

```sh
cargo check --locked --manifest-path prns-groups/Cargo.toml -p prns-group-text --target thumbv6m-none-eabi
```
