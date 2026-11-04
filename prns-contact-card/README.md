# Signed contact cards

`prns-contact-card` defaults to an allocation-free `no_std` building block for exchanging
one public Reticulum identity and one service. It has no dependency on the prns
app, React Native, a camera, discovery, a database, or group messaging. Other
applications choose their own service name hash and adapt their identity signer.
QR codes, links, text, files, Bluetooth and NFC can carry the same bytes; this
crate implements the binary, token and URI formats, not those transports.

The codec accepts any supplied identity/signer, so separate named identities can
each export this same format. A card does not list or link sibling identities.
Selection, activation and relationship storage belong to consumers; these are
not codec features. With the optional `prns-identity` feature, core `PrivateIdentityMaterial` directly
implements `ContactCardSigner`; `encode_identity` derives the public fields from
that same identity. Native owners can pass a lease's borrowed `material()` without
exporting secret bytes or introducing a second key owner. The default codec still
has no PRNS identity, native, host or catalog dependency. Both profiles remain
allocation-free and `no_std`. Identity lifecycle and consumer consent remain
separate integrations.

Version one is a **draft application-level format**, not an established Reticulum
standard or a change to Reticulum packets. The `rns-contact` URI scheme is part
of that draft. Registering a handler is an embedding application's responsibility;
custom schemes do not establish exclusive ownership or open a network route.

## Use and trust

1. Set `CardFields` to public identity bytes, a ten-byte service name hash, and a
   display name. `service_name_hash("lxmf.delivery")` implements Reticulum's
   dotted-name hashing; another application can use another fully qualified name.
2. Implement `ContactCardSigner::sign` on the caller's identity owner. It signs
   the exact supplied bytes using ordinary Ed25519 and never hands private key
   bytes to this crate. `encode_signed` writes into a caller-provided buffer only
   after checking that the resulting signature matches the supplied identity.
3. Use `encode_token` or `encode_uri` for text/QR transport. These wrappers do not
   authenticate their input.
4. On receipt, `decode_uri` or `decode_token` decodes bounded bytes, then `verify`
   returns `VerifiedContactCard`. A successful decoder alone is not acceptance.
5. Check the application's supported service, compare any previously saved
   identity binding, and ask the user before importing or inviting. Importing a
   card must not silently replace a conflicting identity or private contact name.

Verified fields bind the complete public identity, the service and the exact
display name. The identity hash is derived from the public keys, and the service
destination is derived from that hash and the signed service hash. Neither is a
separate caller-controlled field. The signature proves that the Ed25519 private
key authorized the claim. The name remains self-asserted; it does not establish
the person's real-world identity, physical proximity, live possession of the
X25519 key, network reachability, or group membership. Ordinary Reticulum key
agreement/authentication remains necessary for communication.

Cards contain no private key, MLS material, group membership or bearer invitation.
Version one has no expiry, replay counter or freshness claim: repeatedly importing
the same contact is expected to be idempotent, and an old signed name can be
replayed. Existing trust/conflict handling and label-update policy belong to the
consumer. New key identities must not silently replace pinned older identities.

## Canonical version-one wire

All offsets are byte offsets. The format has exactly one service and no optional
extensions. Unknown versions and trailing data are rejected.

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 8 | ASCII `PRNSCARD` |
| 8 | 1 | Version, exactly `1` |
| 9 | 1 | UTF-8 display-name byte length, `0..=128` |
| 10 | 64 | Reticulum public identity: X25519 public key then Ed25519 public key |
| 74 | 10 | Reticulum service name hash |
| 84 | name length | Display-name UTF-8 bytes |
| 84 + name length | 64 | Ed25519 signature |

Signature input is the literal bytes `prns.contact-card.v1\0` followed by every
wire byte before the signature. `\0` is one zero byte. Key and signature points
must be nonidentity prime-order Edwards points; strict Ed25519 verification is
used. The X25519 bytes are signed and hashed, but this offline codec does not
perform key agreement.

