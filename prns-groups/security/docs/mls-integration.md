# MLS integration contract

`prns-groups-mls` integrates the unmodified published `mls-rs` 0.57.0, core
0.27.0 and RustCrypto provider 0.23.0 crates. The
[manifest](../mls-rs/Cargo.toml) declares the selected dependencies and the
lockfile records the exact resolved graph.

## Selected protocol profile

The [policy implementation](../mls-rs/src/policy.rs) selects
`CURVE25519_AES128`, Basic credentials, private controls and owner-authorized
commits. Credentials bind the endpoint, incarnation and signing key. A Basic
credential is not independently trusted: the caller supplies an authenticated
policy, and managed enrollment checks the exact resulting roster.

The selected features include `private_message`, `prior_epoch`, `out_of_order`,
`by_ref_proposal` and `fast_serialize`. X.509, PSKs and external-client behavior
are outside this profile. The adapter creates commits with an update path.
Incoming commits follow upstream MLS rules, including permitted pathless
add-only commits. This is a restricted application profile, not universal
interoperability with all MLS feature combinations.

The lower adapter exposes member Update and owner rekey operations. Their
availability does not install a periodic key-refresh or inactive-member policy
in the managed group client. An embedding should advertise only the operations
and policies that it implements.

## Admission limits and allocations

The adapter caps stored/wire representations, checks complete decoding of
incoming messages and key packages, and checks the requested group ID after
loading a group. Snapshot and prior-epoch decoding use upstream MLS behavior.
The adapter bounds admitted members, groups, proposals, outputs and retained
epoch count; it does not impose exact skipped-key or forward-derivation budgets.

Each operation reloads committed state and keeps its working MLS object inside
the transaction. Provider clones share that attempt; stale or out-of-scope use
fails. Upstream decoding and receive ratchet expansion can allocate and perform
work before an expanded snapshot reaches the storage-limit check. Decoded state,
serialization copies and transient buffers remain part of the embedding's memory
budget; this is not a total heap bound or a recoverable-OOM guarantee. Storage
quota exhaustion does not authorize resending an old record under a later
generation.

## One durable transaction

`GroupStateStorage` and `KeyPackageStorage` stage changes through the same
coordinator as application records. Current/prior state writes and consumed
KeyPackage deletion become durable with exact outgoing bytes and the operation
receipt. `ApplicationTransaction` adds application facts with read-your-writes.
The caller must publish only after a definitive commit.

On definite abort, the adapter discards working crypto state. On uncertainty,
recovery must settle the operation before new work. Exact retry reads committed
output instead of regenerating ciphertext. Provider deletion is idempotent;
a library write callback is staging, not proof that the outer commit succeeded.

The failure boundaries are distinct:

- Invalid MLS authentication or an unapproved roster/control transition aborts
  the attempt, including any provisional ratchet or membership mutation.
- Authenticated application rejection commits consumed receive state and its
  rejection fact, preventing the same ciphertext from becoming fresh input later.
- Successful application admission commits crypto, record facts, exact output
  and recovery evidence together.

[Composition tests](../mls-rs/tests/composition.rs) and
[recovery tests](../mls-rs/tests/recovery.rs) exercise those adapter boundaries.
The [transaction store](../../store/README.md) defines backend obligations.

## Secret lifetime and platform limits

The upstream storage API wraps `GroupState.data` and `EpochRecord.data` in
`Zeroizing<Vec<u8>>`. Other serialization paths still create plain temporary
buffers, including pending commit snapshots and encoded commit secrets.
Zeroizing a final result does not erase every earlier allocation or external copy.

Logical retirement and crash recovery do not prove physical erasure. Journals,
redb pages and backups can retain old snapshots. A complete valid-image rollback
needs protection beyond a counter stored beside that image. The
[secure NOR contract](../../store/docs/secure-flash.md) and
[embedded guide](../../core/docs/embedded.md) state their separate mechanisms and
limits. A plaintext test wrapping key provides no physical protection.

The adapter admits only `CURVE25519_AES128`, regardless of the provider's broader
compiled suite set. Tests exercise that selected integration; they do not qualify
other suites or replace a cryptographic audit.
