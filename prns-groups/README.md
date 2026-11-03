# Reusable group communication

`prns-groups` is the owning group library; its [usage guide](usage.md) covers
local owners, clients and optional networking. The lower crates below separate
portable policy, records, storage and transport.

Address independently credentialed endpoints as one logical group, distribute
records across intermittent contacts and reconcile durable device receipts.
Messaging and telemetry share the same record APIs; applications own schemas,
projections and effects. The group layer uses existing Reticulum SINGLE
destinations, Links, requests and Resources without changing core routing.

The [architecture](docs/design.md),
[behavioral contract](docs/behavior.md), [resource policies](docs/policies.md)
and [security guide](security/docs/security.md) describe their current contracts. Managed enrollment uses one immutable owner
for ordered additions and removals. Management distinguishes cancellation of a
pending invitation, endpoint departure, owner closure and local history deletion.
Fresh re-enrollment uses new credentials and storage while retained old history
remains a separate source. Ownership transfer and authorized history sharing
remain separate work.

| Component | Boundary |
| --- | --- |
| [Core](core/README.md) | Allocation-free `no_std` identifiers, bounded policy and delivery evidence |
| [Transaction store](store/README.md) | Bounded `no_std + alloc` commit/recovery coordinator and optional secure flash backend |
| [redb store](store/redb/README.md) | Caller-owned database adapter; storage is plaintext |
| [MLS adapter](security/mls-rs/README.md) | `no_std + alloc` crypto operations with storage/admission limits, committed with application facts and exact output |
| [Records](service/README.md) | Opaque application bytes, receipt intent, retention and inventory; `no_std + alloc` |
| Foreign bindings | Owning declarations and a standalone local factory, with one image for key and group namespaces |
| [Text profile](profiles/text/README.md) | Allocation-free borrowed UTF-8 encoding |
| [Reticulum transport](transport/reticulum/README.md) | Native adapter using a borrowed Host client and authenticated peers |
| [Group library](usage.md) | Caller-owned storage jobs and network future; [enrollment](enrollment.md), [generic records](generic-records.md) and [progress](progress.md) |

A receipt proves durable acceptance by an enrolled endpoint incarnation. It does
not prove a human read, application execution or network-wide delivery. Delivery
targets select obligations; all MLS readers remain in the encryption audience.
Plaintext history retention is separate from ratchet-key retirement.

For embedded work, start with [support boundaries](core/docs/embedded.md)
and the [integration guide](core/docs/embedded-participants.md). Software-reset tests do not establish electrical interruption behavior, protected
provisioning or resource margins for another build.

The private application formats remain provisional. Host recovery tests do not
qualify mobile background behavior, protected phone storage or board power loss.
Embeddings must qualify their actual storage and device execution.

From the repository root:

```sh
cargo test --locked --manifest-path prns-groups/Cargo.toml --workspace --features prns-groups-store/secure
python3 validation/run.py run --suite group-communication-tests --suite group-communication-clippy --platform current
```

Follow the [contribution guide](../CONTRIBUTING.md) and
[verification ladder](../docs/testing.md). Applications own transport lifetime,
clock, entropy, storage and scheduling; these libraries do not create a second
node or implicitly select a security profile.
