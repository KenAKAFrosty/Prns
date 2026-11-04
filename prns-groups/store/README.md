# Group transaction coordinator

`prns-groups-store` is `no_std + alloc`. It coordinates one bounded transaction
across MLS storage providers, application facts, receipt/custody state and exact
outgoing bytes. Its `flash` module supplies an append-only NOR backend; this is not
a complete group engine.
The dependency-free, allocation-free `prns-groups-core` core supplies IDs, scoped
operation tickets, revisions and checked capacity accounting.

The opt-in `secure` feature supplies [encrypted NOR storage](docs/secure-flash.md)
with immutable versions, erasable catalog-key capsules, strict retirement before
result release, compaction and a bounded receipt window. This experimental format
has host tests. Protected provisioning, electrical interruption and physical
confidentiality require separate platform qualification.

`Coordinator::begin(operation_id, expected_revision)` returns cloneable transaction
handles. They share read-your-writes state and are fenced by a generation token.
`put` and `delete` coalesce by key. `commit` returns committed, definitely not
committed, rejected or uncertain; a backend error also means uncertain. Uncertainty blocks
new work and committed reads until `recover(operation_id)` gives a definitive
answer. Aborted and completed handles remain invalid, including when an aborted
operation ID is retried. Call `abort` explicitly before abandoning active work.

The `AtomicBackend` exclusively owns durable bytes. It must atomically install all
mutations with an operation receipt and revision, enforce bounded reads before
allocation, and distinguish missing from untrustworthy recovery evidence. Durable
receipts prevent reapplying committed operation IDs. The secure backend replaces
finalized receipts with a permanent sequence floor; other backends must define
their own bounded receipt lifetime and replay policy before deployment.
No namespace has special atomicity: crypto, key-package deletion, application
facts and outbox entries must be staged through the same transaction.

Limits bound distinct mutations, key/value lengths and total staged payload bytes.
Reservation and copies use `try_reserve_exact`; quota errors preserve prior staged
work. Staged values are zeroized on replacement, abort and completion, and Debug
omits bytes. Returned read buffers are caller-owned and need immediate zeroizing
ownership when they contain secrets. Backend copies/journal storage have their own
erasure obligations. Logical deletion and RAM zeroization do not erase flash.

These are payload bounds, not a total heap guarantee. Replacement can temporarily
hold old and new buffers; Vec metadata, backend indexing and other allocations add
overhead. The constructor's one-time Arc allocation is infallible. The spin mutex
requires external serialized scheduling: do not call from interrupts or reenter
from a backend. This is not an OOM-safe claim for MLS or a board resource budget.

Unit tests use an atomic map fake for coordinator invariants and NOR simulators
for both backends. The MLS integration exercises secure storage as well as the
original file and NOR fixtures.
These host tests do not prove physical power-loss behavior.

## Encrypted NOR backend

Enable `secure` and construct `secure::SecureBackend<F, K, R>` with an exclusive
raw NOR driver, partition, `SecureLimits`, nonzero `StoreIncarnation`, protected
`KeyProvider`, and fresh cryptographic `Entropy`. `format` requires a completely
erased partition; `mount` requires an existing authenticated state and never
initializes missing state. Geometry, limits and incarnation are bound to the
format. Changing them requires an explicit migration, not a mount with new limits.

Use `OperationTicket::new(incarnation, sequence)?.operation_id()` with consecutive
sequences starting at one. A definitely aborted attempt may retry its ticket;
committed tickets may not name different work. Foreign, invalid and out-of-order
tickets are rejected. Exhausting `max_receipts` stops new transactions while
retained results and maintenance remain available.

`Coordinator::maintain(SecureRequest::FinalizeThrough(ticket))` attests that every
result through that ticket is durably handled and no longer needs commit-result
recovery. This is a trusted owner decision; storage does not infer application
acknowledgements, expire receipts or delete application outputs. Retired tickets
remain terminal (`RecoveryOutcome::Retired` / `OutsideRecoveryWindow`) after
reopen and compaction. They never become `NotCommitted` again. Resolve uncertain
transactions before maintenance, and finalize only a contiguous completed prefix.

`Compact` copies live ciphertext into the spare arena and publishes new locations
without changing the application revision. `EraseRetired` completes ciphertext
cleanup. Maintenance errors fence all coordinator access until
`recover_maintenance` resolves the attempt. Capsule erasure is required before
successful reads, commits or recovery release secret values or dispatchable
results. See the design for the byte format, bounds and fail-closed fault model.

## NOR flash backend

`flash::FlashBackend<F>` accepts an `embedded_storage::nor_flash::NorFlash`, an
explicit `Partition { offset, length }`, and `FlashLimits { slot_bytes,
max_changes, max_key_bytes, max_value_bytes }`. The caller must validate partition
table ownership and versioning, exclusively serialize access, and supply erased
storage initially. Mount never erases or silently repairs an error. An entirely
erased partition is revision zero; an unreadable or corrupt journal is an error.
The driver must complete each write durably before returning success.

Each erase-aligned fixed slot holds one transaction and receipt: a 64-byte
versioned header, mutations with 12-byte headers, CRC-32, padding, and a separate
256-byte final publication marker. Keys and values are streamed through a
256-byte zeroizing buffer. Data must fit `slot_bytes - 324` after mutation headers.
Partition offsets and slots must be aligned to both erase size and 256 bytes;
the driver's read/write sizes must divide 256. Reopen validates all published
records, consecutive revisions and unique committed operation IDs. Reads verify
record integrity and enforce the value bound before allocation. RAM contains a
fallibly allocated metadata index, reserved to `partition.length / slot_bytes`
entries, rather than a clone of all values. Scans and reads are linear in journal
size, so partition/slot counts are also latency bounds to qualify on the board.

There is no compaction, receipt expiry, or automatic erase. Once every slot is
used or retired, `Full` prevents writes while reads and committed recovery remain
available. Four MiB with 64 KiB slots permits at most 64 committed transactions;
interrupted writes can reduce that count. Capacity planning must include identity
provisioning, MLS operations, application updates and acknowledgements. Reclaiming
space requires a future design that preserves operation-ID replay protection and
the atomic generation of all remaining records.

An interrupted body with a completely erased publication marker never committed:
its occupied slot is retired, and recovery can return definite non-commit. A
fully published record exposes all mutations and its receipt together, including
when the caller lost the write result. A partial publication marker is ambiguous
with corruption of a previously published marker, so it **fails closed** instead
of being treated as absence. Mount/recovery must surface that error; applications
must not automatically erase and regenerate MLS state. This conservative initial
journal does not offer automatic availability after every possible power cut.
The host NOR tests cut programming at every byte across the body and marker,
including a complete marker followed by a lost result, and test corruption,
duplicate receipts, revision gaps, tombstones, capacity and partition isolation.

CRC-32 detects accidental corruption; it does not authenticate storage, prevent
physical rollback, or prove secure erasure. The fault model assumes exclusive
ownership and that an erased marker cannot result from an external erase of a
previously committed record. Fixed append-only slots retain **superseded MLS
secrets, removed keys, identity secrets and any stored application plaintext**.
Deleting a record or zeroizing RAM does not provide forward secrecy against later
raw-flash access. On unencrypted flash, an attacker with raw access can read this historical
material. Flash encryption, authenticated
state, rollback defense and a safe erasure/compaction policy remain separate
production requirements; the backend makes none of those guarantees.
