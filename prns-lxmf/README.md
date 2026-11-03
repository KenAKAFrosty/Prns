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
  No feature selects the app or group-message libraries.

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
a task failure remains a failure on retry.

## Resource selection and accepted work

`DirectLimits` explicitly selects transient history count, retained record-buffer
bytes and active attempts (defaults: 1,024 records, 4 MiB, eight attempts). At the
history limit a new send returns `HistoryFull`; inbound admission increments the
overflow diagnostic without inserting its message ID into deduplication. Existing
history is retained. A previously emitted transport proof does not imply that the
application admitted an inbound message. Record slots and actual buffer capacities
are accounted; bounded callback/peer lanes, dedup index overhead, allocator costs
and caller-owned snapshot copies are separate memory obligations.

## Verification

Run from the repository root:

```sh
cargo test --locked -p prns-lxmf-wire
cargo test --locked -p prns-lxmf --features tokio-host
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
