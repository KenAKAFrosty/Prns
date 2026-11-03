# LXMF libraries

Use these libraries to exchange supported LXMF messages with existing LXMF peers.
LXMF and PRNS groups are separate application protocols; consumers can select either
or both. Wire handling and caller-owned direct delivery are independent of the
Prns app and React Native. The root Cargo workspace members are `prns-lxmf-wire`
and `prns-lxmf`.
The [host runtime](../prns-host/README.md) provides the optional concrete host adapter.

- `wire/` is an allocation-free `no_std` parser, composer and validation library.
  Wire parsing borrows its input and applies caller-supplied limits.
- `service/` exposes portable values by default. `tokio-runtime` selects the
  injected direct engine; `tokio-host` adds the canonical Prns host adapter.
  `redb-mailbox` selects synchronous transactions on a caller-owned database.
  Combining `tokio-runtime,redb-mailbox` adds durable delivery without selecting
  a concrete host. No feature selects the app or group-message libraries.

Direct delivery supports the existing bounded Link DATA subset. A delivery proof
is a Reticulum packet proof, not proof of a remote mailbox commit. Resource
transfer and propagation delivery are not implemented here.

The host adapter borrows the node; stopping LXMF never stops that host or its
unrelated Links. Each outbound Link belongs to one attempt and closes after
settlement or rejected admission. Accepted host operations retain that cleanup
if a waiter is cancelled, including a late successful Link establishment.
Use the scoped adapter when an embedding must fence an identity generation;
the unscoped adapter requires an unrestricted native host client.

Transient shutdown cancels its disposable callback work and joins owned tasks.
Interrupted or timed-out shutdown retains the join boundary for a later retry;
a task failure remains a failure on retry. Durable shutdown instead drains
accepted callbacks through the caller-owned database before reporting success.

## Resource selection and accepted work

`DirectLimits` explicitly selects transient history count, retained record-buffer
bytes and active attempts (defaults: 1,024 records, 4 MiB, eight attempts). At the
history limit a new send returns `HistoryFull`; inbound admission increments the
overflow diagnostic without inserting its message ID into deduplication. Existing
history is retained. A previously emitted transport proof does not imply that the
application admitted an inbound message. Record slots and actual buffer capacities
are accounted; bounded callback/peer lanes, dedup index overhead, allocator costs
and caller-owned snapshot copies are separate memory obligations.

`MailboxLimits` is selected only at database creation and persisted exactly.
Defaults are 100,000 records, 256 MiB of logical accounted data, and replay pages
of at most 256 rows / 1 MiB of returned owned records. Admission reserves each
record's largest terminal encoding and required indexes, so cancellation and
settlement do not require new ordinary quota. The accounting does not measure
redb file pages or process RSS. Page memory includes record slots and all decoded
buffer capacities; bounded encoding/decoding scratch is separate. Cold validation
streams rows and cross-checks the exact queued-generation and inbound-ID indexes.

The mailbox format is version 2. Its `LXMF_META` table
(`prns_lxmf_metadata_v2`) owns only LXMF policy and counters; unrelated caller
metadata tables can have any name or type. First creation rejects any existing
owned mailbox table, including empty partial schemas. Unsupported formats, missing markers and
corrupt index relationships are rejected; opening never resets or silently repairs
them. There is no automatic format migration.

`ReplayLimits` bounds active durable attempts and each replay query (defaults:
eight attempts and 32 rows). The owner reads ascending exclusive-ID pages of the
queued index, retains only active attempts, and periodically revisits the index
so a lost notification or retry of an older ID cannot strand accepted work. A
paused start does not load the backlog. Uncertain settlement keeps its slot until
reconciled; Stop drains owned tasks while queued records remain durable for reopen.
No user history is automatically evicted. Caller-owned mailbox submissions must
retain admitted transactions independently of a dropped result waiter.

Ordinary mailbox/conversation listings cap their returned allocation under the
same persisted page policy. Conversation grouping still scans retained records;
it is not a separately indexed conversation catalog or a constant-work query.

## Integrating durable delivery

Enable `tokio-runtime,redb-mailbox`, adding `tokio-host` when using the concrete
Prns host adapter. The embedding retains its database, blocking worker, host and
service for the complete lifetime of accepted work:

1. On the existing database owner, call `initialize_mailbox_tables_with_limits`
   in the first-create transaction and commit it with the embedding's other
   initial schemas. On reopen, call `validate_mailbox_database` instead. Preserve
   errors and existing files; reopening never initializes or resets a mailbox.
2. Implement `MailboxSubmitter` as a command port to that same bounded worker.
   Each admitted command owns its `MailboxRequest` and a reply channel. Return
   `MailboxFailure::Busy` when admission is full. Once enqueued, the worker must
   execute the command even if its result receiver is dropped. A failed reply
   send does not undo the transaction. The synchronous handler is:

   ```rust
   let result = execute_mailbox_request(&database, request);
   let _ = reply.send(result);
   ```

   The embedding retains and drains the worker through accepted mutations. After
   admission, the returned `MailboxFuture` waits for their result. Do not execute these
   blocking database operations on a Tokio executor thread or open a second
   database from `submit`.
3. Prepare the local LXMF identity/destination and call
   `DurableDirectLxmfService::prepare_with_limits(identity, replay_limits)`.
   Register the returned callbacks and destination with the host, following the
   [host integration example](service/examples/live_tcp_peer.rs). Supply that
   host through `DirectNetwork` and the command port through `MailboxSubmitter`.
4. Start paused while the embedding restores host persistence and admission:

   ```rust
   let service = pending
       .start_paused(network, mailbox, Arc::new(SystemMailboxClock))
       .await?;
   // After the embedding's readiness barrier succeeds:
   service.activate_queued_attempts().await;
   ```

   Use `start` when the supplied host is already ready. `ReplayLimits` bounds
   active attempts and page work independently of the persisted mailbox limits.
5. Send through `send_direct_text` and retain the returned local record ID.
   `Accepted` means the queued row committed; subsequent snapshots describe
   delivery. `retry_lxmf_message` and `cancel_lxmf_message` address that same row.
   Dropping a send waiter after admission does not prove it was rejected; inspect
   retained mailbox state before submitting another message.
6. Retire protocol ingress, then await `service.stop()`. Keep the mailbox owner
   available while accepted callbacks settle. If stop is interrupted or times
   out, retain the service and retry its join. After successful stop, drain the
   embedding's database lane before closing its database. Queued rows remain
   available when a new service generation opens the same mailbox; stopping LXMF
   does not stop a shared host.

## Verification

Run from the repository root:

```sh
cargo test --locked -p prns-lxmf-wire
cargo test --locked -p prns-lxmf --features tokio-host,redb-mailbox
./tools/prns build lxmf verify
```

The full verification script checks portable builds, isolated feature consumers
on Rust 1.96 and the declared 1.90 floor, Rust tests and lints, pinned
Python vectors, fixture tests and a two-way loopback exchange. It requires the
`thumbv7em-none-eabihf` Rust target, Rust 1.90.0, and Python 3.13.7; `uv` can provision the pinned
Python interpreter. The test environment defaults to `target/interop/lxmf-1.1.0`;
`LXMF_VENV` can select an existing environment. The separate physical-device
harness remains in `service/interop/python/run_physical_tcp_lxmf.py` and requires
an explicitly selected device/network target.

## Source provenance

See [wire provenance](wire/PROVENANCE.md) for the original bounded wire source and
pinned Python authority and deterministic fixture corpus.
