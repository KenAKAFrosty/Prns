# Standalone identity bindings

This adapter exports the native `keys::KeyOwner` and retained `KeyLease` behavior
through UniFFI. It converts paths and public hashes, retains native ownership,
and projects typed errors. Creation, recovery, filesystem checks, locking,
publication, and key encoding remain in `prns-identities::keys`.

A foreign caller creates a new dedicated directory with `KeyOwner.createNew`,
records its public identity hash, and subsequently calls `openExisting` with that
immutable binding. `recoverCreation` is only for an uncertain initial creation
before that binding was recorded; it never generates missing key material.
An I/O failure can follow a partially completed write: preserve the directory,
and do not reinterpret failure as permission to create a different identity.

Dropping an owner releases its interest. Every opaque lease retains the exact
native key and directory lock until its last use ends. The Rust-only
`KeyLease::key_lease` lets a domain's construction adapter retain that
capability. It is absent from generated foreign metadata, as are root paths,
private material and unrestricted signing functions. The default image also omits the catalog
factory and native owner lane.

The adapter is an `rlib`; `image/` supplies a standalone `prns_identity_keys`
shared library. Both are members of this independent workspace and use one
lockfile. A future aggregate image uses the same adapter and
`prns_identities_uniffi::uniffi_reexport_scaffolding!()` rather than introducing a
second key representation or loading another image with the same namespace.
The lower-level Rust group API remains independent of this binding crate.

Swift validation uses the shared binding recipe and compiler helper.
It obtains its own keys from paths, verifies lease retention and typed errors,
reopens the exact public identity, and checks the actual generated FFI inventory.
No catalog, host, ambient executor, React Native generator or npm is required.

Run from the repository root on macOS:

```sh
CARGO_TARGET_DIR="$PWD/target" python3 prns-identities/bindings/uniffi/tests/check_swift.py
```

`CARGO_NET_OFFLINE=true` uses already-cached dependencies. The generated header
inventory must contain only the reviewed key constructors and public/lease
methods; this guards exposure without declaring a permanent physical ABI.

## Optional catalog owner

Select `catalog` on the adapter or image to add `IdentityCatalogOwner`. Its
create/open/client/close factory owns the native `Owner<4, 32, 4, 16, IdentityOnly>`:
four identities, 32 retained operation slots, four service declarations per
identity, and 16 runtime lease slots. The caller supplies the owning `Options`
and runtime `Budget`; queue, memory and resource checks run before filesystem
access. No service, host, database, or implicit activation is installed.
Applications that compose services construct their selected native owner and
return the same canonical client.

Every catalog operation and its model is generated from the owning core,
runtime and native declarations. This adapter contains no parallel catalog
method list or DTO conversion layer. The fixed-capacity core remains portable;
the native API owns the checked collection projection used by both Rust and
foreign callers. Typed error categories preserve authority, creation, storage,
runtime and submission failures; uncertain writes require exact-request recovery.

Async catalog methods admit on first poll. Losing an already-admitted result
future does not cancel its work. Persist creation IDs and complete transition
inputs before submission. Opening an existing catalog does not activate saved
enabled identities; recovery is explicit. `shutdown()` fences synchronously and
returns a retained observer whose `wait()` confirms actual worker drainage.
Borrowed clients cannot keep admission open after the owner closes or finalizes.
This operation is distinct from generated Kotlin `AutoCloseable.close()`, which
disposes the foreign reference without returning a drain observer.

With this feature, `KeyLease.importKey()` returns an opaque retained import
capability. It retains the source key owner/lock and copies material only inside
the zeroizing native key implementation. Neither private bytes nor a secret
constructor is exported. Rust callers can continue importing their existing
native `IdentitySecretKey` through `api::ImportKey::from`.

```sh
CARGO_TARGET_DIR="$PWD/target" python3 prns-identities/bindings/uniffi/tests/check_swift.py --catalog
```

This uses the same independent image and shared generator/compiler helper. The
consumer creates, retries, renames, activates, pauses, closes and reopens a catalog,
imports an opaque key capability, and verifies typed checked-input errors. Both
profiles check their actual generated metadata and resolved dependency closure.
The default stays Tokio-free; catalog uses only Tokio's sync feature and includes
no redb, host, groups, LXMF, React Native or ambient executor.

## Optional service-neutral contact cards

Select `contact-card` on the adapter or image to expose
`KeyLease.contactCard(serviceNameHash, displayName)`. It checks the ten-byte
service hash, delegates name validation and issuance to the native owner's
canonical codec, and returns signed public wire bytes. The retained lease keeps
the same key and directory lock alive through issuance. Malformed input is
`KeyError.InvalidContactCard`; an issuer failure is `KeyError.Internal`. No secret
bytes or unrestricted signer crosses the foreign boundary. This feature alone
does not select the catalog owner, Tokio, host, groups, LXMF or redb.

Combine `catalog,contact-card` to use the owning native
`IdentityClient.contactCard` method directly. It obtains current offline-signing
authority on the catalog lane, including for a paused identity with no services.
Issuance does not configure a service or activate networking. Invalid public
input returns the canonical `IdentityApiError.InvalidInput`; storage, authority
and admission failures retain their existing typed categories.

Both profiles also include the `prns-contact-card` namespace's
`verifyContactCard` and `serviceNameHash`. Verification returns bounded public
claims after the owning codec checks framing and signature. Consumers apply
their own supported-service, pinned-identity and consent policy; a signed name
is self-asserted and a card grants no group admission or network reachability.
An aggregate image anchors this namespace with
`prns_identities_uniffi::contact_card::uniffi_reexport_scaffolding!()` alongside
the existing identity adapter anchor, selecting the same `contact-card` feature.

```sh
CARGO_TARGET_DIR="$PWD/target" python3 prns-identities/bindings/uniffi/tests/check_swift.py --contact-card
CARGO_TARGET_DIR="$PWD/target" python3 prns-identities/bindings/uniffi/tests/check_swift.py --catalog --contact-card
```

The generated Swift consumers issue cards for arbitrary services, use canonical
verification and independently check Ed25519 signatures/identity hashes with
CryptoKit. They test malformed hash lengths and names, retained key ownership,
paused catalog issuance with no services, and exact card bytes after reopen.
Every profile checks the actual generated metadata and shipping dependency
closure separately from the generator's dependencies.
