# Recoverable MLS adapter

`prns-groups-mls` implements one recoverable MLS operation at a time using
unmodified upstream `mls-rs` with `no_std + alloc`. The
[generic core](../../core/README.md) remains allocation-free; the
[transaction coordinator](../../store/README.md) is independent of MLS and SQL.
This crate has host recovery tests.
The [embedded support guide](../../core/docs/embedded.md) separates portable
library support from platform integration. Protected storage and a complete
networked board member require separate qualification.

## Using the adapter

1. Supply an exclusively owned `AtomicBackend` and explicit transaction limits to
   `Coordinator`. It must provide atomic changes, durable operation receipts,
   bounded reads and definitive recovery after an uncertain outcome.
2. Provision a `LocalIdentity` and a `Policy` containing the owner and explicitly
   approved endpoint/incarnation/signing-key bindings. Persist identity material
   before advertisement; `into_parts`/`from_parts` support platform storage. The
   adapter never replaces missing identity material automatically.
3. Construct `Adapter` with explicit `Limits`. Each store belongs to one endpoint
   and immutable trust configuration. Reopening with changed identity, owner or
   limits fails; configuration migration is not implicit.
4. Call `create`, `key_package`, `add`, `join`, `send`, `receive_with`,
   `propose_update`, `rekey` or `remove` using unique `OperationId`s. Byte bodies
   are application-defined. `receive_with` supplies the authenticated endpoint
   binding and plaintext to a pure admission callback. With the secure backend,
   encode consecutive `OperationTicket`s as those IDs; receipt reclamation must
   preserve their durable retired-prefix fence.
5. Publish or apply only a returned `Committed` result. On uncertainty, resolve
   `coordinator().recover(operation)` before more work. `retry(operation)` reads
   the exact committed bytes; it never encrypts again. At dispatch, use `retry`
   to recheck learned membership barriers. Acknowledge a consumed result with a
   separate operation ID after applying the application's effects.

The [host fixture](tests/support/mod.rs) demonstrates endpoint construction and
reopening; [recovery tests](tests/recovery.rs) demonstrate the complete flow.
Accepted plaintext and authenticated rejection decisions are durable inbox
results in the same transaction as consumed receive keys. The opt-in
[secure backend](../../store/docs/secure-flash.md) also finishes obsolete-key
capsule erasure before a successful result is released. Applying arbitrary
external application effects is a later step; this does not promise exactly-once
execution of those effects. Returned application buffers are scrubbed on drop and
redacted in debug output. The backend stores secret MLS snapshots and plaintext
inbox results and must supply the required at-rest protection.

## Composing application state

`compose(operation, Command, admission, stage)` uses the same transaction runner
as the standalone operations. The borrowed `ApplicationTransaction` exposes
only application `read`, `put` and `delete`, with read-your-writes. Its reserved
zero-byte key prefix separates application facts from MLS snapshots, indexes and
durable outputs. Application keys are at most 255 bytes and the extra prefix must
fit the coordinator's key limit. Values and aggregate staging share the explicit
coordinator budgets. Returned application values zeroize on drop.

For `Command::Receive`, admission receives the authenticated sender, plaintext
and application transaction view. It can check persisted deduplication and policy
facts. Returning `Admission::Reject` consumes the receive key and persists the
rejection; returning an application error aborts the entire attempt. The final
stage callback receives the provisional `Output` and can write records, immutable
targets, delivery obligations or receipt facts. MLS state, those facts, exact
output bytes and the operation receipt then publish together. Any failed
application storage call prevents commitment, even if its error is ignored.

Callbacks must not publish bytes or perform external effects: their output is
still provisional. A callback error or host unwind aborts and invalidates the
transaction. An uncertain commit fences both MLS retries and application reads
until recovery. After recovery, `retry(operation)` and `application_read(key)`
retrieve committed output and application facts without repeating callbacks or
encryption. The service should key immutable operation facts by operation ID;
separate reads do not constitute a multi-key snapshot. `local_binding()` exposes
the adapter's actual local credential for application author binding.

[Composition tests](tests/composition.rs) cover atomic staging, ignored storage
errors, namespace isolation, accepted/rejected receive, callback failure/unwind,
and exact recovery across torn and uncertain publication. Enrollment, application
schemas and remote delivery semantics remain the service's responsibility.

