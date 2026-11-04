# Group security profiles

MLS is the selected private group protocol. The [adapter](mls-rs/README.md)
implements recoverable operations, credential binding and explicit storage and
admission quotas with `no_std + alloc`. The generic group core remains
allocation-free and independent
of MLS types. No application may silently weaken protection when memory,
connectivity or keys are unavailable.

Read the [security guide](docs/security.md) for authority, metadata visibility,
delayed delivery, key lifetime and storage exposure.
The [integration contract](docs/mls-integration.md) records the pinned backend's integration
requirements; the adapter README distinguishes implemented controls from gaps.

The [embedded guide](../core/docs/embedded.md) separates portable libraries
from a complete network member. Library builds and recovery tests do not
establish protected storage or physical secure erasure.

## Backend and regression entry points

The [`mls-rs` adapter](mls-rs/README.md), named `prns-groups-mls`, supports
`no_std + alloc`; it supplies storage and admission quotas, credential checks and
recoverable operations over unmodified upstream crates. The
[integration limits](docs/mls-integration.md) distinguish those quotas from
upstream transient allocation and skipped-key retention.

| Scenario | Current regression owner |
| --- | --- |
| Delayed receive across rekey; removal purges prior epochs | [Adapter recovery](mls-rs/tests/recovery.rs) and [secure reopen](mls-rs/tests/secure_recovery.rs) |
| Exact committed ciphertext retry after recovery; membership fences stop stale output | [Adapter recovery](mls-rs/tests/recovery.rs) |
| Failed staging or malformed input leaves a valid retry possible | [Composition](mls-rs/tests/composition.rs) and [adapter recovery](mls-rs/tests/recovery.rs) |
| Receipts retain exact endpoint incarnations, original targets and deadlines | [Portable delivery](../core/src/delivery.rs) and [clock admission](../core/src/time.rs) |

Timed MLS grace, independent custody and live-record FIFO eviction are not
implemented. Capacity failure preserves retained work under the selected policy.
