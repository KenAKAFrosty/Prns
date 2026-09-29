# Authorization commit and recovery

Status: proposed implementation contract, not implemented assurance. This follows
the [rollback power-loss finding](measurements/grant-persistence-cuts.md#open-finding-rollback-intent-is-volatile).
The existing campaign remains a characterization until production consumers use
the new contract and the same cuts recover the required authority.

## What the audit establishes

| Owner | Current order | Consequence |
| --- | --- | --- |
| Embassy pairing persistence | Store candidate, activate, settle pairing, possibly store prior | Power loss before rollback commit can restore a rejected candidate. Demonstrated with real flash-owner cuts. |
| Tokio pairing persistence | Store candidate, activate, settle pairing, possibly store prior | The same ordering exists in source. No host crash campaign has demonstrated its cut points yet. |
| Tokio controller-grant management | Store candidate, check caller, activate, send outcome, possibly store prior | Caller cancellation or lost completion can initiate rollback after candidate storage. Source-audited, not crash-tested. |
| Shared target pairing engine | Validate deadline, sign completion, dispatch completion in one settlement | Persistence cannot currently obtain a successful preparation decision separately from sending success. |
| Shared controller pairing engine | Consume persisted result and retire the pairing link | A target-access transaction also needs attempt ownership through durable completion. |

The source owners are:

- `prns-core/src/engine/remote_control/target_pairing_authorization.rs`
- `prns-core/src/engine/commands/remote_control_controller_pairing.rs`
- `prns-core/src/remote_control/pairing/target_pairing/state.rs`
- `prns-runtime/impls/embassy/src/runtime/remote_control_pairing_persistence.rs`
- `prns-runtime/impls/embassy/src/runtime/remote_control_pairing_authorizations.rs`
- `prns-runtime/impls/tokio/src/runtime/remote_control_pairing_persistence/mod.rs`
- `prns-runtime/impls/tokio/src/runtime/remote_control_controller_grants/mod.rs`

Snapshot atomicity is not transaction atomicity. Neither choosing an older valid
snapshot on boot nor persisting a rollback marker only after rejection closes the
window between candidate commit and that marker. Moving the final store after the
existing settlement is also insufficient: settlement can already send success.

## Commit contract

Use a single local durable commit point. Before it, reboot recovers the prior
authorization; after it, reboot recovers the candidate. A response is evidence of
an already committed operation, not the event that commits it.

1. Validate the desired mutation and reserve its transaction owner without
   changing live authority. Preserve the complete prior and candidate values.
2. Prepare the pairing completion in shared core: check the exact attempt,
   permissions and deadline, and perform fallible signing. Do not dispatch a
   success response. A rejection here cannot leave candidate authority durable.
3. Hold the prepared attempt while committing storage. Cancellation and timeout
   must not independently discard that owner while its I/O may still commit.
4. Establish the durable decision, then activate its exact candidate and release
   the prepared completion for dispatch. No success is emitted before durability.
5. A post-commit dispatch failure is an undelivered committed outcome, not an
   implicit revocation. Retrying delivery cannot repeat or reverse the mutation.

The readiness check is the deadline boundary for admission to commit; storage
acknowledgement may arrive later. This is an intentional change from today's
post-store deadline check and needs an explicit shared-core state and boundary
tests, not a relaxed comparison inside an adapter. The prepared phase must not
renew the original window or allow an expired attempt to enter it.

Caller cancellation has the same boundary. Before commit starts it may abort;
once a write can have committed, cancellation alone cannot promise rollback.
Any explicit subsequent revocation is a new transaction. Preserve administrator
authorization rules and unchanged-operation behavior throughout this change.

Durable commitment and successful delivery to another device cannot be made one
atomic operation. A crash after commit but before delivery may leave one peer
uncertain. This design does not claim distributed exactly-once pairing or reboot-
persistent completion retransmission. Existing completion-retention and signed
transcript behavior must be checked before wiring the new prepared phase.

## Storage design gate

Do not add an envelope or another full snapshot buffer merely to name a state.
First implement and test the shared prepare/commit boundary. If all rejectable
decisions can move before the existing atomic snapshot replacement, that snapshot
can remain the durable commit record and existing boot formats can remain valid.

An alternative staged-record design is required if a candidate must be written
before a rejectable decision remains. In that design:

- A pending candidate is not boot authority. Recovery retains the complete prior
  committed table until an explicit, matching commit decision is durable.
- The decision binds the authorization region and exact candidate, not just a
  volatile pairing attempt ID that may be reused after reboot.
- Compaction must retain enough information to recover the committed table while
  a transaction is pending. It must not promote the cached candidate accidentally.
- Legacy snapshots remain committed snapshots; unreleased pending records must
  never be interpreted as legacy authority. Unknown versions fail closed.
- Space admission reserves what is needed to finish or safely abandon a pending
  transaction before accepting it. Daily compaction limits still apply.

Choose between these designs before publishing a new storage format. A pending
record alone does not fix a success response dispatched before durable commit.

## I/O outcomes and ownership

Use typed committed, not-committed and indeterminate outcomes where the backend
can distinguish them. A lost acknowledgement is not proof of a failed write.
Do not release transaction ownership or admit another authorization mutation
while an indeterminate result could later commit. Recovery/read-back must resolve
the exact candidate versus prior durable value; an unresolved outcome must not
be reported as success or successful rollback.

For Tokio, `RemoteControlAuthorizationPersistence::store` runs a blocking task.
Dropping the awaiting future does not establish that the underlying task stopped.
Tests must hold and release that worker explicitly rather than equating future
cancellation with power removal. Also audit background snapshot flush ownership
so it cannot overwrite a decision using a stale live table.

`FileStore::store` currently syncs the staging file and renames it, but does not
sync the parent directory. Process-restart tests and NOR-model cuts must not be
presented as proof of host filesystem power-loss durability. Define and verify
the applicable filesystem durability contract separately, including supported
non-Unix hosts, before using a stronger claim in the transaction API.

For Embassy, keep the bounded exchange, exact completion and compaction wear
budget. Audit the owner's cached snapshots and boot restoration together with
its transaction adapter. A live activation inconsistency after durable commit
must fail closed and recover from the durable decision, not silently rewrite
the prior table as though commitment never happened.

## Implementation and evidence slices

1. **Shared preparation boundary.** Split target pairing preparation from
   completion dispatch using typed attempt-bound states. Cover signing failure,
   attempt mismatch, exact deadline, timeout while reserved, duplicate completion
   and dispatch failure. Include the controller-side target-access lifecycle.
   Do not change shipping adapters until the new contract is usable end to end.
2. **Runtime integration.** Apply the same commit policy to Embassy and Tokio
   pairing and grant management. Retain bounded admission, explicit ownership,
   exact live activation and no success before commit. Resolve indeterminate I/O
   and cancellation without permitting a concurrent mutation to race recovery.
3. **Recovery campaigns.** Change the Embassy characterization's expected tables
   only after the production boundary changes. Cut before preparation, throughout
   storage, before activation, before dispatch and during retry. Check prior or
   candidate by the durable decision, not by whether the caller saw success.
   Add host worker-cancellation/restart cases and both authorization regions.
4. **Whole-node assurance.** Replace scripted settlement with real engine
   preparation/dispatch; exercise pairing deadlines and lost responses through
   controlled nodes. Keep codec/flash tests as narrower independent evidence.
   Run affected registered suites, focused proofs, firmware builds and resource
   contracts before claiming the production change fits supported boards.

Each slice must retain whole-value grant/access assertions, including unrelated
administrators, and compare repeated fresh boots. Fault campaigns must cover
first grants, replacements and revocations, not only a one-entry permission
change. RAM/stack, record capacity, flash writes and compaction pressure require
before/after evidence; this proposal claims no free resource cost.

Production impact of this design slice: none. It records the cross-runtime audit,
proposed observable semantics and implementation gates; it neither repairs the
open crash window nor adds a new runtime dependency or persisted format.

## Implementation checkpoint: shared target preparation

The first implementation slice adds `prepare_authorization` to the shared target
pairing state machine. Its typed outcome reports readiness, deadline refusal,
signing failure, attempt mismatch or absence of authorization work. Readiness
retains the signed completion internally without returning a deliverable response.
The state remains authorizing until persistence settles.

Preparation checks the existing deadline strictly. Once prepared, retries use the
retained signature, and timeout, rejection and link loss cannot consume the
reserved authorization. A correlated storage failure can still abort it. A
successful persistence settlement can release the exact completion after the
original deadline without re-signing. Unprepared callers retain the legacy
post-store deadline/signing behavior; they have not silently adopted this policy.

Six focused tests cover preparation, exact deadline boundaries, signing failure,
attempt correlation, failure settlement and non-authorizing states. The existing
competing-begin whole-state test now includes prepared authorization. A compile-
time size assertion bounds the preparation payload to the completion payload
already present in the phase enum; this adds no heap allocation or storage format.
It is not a firmware stack-usage measurement.

At this first checkpoint neither runtime called the new preparation
method yet. The crash-consistency finding remained open. The next step was to wire engine admission
and both runtime adapters to the boundary, resolve indeterminate storage outcomes,
and remove rollback triggered solely by post-commit delivery failure. Controller-
side target access and grant management remain required parts of that integration.
Completion retention still uses the existing deadline; post-deadline delivery
retry behavior must be addressed explicitly rather than assumed from preparation.

Verification on macOS arm64: all 28 target-pairing tests, the root default-member
tests (`cargo test --locked`), core all-target clippy, registry checks and 49
website tests passed. The no-default-features `thumbv7em-none-eabihf` core check
also passed, including the payload-size assertion. All Cargo commands used
`CARGO_INCREMENTAL=0`. Filtered library tests named `remote_control_pairing`
passed through the separate Embassy (13 tests) and Tokio (3 tests) manifests.
These exercise existing callers, not adoption of preparation by either adapter.
No runtime crash-gap closure, full workspace or firmware
matrix, hardware, Kani or Miri result is claimed by this checkpoint.

## Rollout checkpoint: target grant admission and delivery

The shared engine now prepares completion before emitting
`RemoteControlTargetPairingAuthorizationRequired`, in both approval orders.
This is the production persistence event consumed by Tokio and Embassy: no
adapter-specific preparation policy is needed. Preparation failure aborts the
matching attempt and retires its exchange without requesting a candidate store.
The local approval result and ingress diagnostic report that failure explicitly.

After storage, the engine consumes its retained completion without fetching the
signer again. A real-engine regression checks slow persistence beyond the original
deadline with the signer removed after preparation: completion is dispatched,
not rejected for durable rollback. Another checks that removing the signer before
preparation requests no storage in either approval order.

Both runtimes now preserve committed authority when settlement reports completion
dispatch failure or completion-retention expiry. The classification is shared
core policy; the adapters report an error without undoing the grant. Tokio retains
a typed `CommittedCompletionDelivery` error. Embassy releases transaction ownership
while returning its typed settlement error. Completion delivery has not become a
success merely because authorization committed.

The Embassy fixture drives the real queue, activation and flash owner through both
delivery failures, checks the exact prior/candidate record history without a
rollback record, and restores the complete candidate table on two fresh boots.
Its settlement acknowledgement is still scripted. A Tokio finalization-unit test
checks the same live-authority rule; it is not host filesystem durability evidence.

The existing 133-cut rollback characterization deliberately injects the old
`AuthorizationRollbackRequired` acknowledgement. It remains useful evidence for
the rollback fallback, but is no longer evidence that the prepared production
engine requests rollback merely because storage crossed the admission deadline.

Still open: indeterminate writes and missing settlement acknowledgements, activation
inconsistency after commit, controller-side target-access persistence, grant-
management cancellation, post-deadline completion retry semantics, and whole-node
power-loss coverage. No new persisted format is introduced. Full transaction
crash-consistency closure is not claimed.

On macOS arm64, root default-member tests, all 272 active Tokio library tests
(one ignored), all 151 Embassy library tests, 218 focused core Remote Control
tests, and the registered 39-test embedded persistence recovery suite passed.
Core and both runtimes passed all-target clippy; the no-default-features ARM core
check passed. Cargo runs used `CARGO_INCREMENTAL=0`. Firmware/resource builds,
physical devices, full workspace, Miri and Kani were not run for this slice.

## Rollout checkpoint: grant-management response loss

Tokio and Embassy no longer undo a successfully stored and activated controller
grant change because a local caller cancelled or a remote response failed. This
covers grant creation, replacement and revocation. In particular, losing the
response to a committed revocation must not resurrect the controller.

Cancellation before command processing retains the existing no-mutation behavior.
Once storage succeeds, both adapters finish activation independently of the
completion receiver. The existing response failure handling remains in place;
neither path claims the peer received a result. Initial-store and activation
failures still use their existing recovery paths and remain subject to the open
indeterminate-write audit. No shared-core state or storage format changes here:
the removed rollback decisions belong to runtime delivery mechanisms.

The new Tokio test pauses the real storage worker with its existing mutex before
polling the application future. It proves the old snapshot remains readable while
the write is pending, cancels the local receiver, then releases the worker. Six
cases cover add/replace/revoke with either local cancellation or a missing remote
node. Each compares the complete live table and decodes the file through two new
store instances, preserving an unrelated administrator. The pause accessor is
test-only; it adds no shipping state or storage backend. No sleeps or scheduler
timing guesses determine the cancellation boundary.

The existing Tokio router test now injects an actual response settlement failure
and checks both live and stored authority. Embassy tests drop actual local request
futures during pending storage and fill the command queue to reject a remote
response. They verify the full table, ready state where directly available, and
absence of a newly queued rollback store. Embassy storage completion is scripted
in these router tests; the NOR owner campaign remains separate evidence. Host file
reopening is not filesystem power-loss or full-node boot evidence.

Verification on macOS arm64: all 273 active Tokio library tests (one ignored),
all 152 Embassy library tests, and all-target clippy for both runtimes passed.
The narrow filters were `remote_control_controller_grants` on Tokio (7 tests)
and `request_runner` on Embassy (11 tests). Cargo commands used
`CARGO_INCREMENTAL=0`. The full firmware/resource matrix, Miri, Kani and physical
hardware were not run for this adapter-only change.

Remaining production work: indeterminate storage and missing pairing settlement
acknowledgements, activation inconsistency after commit, controller-side target-
access transactions, and completion-retention/retry semantics. Grant-management
response-loss rollback is now fixed; full crash-consistency closure is not claimed.

## Rollout checkpoint: controller-side target-access settlement

Both adapters now retain successfully stored and activated target access when
the subsequent engine settlement fails. Tokio reports distinct typed errors for
a missing acknowledgement, rejected settlement, or inconsistent finalization.
Embassy releases transaction ownership and returns its existing typed settlement
error without scheduling a rollback. Neither adapter reports successful pairing
when settlement failed. This does not introduce a completion retry mechanism.

Shared controller pairing already preserves its prepared access in `Persisting`
through link closure and expiry; these rollback decisions belonged to the adapters.
Initial-store and activation failure recovery are unchanged and remain subject
to the indeterminate-write audit.

The Tokio test exercises the real file worker for add and replacement, preserving
an unrelated target. It checks committed contents before scripted acknowledgement,
the entire live table afterward, and two fresh file readers. Cases cover success,
lost acknowledgement, attempt mismatch, no persistence owed, and unexpected failure
finalization. These are not filesystem power-loss tests.

The Embassy test exercises the actual flash-journal owner for the same add/replace
cases. It checks success, attempt mismatch, no persistence owed, unexpected failure
finalization, and an occupied pairing-settlement slot. It verifies released
transaction ownership, no queued rollback, exactly the prior and candidate records,
and the complete candidate table on two fresh restores. Engine acknowledgements
are scripted; these restores do not claim whole-node boot or hardware coverage.

Remaining work includes indeterminate storage, target-side missing settlement
acknowledgements, activation inconsistency after commit, completion-retention/retry
semantics, and whole-node crash coverage. Full crash-consistency closure is not
claimed.

Verification on macOS arm64, with `CARGO_INCREMENTAL=0` for Cargo runs:

- `cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib`:
  274 passed, one ignored; the focused `remote_control_pairing_persistence` filter
  passed all four tests.
- `cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib`:
  153 passed.
- `cargo clippy --locked --manifest-path <runtime>/Cargo.toml --all-targets -- -D warnings`:
  passed for both runtime paths above.
- `python3 validation/run.py run --suite embedded-persistence-recovery`:
  all 40 tests passed.
- `./tools/prns verify`, `python3 validation/run.py verify`, and
  `cargo test --locked --manifest-path docs/website/Cargo.toml`: passed.

No shared-core code changed. Root/workspace tests, firmware/resource builds,
physical hardware, Miri and Kani were not run for this adapter-only slice.

## Rollout checkpoint: unavailable target-grant settlement

Tokio now retains a successfully stored and activated controller grant when the
settlement channel closes without an acknowledgement, returning
`CommittedTargetGrantSettlementUnavailable`. Embassy likewise retains the grant
when its pairing-settlement slot is busy or its settlement helper reports
`NodeStopped`, releasing transaction ownership and returning the typed failure.
An unavailable acknowledgement does not establish that committed authority was
rejected. Explicit engine-directed rollback and explicit core rejection retain
their existing recovery behavior; those are distinct from this transport failure.

The regression tests failed before the production changes. Tokio removed the
committed live grant; Embassy entered rollback rather than returning the busy
settlement failure. The Tokio fixture exercises the actual file writer for both
add and replacement, with either a closed command receiver or a dropped settlement
sender. It checks the candidate on disk before dropping an issued acknowledgement,
the complete live grant table afterward, and two fresh file reads.

Embassy extends the existing flash-owner fixture with an actually occupied
settlement slot and a mismatched typed acknowledgement, which the node handle
maps to `NodeStopped`. The latter is a scripted adapter failure, not a simulated
physical node shutdown. Both cases check the precise error, released ownership,
no queued rollback, exactly the prior and candidate journal records, and the
complete candidate grant on two fresh restores. Existing successful settlement,
delivery failure, expiry, explicit rollback and interrupted-rollback cases remain
in the same fixture.

No shared-core state or persisted format changes. Indeterminate writes, activation
inconsistency, explicit target settlement rejection, completion retries and
whole-node crash coverage remain open; this is not full crash-consistency closure.

Verification on macOS arm64 with `CARGO_INCREMENTAL=0`: the focused Tokio
`remote_control_pairing_persistence` filter passed five tests; the focused Embassy
`committed_authority_survives_unavailable_settlement` regression passed. Full
`cargo test --locked --manifest-path <runtime>/Cargo.toml --lib` runs passed
275 Tokio tests (one ignored) and 154 Embassy tests. Both runtime paths passed
`cargo clippy --locked --manifest-path <runtime>/Cargo.toml --all-targets -- -D warnings`.
`python3 validation/run.py run --suite embedded-persistence-recovery` passed all
41 tests. Formatting, both registries, and website tests also passed. Root/workspace,
firmware/resource, hardware, Miri and Kani checks were not run for this adapter slice.

## Storage checkpoint: reconcile uncertain flash commit writes

The shared flash journal previously returned immediately when its commit-word
write reported an error, even when the requested bytes had actually landed.
`write_record` now performs its header read-back on that path too. If the full
header, checksum, epoch, kind, payload length and commit marker match the intended
record, the operation succeeds and advances its cursor. Payload verification
still happens before the commit write. A torn commit or failed read-back retains
the original write error; uncertainty is never promoted to success without evidence.

This fixes an actual write-owner boundary, not just runtime error classification.
Embassy's existing critical-snapshot owner receives `Stored` for a reconciled commit
and updates its snapshot cache; all consumers of this shared flash writer benefit.
Tokio uses `FileStore`, not this journal, so its backend is unchanged. The journal
module's existing tests were extracted without changing their behavior, and the
new fault fixture has its own test module.

The new test failed before the fix and passes afterward. It crosses complete/torn
commit writes with available/failed read-back, including the case where an error
is correctly retained but a fresh open finds the committed candidate. A reconciled
success permits a subsequent append and restores all records in order, proving
cursor advancement. Driver faults are simulated; this is not physical flash or
whole-node power-loss evidence.

Still open: failed verification after a possibly committed write, safe retry at
an uncertain journal tail, activation/rollback recovery, and full transaction
crash consistency. The host audit also found that `FileStore` syncs staged contents
then renames without synchronizing the containing directory; portable filesystem
durability and error classification require separate work. Reopening a file is not
proof of survival across host power loss.

Verification on macOS arm64 with `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-core --features flash flash_journal`: 19 passed,
  including the new four-case uncertain-commit test, which failed before the fix.
- `cargo test --locked`: root default-member tests passed.
- Full runtime library tests: 275 Tokio passed (one ignored), 157 Embassy passed.
- `cargo clippy --locked -p prns-core --features flash --all-targets -- -D warnings`
  and Embassy all-target clippy passed.
- `cargo check --locked -p prns-core --no-default-features --features flash --target thumbv7em-none-eabihf` passed.
- `python3 validation/run.py run --suite embedded-persistence-recovery`: 44 passed.
- Formatting, both registries and website tests passed.

No full workspace, firmware/resource matrix, Miri, Kani or physical-board run is
claimed. The no-std target check is not a firmware RAM or stack-budget result.

## Rollout checkpoint: stale target-attempt settlement

Both adapters now retain committed controller grants when target settlement
returns `NoAuthorizationOwed` or `AttemptMismatch`. Those outcomes describe the
engine's current attempt ownership, not a revocation request. Tokio preserves
the exact error in `CommittedTargetGrantSettlement`; Embassy releases transaction
ownership and returns its existing typed target-settlement error. Neither reports
successful pairing. Shared core already preserves an unrelated active attempt
on mismatch, so no core production change is needed.

Both regressions failed before the adapter changes: Tokio removed the live grant,
and Embassy entered rollback. The Tokio file-worker fixture now covers eight
add/replace cases across unavailable and stale settlement, checking the complete
table in memory and through fresh file readers. The Embassy flash-owner fixture
injects both stale-attempt errors and checks the returned error, ready/released
state, no rollback request, prior/candidate record order and complete candidate
table on two fresh restores. Settlement errors are scripted; this is not evidence
of a whole-node race or physical power loss.

Explicit `AuthorizationRollbackRequired`, signing failures, and inconsistent
finalizations retain their existing behavior and need separate audit. Indeterminate
writes, activation failure recovery, completion retries and whole-node crash
coverage also remain open. No persisted format changes or complete crash-consistency
claim accompanies this slice.

Verification on macOS arm64 with `CARGO_INCREMENTAL=0`:

- `cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib`:
  275 passed, one ignored; the focused pairing-persistence filter passed five tests.
- `cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib`:
  155 passed, including the focused stale-attempt regression.
- Both runtimes passed `cargo clippy --locked --manifest-path <runtime>/Cargo.toml --all-targets -- -D warnings`.
- `cargo test --locked -p prns-core authorization_settlement_preserves_mismatch_and_absence`:
  one existing shared-engine test passed.
- `python3 validation/run.py run --suite embedded-persistence-recovery`:
  42 tests passed.
- Formatting, `./tools/prns verify`, `python3 validation/run.py verify`, and
  `cargo test --locked --manifest-path docs/website/Cargo.toml` passed.

Root/workspace suites, firmware/resource builds, hardware, Miri and Kani were not
run for this adapter-only change.

## Rollout checkpoint: completion-signing failure after commit

Both adapters retain a successfully stored and activated controller grant when
settlement reports `TargetSignerUnavailable` or `CompletionSigningFailed`.
These failures concern the completion receipt, not the stored controller's
authorization. Normal engine admission already prepares that receipt before
storage; the errors remain available through the unprepared compatibility path.
No pre-storage validation or signing checks were weakened.

Tokio returns the exact failure in `CommittedTargetGrantSettlement`. Embassy
releases transaction ownership and returns its typed target-settlement error.
All target settlement errors now preserve the committed grant, simplifying the
previous per-error branches. Explicit rollback and inconsistent successful
finalizations retain their existing handling and remain distinct audit items.

The new regressions failed against the previous adapters and pass after the fix.
The Tokio file-worker fixture covers twelve add/replace cases across unavailable,
stale and signing failures, checking exact errors and full live/durable tables.
The Embassy flash-owner fixture injects both signing errors, verifies released
ownership and no rollback request, checks the exact prior/candidate record order,
and restores the full candidate table twice. Settlement errors are scripted;
this does not establish a physical signer failure or whole-node power-loss result.
The shared authorization-preparation tests still prove signing refusal before
commit. No shared-core production code or storage format changed.

Indeterminate writes, activation failure recovery, inconsistent finalizations,
explicit rollback durability and completion retries remain open. Full transaction
crash consistency is not claimed.

Verification on macOS arm64 with `CARGO_INCREMENTAL=0`: focused pairing-persistence
tests passed on Tokio (five), and the new signing-failure regression passed on
Embassy. Full `cargo test --locked --manifest-path <runtime>/Cargo.toml --lib`
runs passed 275 Tokio tests (one ignored) and 156 Embassy tests. Both runtimes
passed `cargo clippy --locked --manifest-path <runtime>/Cargo.toml --all-targets -- -D warnings`.
`cargo test --locked -p prns-core authorization_preparation` passed six tests.
`python3 validation/run.py run --suite embedded-persistence-recovery` passed 43.
Formatting, both registries and website tests passed. Root/workspace suites,
firmware/resource builds, hardware, Miri and Kani were not run for this adapter slice.

## Rollout checkpoint: inconsistent target finalization

Both adapters now preserve a stored and activated grant when a `Persisted`
settlement unexpectedly returns `AuthorizationFailureRecorded`. That response
belongs to the failed-storage path; it is not the explicit
`AuthorizationRollbackRequired` outcome. Tokio returns
`CommittedTargetGrantFinalizationMismatch`; Embassy releases authorization
ownership and returns `UnexpectedTargetFinalization`. Neither reports success.
The legitimate failed-storage path and explicit rollback handling are unchanged.

Both regression fixtures failed before the change. Tokio removed the committed
grant; Embassy entered rollback instead of immediately returning the inconsistency.
The Tokio file-worker fixture now covers fourteen add/replace cases, checking
whole live tables and fresh file readers. Embassy verifies the exact error,
ready/released state, absence of a rollback request, prior/candidate record order
and the complete candidate table on two fresh restores. The inconsistent response
is deliberately scripted: the current engine does not normally emit it for a
`Persisted` command. This is defensive adapter coverage, not a newly reproduced
whole-node race. No shared-core production code or persisted format changed.

The post-commit settlement audit now leaves only explicit engine-directed rollback
on that path. Indeterminate storage, activation failure recovery, explicit rollback
durability, completion retries and whole-node crash coverage still require work;
full crash-consistency closure is not claimed.

Verification for inconsistent-finalization handling on macOS arm64, using
`CARGO_INCREMENTAL=0`: focused Tokio pairing-persistence tests (five) and the
Embassy inconsistent-finalization regression passed. Full
`cargo test --locked --manifest-path <runtime>/Cargo.toml --lib` passed 275 Tokio
tests (one ignored) and 157 Embassy tests. Both runtimes passed
`cargo clippy --locked --manifest-path <runtime>/Cargo.toml --all-targets -- -D warnings`.
`python3 validation/run.py run --suite embedded-persistence-recovery` passed 44
tests. Formatting, both registries and website tests passed. Root/workspace,
firmware/resource, hardware, Miri and Kani checks were not run for this adapter slice.

## Storage checkpoint: uncertain tails and host namespace confirmation

Shared flash cursors now become non-appendable before programming starts, and
regain an append position only after verified success. An error or cancellation
therefore cannot cause another append to reprogram potentially occupied NOR
cells. This applies to ordinary appends, compaction records and the compaction
commit. Starting a compaction commit also closes the old active tail: if a cancelled
commit already selected the new epoch, subsequent writes to the old epoch would
otherwise be silently ignored on reboot. No cursor fields or happy-path flash
operations were added.

The uncertain-commit retry regression failed before the change. Tests now cover
read-back failures, torn commits, and cancellation during active append, compacted
append and compaction commit. Rejected retries leave the complete flash image
unchanged. Fresh opens check whole record sequences. Embassy's recovery fixtures
now exercise compaction instead of assuming an uncertain tail can be overwritten;
they continue to enforce backoff, compaction budget, exact durable records and
fresh restores. Conservative recovery can take longer, including the configured
compaction cooldown; an I/O error is not proof that no cells changed.

The shared host `FileStore` now confirms Unix directory updates after rename and
removal. Initial directory readiness also syncs the canonical ancestor chain,
including retries after partial directory creation. `DurabilityUnconfirmed`
distinguishes confirmation failure from an ordinary I/O error. The injected
post-rename test checks that the new snapshot is visible even though confirmation
fails, retains the exact error category, and successfully retries the same value.
This is host-file boundary evidence, not power-loss testing. Non-Unix platforms
retain their previous file-sync-and-rename behavior; no Windows directory-durability
guarantee is claimed. Tokio inherits the shared host implementation; Embassy
inherits the shared flash changes.

Verification on macOS arm64 used `CARGO_INCREMENTAL=0`: 20 focused flash-journal
tests, 10 focused file-store tests, root default-member tests, 275 Tokio library
tests (one ignored), 157 Embassy library tests, and 44 registered embedded
persistence recovery tests passed. Core (with `flash`) and both runtime all-target
clippy checks passed, as did the no-default-features `flash` core check for
`thumbv7em-none-eabihf`, formatting, registries and website tests.

`./tools/prns doctor embedded-assurance` failed readiness: 5.6 GiB was free versus
24 GiB required, and Renode was unavailable for the nRF platform lane. No full
firmware/resource, Miri, Kani, ISA or physical-board result is claimed here.

This completes the bounded settlement-error and unsafe-tail fixes, not the entire
authorization crash-consistency contract. Durable transaction intent across power
loss, uncertain-store reconciliation through runtime activation, explicit rollback
durability, completion retries, and non-Unix host durability still need work.

## Recovery checkpoint: prepared engine grant through flash commit

The `authorization-commit-recovery` suite connects the real shared pairing engine's
prepared grant to the production snapshot codec and flash journal. It exercises
first grants and replacements while preserving an unrelated administrator. Each
candidate append is interrupted at every programmed-byte boundary, including the
last commit byte before acknowledgement. Every resulting image is opened twice
through fresh journals and compared against the entire expected grant table.

The final-byte cut deliberately returns a storage error while both restores select
the candidate. Earlier cuts select the prior table. This establishes a concrete
recovery oracle for the upcoming indeterminate-store adapter work: an error alone
cannot choose rollback. The model only cuts append programming; erase, compaction,
runtime ownership and physical flash behavior remain separate evidence.

Successful storage is also followed by real engine settlement at the admission
deadline, with the signing identity removed after preparation. One case discards
the dispatched response; another removes egress and checks the exact dispatch
failure. Fresh journal restores retain the committed table in both cases. The
existing shared preparation fixture now rejects any outbound directive before
storage is requested.

These are shared-engine/storage boundary tests, not whole-node reboot tests or
runtime-adapter activation tests. They do not resolve indeterminate I/O for Tokio
or Embassy, nor prove host filesystem power-loss durability. No production policy,
snapshot format, allocation or firmware resource usage changes in this slice.
Next: make each runtime reconcile that durable decision before releasing ownership
or activating authority, then use the same whole-table expectations through actual
node reboot and controller-side target-access recovery.

Verification on macOS arm64: `CARGO_INCREMENTAL=0 python3 validation/run.py run
--suite authorization-commit-recovery` passed both campaigns; `CARGO_INCREMENTAL=0
cargo test --locked -p prns-core --features flash --lib --quiet` passed 2,121 tests
with three ignored. Root default-member tests and core all-target clippy with
`flash` passed, also with incremental compilation disabled. Formatting, diff
whitespace checks and validation registry verification passed. Runtime-specific,
firmware, Miri, ISA and physical-device suites were not run for this test-only
slice; unrelated CI failures remain deferred.

## Recovery checkpoint: retain uncertain embedded commits

The flash journal now distinguishes failed commit readback from an ordinary
pre-commit failure. Its read-only `confirm_append` operation compares the exact
record kind, epoch, length, checksum and payload, distinguishing a committed
candidate from an absent commit word without rewriting the record. Read errors
and mismatched committed records remain unresolved; they cannot authorize success.

The Embassy critical-snapshot owner retains the uncertain record address and kind.
It keeps the existing bounded transaction and candidate snapshot, retries on the
existing persistence schedule, and blocks other journal work until confirmation
resolves. A queued discovery-group request cannot bypass the authorization owner
or continuously wake its retry wait. Confirmed commits update the durable cache
before activation; confirmed non-commits enter the existing failure path. No
rollback or completion is emitted solely because commit readback failed.

This covers both authorization regions and the shared discovery-group snapshot
path. The new owner regressions lose the commit acknowledgement, check pending
completion and unchanged live authority, recover reads, and require read-only
confirmation followed by exact activation and two fresh restores. Journal tests
also cover torn commits and refusal of a different candidate or record kind.
The group fault campaign now expects unresolved completion, rather than definite
failure, when power disappears during commit confirmation.

This is the Embassy backend recovery slice. Tokio's rename/directory-sync
reconciliation is still pending and must retain worker ownership without an
unbounded blocking-thread retry or a stale background flush. Cancellation during
outstanding flash I/O, post-commit activation inconsistency, explicit rollback
durability and complete node-level recovery remain separate open boundaries.
The new embedded owner state has a real resource cost; firmware resource evidence
must measure it rather than assume it fits.

## Recovery checkpoint: exact host-file confirmation

The shared std file backend now distinguishes `PublishedDurabilityUnconfirmed`
from failures before replacement. The former means rename succeeded but directory
confirmation failed; the visible candidate must not be interpreted as a rejected
write. Directory creation and removal retain their existing error vocabulary.

`FileStore::confirm_store` compares the complete candidate, including length, in
bounded chunks without staging or replacing it. Exact matches are file-synced and,
on Unix, directory-synced before returning `Confirmed`. Missing and different
values are distinct outcomes, and I/O errors remain unresolved. Other platforms
retain the existing file-sync-and-rename durability limit. The caller must exclude
concurrent writers through confirmation and activation; this primitive does not
itself reserve transaction ownership.

The backend tests cover lost post-rename confirmation, repeated confirmation
failure and recovery, both authorization regions, empty and multi-chunk values,
every byte mismatch, shorter and longer values, missing files, and pre-publication
failures retaining the old value. Staging sentinels, contents, modification times,
and Unix file identities check that confirmation does not rewrite the snapshot.
These are file-backend tests, not whole-node simulator or power-loss evidence.

Tokio worker ownership and stale-background-flush exclusion remain the next slice.
No adapter consumes the new confirmation primitive yet, so its authorization
rollback behavior is not fixed by this prerequisite alone. Embedded behavior and
its previously measured resource footprint are unchanged.

Verification on macOS arm64 passed:

```console
CARGO_INCREMENTAL=0 cargo test --locked -p prns-core persistence::impls::file --quiet
CARGO_INCREMENTAL=0 cargo test --locked --quiet
CARGO_INCREMENTAL=0 cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --quiet
CARGO_INCREMENTAL=0 cargo clippy --locked -p prns-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The focused backend suite passed 14 tests; the root run included 2,126 passing
core tests with three ignored, and Tokio passed 275 with one ignored. Windows,
Linux, firmware, Miri, ISA and hardware checks were not run for this std-only
backend slice. The changed file backend is outside the configured mutation surface.

## Recovery checkpoint: Tokio transaction ownership and confirmation

Tokio now reserves the shared file-store owner before an authorization command
prepares its mutation and releases it only after the command settles successfully.
Pairing in both directions and local/remote controller-grant changes share this
path. The owner token survives cancellation of its awaiting future, including
cancellation while a blocking file operation is still running. An abandoned or
fatally failed owner remains fenced until node restart; this is fail-closed
recovery, not automatic in-process resumption of abandoned work.

Post-publication durability errors now enter exact candidate confirmation, with
250 ms asynchronous waits between attempts. Confirmation does not rewrite or
roll back the candidate. Missing/different values and read/sync failures remain
unresolved. Only exact confirmed durability returns write success; definite
pre-publication errors retain the existing failure/rollback path. Loss of the
blocking worker is a fatal ownership failure, not permission to overwrite a
potentially committed candidate. No blocking thread is retained during retry waits.

Background flushes capture an authorization revision before preparing snapshots
and recheck it under the storage lock before writing any region. Owned or stale
flushes are deferred without triggering the background exit-on-failure policy;
an explicit startup/shutdown flush cannot report success for deferred work.
Read-only authorization snapshot requests do not acquire ownership or invalidate
their own prepared flush. Explicit transaction completion permits fresh flushes,
but never rehabilitates an older revision.

The new tests exercise the actual Tokio owner and file store with injected lost
acknowledgement/readback outcomes, both authorization regions, paused Tokio time,
stale prepared flushes, cancellation across a blocking-write barrier, worker
failure, and read-only snapshot behavior. Existing adapter regressions continue
checking activation and cancelled response delivery. This is runtime/backend
evidence, not a new whole-node power-cut simulator campaign. Explicit rollback
crash durability, post-commit activation inconsistency, and complete node-level
reboot recovery remain open; Embassy's outstanding-flash-I/O cancellation boundary
also remains distinct from its already implemented lost-readback confirmation.

Verification on macOS arm64 passed with incremental compilation disabled:

```console
CARGO_INCREMENTAL=0 cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --quiet
CARGO_INCREMENTAL=0 cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked --manifest-path validation/integration/Cargo.toml --test runtime_persistence --test runtime_remote_control_pairing --quiet
CARGO_INCREMENTAL=0 cargo test --locked --quiet
CARGO_INCREMENTAL=0 python3 validation/run.py run --suite authorization-commit-recovery --suite virtual-device-simulation
cargo fmt --manifest-path prns-runtime/impls/tokio/Cargo.toml -- --check
git diff --check
```

Tokio passed 280 tests with one ignored; persistence/pairing integration passed
six, and the registered recovery and simulation suites passed. The integration
tests initially could not bind loopback sockets in the sandbox and passed after
rerunning with that permission. No firmware, hardware, Miri, ISA, Linux or Windows
run is implied. This slice changes only the Tokio adapter, outside the configured
mutation surface; the shared file confirmation primitive remains the prior slice.

## Recovery checkpoint: refuse settlement-directed revocation after commit

Both runtime adapters now reject `AuthorizationRollbackRequired` after successful
storage as a typed committed-finalization inconsistency. Shared core prepares the
real target pairing attempt before emitting its persistence requirement, so the
old deadline-driven rollback result is not a valid outcome of that prepared path.
The adapters retain the complete committed candidate in live and durable state;
they neither report success nor schedule a compensating write. A deliberate later
revocation must be admitted and committed as its own authorization transaction.

Tokio's post-commit finalizer is now a synchronous outcome classifier with no
access to the mutable authorization table or persistence owner. Embassy's settled
authorization outcome no longer has a rollback variant. Existing pre-commit
failure/rollback handling is retained. Post-commit activation inconsistency is
still a separate unresolved path and is not covered by this change.

The former 133-cut Embassy rollback campaign characterized a now-removed write
path. Its historical result remains in the measurements document. Its replacement
checks zero post-settlement flash operations, no queued rollback, exact candidate
authority and repeated fresh restoration. Tokio covers the same late result for
both first grants and replacements through its real file owner and adapter, with
two fresh reads. Acknowledgements in these adapter regressions are injected;
the separate shared-engine recovery suite exercises actual preparation and late
completion. Neither is presented as a whole-node crash campaign.

## Recovery checkpoint: fail closed on committed activation inconsistency

After successful durable storage, neither adapter may compensate for an
activation inconsistency by writing the prior snapshot. The shared runtime
service now exposes `require_authorization_recovery`: it makes authorization,
request configuration and inventories unavailable, and snapshot writers return
no snapshot rather than serializing an empty table. Recovery requires fresh
service construction and restoration of the committed durable tables. This uses
the existing service availability representation without a new resident buffer.

Tokio propagates a typed committed-activation failure through the router and
retains the persistence owner's reservation. Embassy disables the same shared
service and retains an unrecoverable continuation instead of sending a pairing
persistence-failure settlement or submitting a compensating write. Local and
remote controller-grant management use the same rule. A stray store completion
cannot release Embassy's unrecoverable state. Pre-commit storage failure and its
rollback handling remain unchanged.

Focused tests inject activation mismatches for grant addition, replacement and
revocation and for target-access addition and replacement. Tokio's tests exercise
the production activation helpers with the real file owner, repeated fresh reads
and retained ownership. Embassy's tests exercise its continuations and bounded
store exchange; the target-grant pairing case additionally uses real journal
storage, verifies zero compensating flash operations and restores the candidate
on two fresh boots. Target-access Embassy completion is scripted. Shared runtime
tests verify service quarantine and refusal to emit empty snapshots. This is
adapter/backend recovery evidence, not a complete node-level crash campaign.

Remaining recovery work includes Embassy cancellation with flash I/O outstanding,
pre-commit rollback crash durability and end-to-end controlled-node reboot
campaigns. These are distinct from the now-removed post-commit rollback paths.

## Recovery checkpoint: retain confirmation across cancelled append I/O

The Embassy critical-snapshot owner now captures the shared journal's append
offset into its existing pending-confirmation state before awaiting flash.
Previously, dropping that future after the commit word reached flash left no
confirmation state: a retry encountered the journal's deliberately poisoned tail
and entered compaction instead of confirming the durable candidate. The focused
committed-cancellation regression failed on that implementation.

Resumption now uses read-only exact-record confirmation before any new write.
A committed record updates the retained snapshot and acknowledges its owner;
an uncommitted record returns a typed failure with a retry deadline. Retaining
that deadline also matters for required rollback: cancellation must not turn a
retryable rollback into a permanently parked continuation. Cancellation during
confirmation itself leaves the same ownership intact.

The new campaign runs the real Embassy manifold persistence wrapper and flash
journal across 133 append cancellation boundaries: every write prefix and the
start/end of every read. Each case cancels a second time during confirmation,
checks read-only resolution, verifies the complete retained snapshot and restores
the expected authority on two fresh boots. Separate focused cases cover the
fully written commit and a cancelled rollback progressing through its retry.
This uses the deterministic pending-I/O fixture with storage made available
again; it does not claim that physical flash drivers can survive arbitrary
cancellation or power restoration without their documented recovery.

Shared core already owns poisoned-tail protection and exact confirmation. This
slice exposes its cursor for the adapter's existing confirmation state; it adds
no journal field or snapshot buffer. Tokio uses its separately owned file-store
transaction and does not use this flash append path. Cancellation while starting
or advancing compaction, and full controlled-node reboot campaigns, remain
separate work rather than being implied by this append/resume campaign.

## Recovery checkpoint: confirm the compaction arena commit after cancellation

Shared core now confirms an arena commit against its exact offset, epoch, kind,
length and checksum before selecting that arena and restoring the append cursor.
Ordinary append confirmation uses the same record-verification helper. Embassy
records a `ConfirmCommit` phase before awaiting the final compaction write and
retains it through cancelled or failed readback; uncertain completion cannot
abort compaction and silently fall back to the previous arena.

A confirmed commit completes compaction without another erase, allowing the next
authorization snapshot to append in the selected arena. A confirmed incomplete
commit uses the existing failure/retry path, preserving the recorded compaction
wear budget. The initial regression failed on the former implementation because
resumption abandoned compaction without reading back the completed marker.

The new native campaign covers 38 final-commit cancellation boundaries, including
all write prefixes and read start/end points. Every case also cancels confirmation,
injects a readback error, checks the retry deadline and read-only resolution,
then stores a new grant and verifies it through two fresh restores. Incomplete
commits additionally prove refusal before the wear-limit deadline and successful
retry afterward. Shared-core tests reject an incorrect marker kind and invalid
offset, hold writes during uncertainty and check replay after arena selection.

This extends real adapter/journal evidence, not a whole-node crash campaign or
physical flash-driver cancellation guarantee. Cancellation earlier in compaction
(budget recording, sector erase, copied-record appends) remains a separate next
step. Tokio's file-store owner does not use this flash-arena path.

Ordinary append and compaction share their route and ratchet encoders, and the
two authorization snapshot kinds share one compaction write phase. Route encoding
relies on the core writer's bounds checks instead of cloning and measuring the
same row first. Equivalence tests compare complete buffers with the former codec
sequence and reject every shorter output length; ratchet scratch remains zeroizing.

## Recovery checkpoint: cancellation while recording the compaction budget

The existing shared journal rescans both timebase pages on every budget attempt.
A 707-boundary Embassy campaign now exercises cancellation at read start/end,
every write prefix, and every erase prefix across fresh-page and full-page-rollover
cases. Resumption must record the budget before entering the arena-erase phase;
a completed write is recognized by read-only rescan instead of being written again.
The campaign checks the untouched journal arenas, retained configuration, and
the exact cooldown after two fresh restores, including refusal just before the
deadline and admission at it. This stage needed no production behavior change.

These are cancelled-operation/resumption tests with reboot checks after recovery,
not proof of physical driver cancellation or reboot at every interrupted instruction.
Cancellation during arena erase and copied-record appends remains subsequent work.

## Recovery checkpoint: arena erase and copied snapshots

The next three campaigns cover 1,026 arena-erase cancellation boundaries, 133
controller-grant-copy boundaries, and 65 discovery-group-copy boundaries. Each
case resumes the same owner, refuses additional programming before the recorded
wear deadline, then persists a new grant and verifies both it and the retained
group configuration through two fresh restores. Both inactive sectors begin with
programmed bytes, so partial erases exercise real prefix changes in the fixture.

The erase campaign exposed a production issue: cancellation left the current
sector eligible for another erase within the same recorded wear budget. Embassy
now records an in-flight erase phase before awaiting the driver. Resumption of
that phase abandons compaction and uses the existing cooldown-controlled retry
path. Successful erase completion still advances normally. The wear-budget policy
belongs to this Embassy owner; Tokio's file-store path does not erase journal arenas.

Copied snapshots already recover safely: the shared journal poisons the append
cursor before programming, and the next owner turn aborts rather than reprogramming
that tail. Those two campaigns required no production change. Route, ratchet, and
target-access copy cancellation, full-node interrupted-reboot campaigns, and physical
flash-driver semantics are not claimed by these tests.