Every operation reloads committed state. Group, prior-epoch and key-package
providers share the same fenced transaction. No live MLS `Group` escapes. MLS
errors, host callback unwinding and unapproved control transitions discard the
attempt; authenticated application rejection commits its consumed keys and
rejection evidence instead.
The owner must sign both commits and Welcome. Member controls are private, and
locally created commits include an update path. Incoming commits follow upstream
MLS rules, which permit pathless add-only commits. Non-owner leaf updates survive
reload until the owner commits them.

## Owner-managed enrollment

`Policy::new` fixes the configuration and immutable allowlist.
`Policy::owner_managed` uses a separate encoding that pins the owner binding and
local identity while allowing the caller to supply an updated authenticated
allowlist. Changing the owner, local identity, limits or policy mode still fails
on reopen. The policy does not authenticate an arbitrary supplied roster.

Use `compose_with_roster` for Create, Add, Remove, Welcome and received controls to check
that the resulting MLS tree exactly matches the owner-authenticated roster before
committing. An allowlist alone accepts subsets and is insufficient for this check.
A mismatch aborts MLS changes and application staging together. The caller also
checks control parent, epoch, full routing-to-MLS credential mapping and immutable
owner authority. `key_package_matches` binds a directed recipient's claimed
credential to its encoded KeyPackage; MLS performs full cryptographic and lifetime
validation during Add. Add and Remove advance the authorization barrier and purge
prior epochs. The managed profile accepts exactly one Add or one non-owner Remove
per commit; routine rekey and owner transfer remain excluded. Use
`Command::RemoveMember` with the exact endpoint/incarnation/signing-key binding;
the adapter resolves its current MLS leaf inside the transaction. For removal,
the authenticated allowlist must still contain the prior roster during processing,
while the expected roster names exactly the remaining members. A removed receiver
validates that successor against the authenticated single Remove before erasing
its live state and returning `Event::Removed`.

`Command::Retire` is a local cryptographic terminal operation for an authenticated
group closure. The caller verifies the owner-signed control and atomically stages
its terminal facts and exact notification work with this command. It does not
create an MLS epoch or empty roster. `Event::Retired` records the outcome; live
state and retained epochs are deleted, and a durable terminal marker fences
application/proposal retries and future activity. Application history and retained
control results remain readable. The group ID stays reserved; fresh enrollment
uses a fresh credential and store incarnation, not clearing this terminal marker.

`LocalIdentity::sign_enrollment` and `MemberBinding::verify_enrollment` sign and
verify domain-separated public metadata without exporting private keys. Pin this
key using an authenticated enrollment channel before trusting it. The native
[enrollment engine](../../src/enrollment/mod.rs) stages owner-signed
manifests and exact Commit/Welcome bytes with MLS operations; its catalog owns
local identity admission and durable retries. [Managed policy tests](tests/managed_policy.rs)
exercise configuration stability, exact-roster rollback and signature rejection.
[Management tests](tests/management.rs) cover owner-only removal, remaining and
removed recipients, terminal-state recovery and exact retry fences.

## What is bounded

| Resource | Enforcement |
| --- | --- |
| Staged changes, key/value bytes, aggregate staging bytes | Coordinator checks before fallible copies; shared handles have read-your-writes and stale-token rejection. |
| Incoming wire and persisted crypto record bytes | Capped before MLS decoding. Incoming messages and key packages require complete decoder consumption; loaded groups must match the requested group ID. |
| Groups, key packages, proposals, pending results and admitted members | Persistent bounded indexes/counters, explicit limits and credential/roster checks. |
| Old epochs | Oldest epochs are removed when the count cap fills. Add/remove barriers purge every previous epoch and block stale outgoing application/proposal retries. |

Limits remain local configuration, not a claim that arbitrary peers support this
restricted MLS profile. Capacity failure preserves committed state. Decoding of
MLS snapshots and retained epochs follows upstream behavior; the adapter does
not add validation of private MLS serialization fields.