The display name is not normalized. Empty names and Unicode are allowed; Unicode
control characters, U+2028/U+2029 line separators, and bidi controls U+061C,
U+200E/U+200F, U+202A..U+202E and U+2066..U+2069 are rejected. Applications should
still render identity fingerprints separately from untrusted labels.

Derivation uses SHA-256 and first-byte truncation:

```text
service_name_hash = SHA256(UTF8(fully_qualified_dotted_service_name))[0..10]
identity_hash     = SHA256(public_identity)[0..16]
destination_hash  = SHA256(service_name_hash || identity_hash)[0..16]
```

The token is unpadded base64url of the complete signed wire. Only `A-Z`, `a-z`,
`0-9`, `-` and `_` are accepted. Unused final sextet bits must be zero; padding,
whitespace and alternate spellings are rejected.

The complete URI is exactly `rns-contact://v1/<token>`. It contains no query,
fragment, trailing slash, user information, port or percent encoding. Scheme and
version casing are exact. Consumers must pass the original URI to the validator;
normalizing it in a generic URL parser would weaken this canonical-input rule.

## Bounds and checks

The complete card is 148–276 bytes; its token is at most 368 ASCII bytes and its
URI at most 385 bytes. The default encoders/decoders use caller-owned output and
leave that output unchanged on errors. Verification borrows its input. Bounded
stack scratch is used for signature input and token validation; there is no
allocation or implicit retained state. Cryptographic dependencies also disable
their default features.

```sh
cargo test --manifest-path prns-contact-card/Cargo.toml --no-default-features
cargo clippy --manifest-path prns-contact-card/Cargo.toml --all-targets --no-default-features -- -D warnings
cargo build --manifest-path prns-contact-card/Cargo.toml --no-default-features --target thumbv7em-none-eabihf
cargo build --manifest-path prns-contact-card/Cargo.toml --no-default-features --target riscv32imac-unknown-none-elf
```

Tests cover signed field tampering, domain separation, malformed/weak key material,
unknown versions, exact framing, Unicode/name limits, all valid output lengths,
strict token/URI spelling, signer failures and unchanged output after rejection.
Cross-language fixtures under `tests/vectors/` provide independently produced
Ed25519 cards for more than one service.

`examples/telemetry_contact.rs` is a small independent consumer: it verifies a
telemetry card from those fixtures, applies its own supported-service policy,
derives the identity/destination, and round-trips the canonical URI. It imports no
chat application, service runtime or group crates. The example uses `std` and
dev-only JSON/hex dependencies for fixture loading and printing; the library and
its production dependencies remain `no_std` and allocation-free.

```sh
cargo run --manifest-path prns-contact-card/Cargo.toml --example telemetry_contact --no-default-features
```

## Optional owned and foreign verification

The `alloc` feature adds `api::verify_contact_card(Vec<u8>) -> ContactCardClaims`
and `api::service_name_hash(String) -> Vec<u8>`. The former calls the same bounded
`verify` before allocating the public projection: complete public identity,
service hash, display name, derived identity hash and destination hash. The
existing `CardError` preserves structural and signature rejection reasons.
Constructing a claims record yourself does not prove verification or grant
authority. No signer, key owner, catalog, service registry or network is needed.

`api::encode_contact_card_uri` and `api::decode_contact_card_uri` adapt the same
bounded URI carrier to owned strings/bytes. They preserve its exact spelling and
reject normalization aliases; decoding is separate from signature verification.
Foreign callers receive the canonical `TokenError` variant and diagnostic text.
The caller-buffer size fields remain native-only; owned calls supply the fixed
maximum output capacity internally.

`uniffi-bindings` selects that owned API and generates its declarations directly
from this crate. Native images retain the namespace through
`prns_contact_card::uniffi_reexport_scaffolding!()`. This optional feature uses
`std` for foreign scaffolding; `alloc` alone remains `no_std`, and the default
codec still requires neither allocator nor foreign tooling.
