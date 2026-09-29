# Controller-grant persistence across abrupt power removal

The owner-local NOR campaign now exercises production controller-grant snapshot
storage and restoration. A confirmed table contains an administrator and an
operator. One replacement changes only the operator's allowed request; another
removes the operator while retaining the administrator. Expected tables are
explicit typed grants, compared in canonical identity order after every reboot.

Each replacement runs both with append space and with a full arena requiring
compaction. The uninterrupted production store supplies the bounded operation
trace. Every write/erase byte prefix and both boundaries of every read become
an abrupt-removal trial. I/O stays pending at the selected boundary; the harness
drops the write future and reconstructs fresh owners from surviving bytes only.

| Replacement | Append cuts | Compaction plus append cuts |
| --- | ---: | ---: |
| Operator permission change | 197 | 1,555 |
| Operator revocation | 133 | 1,491 |

All 3,376 trials compare the exact trace prefix and restore twice. Before the
complete final record commit word, recovery must yield the full confirmed table.
Afterward it must yield the full replacement, even when the store never received
acknowledgement. Authority, identities and request sets are compared together;
no partial grant table, lost administrator or resurrected committed revocation
is accepted. Restore counts must also match, with no refused or dropped entries.

The owner's cached durable snapshot must remain confirmed while an interrupted
write is pending and adopt the candidate only after successful storage. The live
authorization table stays unchanged throughout the store call, including success:
activation belongs to the caller's transaction, not this persistence API.

## Scope and execution

Production impact: none. The tests reuse the owner-local bounded flash fault
model and real grant codecs, persistence owner and authorization restore path.
They do not exercise pairing transcripts, grant-management admission, live
activation/rollback, target-access snapshots, physical flash or a whole-node boot.
Fixture public keys are identifiers for storage tests, not signature evidence.

These cases run in the registered `embedded-persistence-recovery` suite alongside
the group campaigns. Use `CARGO_INCREMENTAL=0`:

```console
python3 validation/run.py run --suite embedded-persistence-recovery
cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib
cargo clippy --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --all-targets -- -D warnings
```

On macOS arm64, the registered recovery suite passed all 34 tests and the full
Embassy library passed all 146 tests. All-target Embassy clippy, registry and
website checks passed. The radio simulation, full workspace, Miri and firmware
matrix were not rerun for this test-only extension.

## Pairing rollback with the real persistence owner

The transaction follow-up connects `RemoteControlPairingPersistenceProgress`,
the authorization store exchange, its manifold adapter and the real flash owner.
It begins with a durable existing operator grant and prepares a permission change.
Preparation and a failed initial flash write must leave the complete live table
unchanged. The test verifies the exact failure-settlement command; its engine
acknowledgement is scripted, rather than executing a pairing link lifecycle.

Rollback then uses the real queue and journal. The first rollback write also fails.
The pending request must not complete before durability recovers, and no write may
consume the armed fault before the original retry deadline. A second retry deadline
must likewise be honored. Successful rollback returns the original typed initial
storage error, releases the progress state, and preserves prior live authority.

Journal inspection requires two complete copies of the confirmed snapshot: the
original and the newly persisted rollback. This prevents a false pass from merely
leaving the original record untouched. Two fresh restore owners must recover the
prior table. This is transient flash-failure coverage for the pairing transaction,
not abrupt power removal during rollback, remote grant-management admission,
successful pairing activation, or an end-to-end radio handshake.

Production impact remains none; the test now connects previously separate
transaction and storage assurances without changing their implementation.

The follow-up passed the registered recovery suite (35 tests), all 147 Embassy
library tests, all-target Embassy clippy, registry and website checks on macOS
arm64. No radio-simulation, full-workspace, Miri or firmware run is claimed for
this owner-local test extension.

## Successful storage and settlement-directed rollback

Two more cases drive successful candidate persistence through the same real
transaction, queue and flash owner. The complete live grant table must remain
unchanged during preparation and after storage completion until the transaction
consumes that completion. No settlement command may appear before that point.

For `CompletionDispatched`, the transaction adopts the candidate, becomes ready
and releases authorization ownership. The journal must contain exactly the prior
and candidate snapshots, and two fresh boots must restore the candidate.

For `AuthorizationRollbackRequired`, the transaction restores prior live authority
and queues durable rollback. An injected failure of that rollback write must keep
completion pending through the retry deadline. Once storage succeeds, the
transaction becomes ready and releases ownership. The journal must contain exactly
prior, candidate, prior snapshots; two fresh boots must recover the prior table.

Both acknowledgements are scripted responses to the exact production `Persisted`
settlement command. These cases exercise the runtime activation/rollback machinery
with real storage; they do not prove that the engine emits those acknowledgements
for a real handshake, or cover a crash between candidate commit and rollback.
Production behavior remains unchanged.

This extension passed the registered recovery suite (37 tests), all 149 Embassy
library tests, all-target Embassy clippy, format/docs, registry and website checks
on macOS arm64. The wider radio simulation, full workspace, Miri and firmware
matrix were not rerun for this owner-local test extension.

## Open finding: rollback intent is volatile

Historical characterization: the settlement-directed rollback branch described
below has now been removed from both runtime adapters. A late rollback finalization
is a typed committed-outcome inconsistency; it cannot revoke the durable candidate.
The replacement regression requires no post-settlement flash operations, no queued
rollback and two fresh restores of the candidate. The original cut counts below
remain evidence of the former behavior, not coverage claimed by the new test.
Post-commit activation inconsistency and whole-node recovery remain separate gaps.

The [target-grant rollout](../authorization-commit-design.md#rollout-checkpoint-target-grant-admission-and-delivery)
now prepares before storage in the shared engine and preserves committed grants
on delivery failures in both runtimes. The campaign below deliberately injects
rollback settlement; it characterizes that fallback, not a deadline rejection
still emitted by the prepared engine. Other crash-consistency paths remain open.

The abrupt-rollback campaign drives the real pairing transaction through a
successful candidate store and a scripted `AuthorizationRollbackRequired`
settlement. Live authority has already returned to the prior grant when the
rollback's flash operation is suspended and the owner is discarded.

Across 133 byte-prefix/read-boundary cuts, two fresh restore owners recover the
candidate in 130 cases before rollback commit, and the prior grant in three cases
at or after the complete rollback commit word. The rollback request remains
pending at every cut; the test does not manufacture a successful rollback result.
Complete typed grant tables and exact flash trace prefixes are compared.

This is a characterization of a crash-consistency limitation, not an assertion
that rejected pairing is durably undone. Atomic snapshot storage works as designed:
the candidate remains the newest committed snapshot until rollback commits.
However, the intent to roll back exists only in volatile transaction state, so
boot cannot distinguish that candidate from a successfully finalized grant.

Closing this gap requires a defined durable authorization-transaction protocol,
including its commit point relative to pairing settlement and recovery rules for
pending or aborted transactions. It cannot be solved by changing the parser's
choice of newest valid snapshot. No production change or stronger transaction
guarantee is introduced in this diagnostic slice. Settlement acknowledgement is
still scripted; no physical power-loss or full radio-handshake result is claimed.

The characterization passed in the registered recovery suite (38 tests), with
all 150 Embassy library tests, all-target Embassy clippy, registry and website
checks passing on macOS arm64. Passing here records the current crash window;
it is not evidence that durable transaction rollback is implemented. No wider
radio-simulation, full-workspace, Miri or firmware result is claimed.