**Not all MLS allocations are fallible.** Wire/snapshot size and logical quotas
bound admitted representations and stored state. Upstream decoding and receive
ratchet expansion can allocate before member/count validation or rejection of an
expanded snapshot, and serialization makes temporary copies. Skipped-key retention
follows upstream MLS; the adapter does not impose exact per-sender, per-epoch or
device-wide key budgets. These limits are not a total heap/stack bound or a
recoverable-OOM guarantee. Backend indexes, receipt retention, compaction and
allocator fragmentation need separate budgets. The file fixture serves only host
tests; it is not a production allocation or erasure guarantee.

## Integration boundaries

- Prior-epoch retention is count-based. Trusted-clock epoch age limits and a
  timed grace policy are not implemented.
  Key-package generation requires an explicit timestamp and rejects values below
  the library's one-hour skew allowance. Clock trust remains the caller's duty.
- The fixed policy keeps immutable credential bindings; the optional
  owner-managed policy allows an authenticated evolving roster under one fixed
  owner. Authority discovery, profile negotiation, owner transfer and credential/config
  migration are outside this adapter. A removed group cannot rejoin within the
  same adapter namespace: its tombstone retains the group slot. The managed owner
  performs readmission using a fresh credential and namespace instead.
- Expired/abandoned key-package reclamation and tombstone retirement need explicit
  lifecycle rules. Secure storage has explicit receipt-prefix finalization;
  the owner must attest that every result in the prefix is durably handled.
  Output acknowledgement
  frees output capacity but never permits reusing its committed operation ID.
- Generic records, delivery obligations, subscriptions, receipts, repair and the
  Reticulum transport are not implemented here. Immutable control outputs remain
  retrievable for catch-up; old application bytes cannot be retransmitted through
  `retry` after a known membership barrier. Previously delivered plaintext is
  application history, with its own retention policy.
- The [NOR backends](../../store/README.md) require entropy and a wrapping key
  supplied by the embedding. Host software resets at completed flash calls do
  not qualify brownouts or physical confidentiality. Protected provisioning, electrical cuts, loaded
  LoRa, stack margins, rollback resistance and trustworthy time remain gates.
- The selected adapter profile and coordinator have host tests and strict Clippy
  checks; these do not constitute a cryptographic audit or target qualification.

No Reticulum core protocol changes are involved.

## Provider dependency profile

The adapter selects `CURVE25519_AES128` and uses the published RustCrypto
provider with defaults disabled. The [manifest](Cargo.toml) declares the upstream
crates and the lockfile selects their exact resolved versions. Other algorithms
compiled by the provider are not admitted by this adapter's policy. The normal
dependency advisory gate checks the selected graph; changing the allowed suite
requires separate protocol and integration qualification.

## Validation

The registered suites exercise adapter recovery, storage limits and delayed
receive, alongside the core/coordinator and both NOR backends. The file backend simulates four cuts around
snapshot publication and reopens the actual files with fresh objects. These are
simulated crash-boundary tests, not process-kill or physical power-loss tests.
Real-MLS/NOR tests also reconstruct both endpoints after partial-body,
pre-publication and post-publication cuts, verifying exact outbox retry and
atomic receive-key/inbox persistence. NOR unit tests sweep byte-level cuts.
Secure-storage MLS tests additionally cover encrypted identity reload,
capsule retirement and compaction interruptions, exact output recovery, consumed
key packages/receive generations, delayed receive across rekey, removal and
permanent receipt floors. The one-retained-epoch profile also guards the provider's
epoch-eviction dispatch: it must delete an epoch through the raw transactional
provider, not the similarly named key-package trait method.

```sh
python3 validation/run.py run --suite group-communication-tests --suite group-communication-clippy --suite group-mls-adapter-tests --suite group-mls-adapter-clippy --platform current
RUSTFLAGS='--cfg getrandom_backend="unsupported"' cargo +esp check --locked --offline --manifest-path prns-groups/security/mls-rs/Cargo.toml --lib --target xtensa-esp32s3-none-elf -Z build-std=core,alloc
```

The S3 command uses the configured Espressif toolchain and checks only the
adapter and its dependencies. The explicit `getrandom` unsupported backend makes
this a compilation check with no working entropy. A production embedding must
supply a supported entropy backend for `getrandom` 0.4. Core/store also have
`no_std` library checks on Cortex-M4F and RISC-V. A library build does not establish
working target entropy, firmware resource limits or physical behavior.
