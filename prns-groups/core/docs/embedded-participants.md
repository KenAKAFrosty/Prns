# Embedded group integration boundary

The repository does not supply a complete embedded network group participant.
The reusable libraries supply the portable pieces of that work. This guide identifies the available components and their missing platform
connections without presenting an unimplemented board profile as supported.

## Available components

| Component | Embedded boundary |
| --- | --- |
| Group core and text codec | Allocation-free `no_std`; the caller supplies fixed target storage and authenticated facts. |
| Transaction coordinator and NOR stores | `no_std + alloc`; bounded staging, recovery and explicit flash ownership. |
| MLS adapter and record service | `no_std + alloc`; real crypto, application records and receipt obligations under explicit limits. |
| Managed enrollment | `std` owner using redb and JSON state; not a portable firmware state machine. |
| Reticulum carrier | Default concrete Link adapter uses the host runtime and Tokio. Disabling its network feature preserves shared mapping/cursor types, not an embedded carrier implementation. |

A routing board need not hold MLS keys or join a group. Forwarding group packets
is therefore not evidence of membership. The code also has no independently
admitted blind-custody service.

## Integration responsibilities

A board consumer must bind credentials to its device identity, persist them before
advertisement, and provide entropy and trusted time. It must commit crypto,
record facts and exact outgoing work together, then retain interrupted operations
until recovery. The existing transaction APIs own those invariants; a firmware
adapter cannot replace them with an unrelated delayed transport journal.

Native enrollment and carrier execution are not usable merely by selecting a
smaller profile. A complete board member additionally needs enrollment/control
execution, authenticated carrier ingress, scheduling and lifecycle integration.
See the
owner contract for their semantics and the
[transport contract](../../transport/reticulum/README.md) for Link ownership.

## Resource and security boundaries

Embedded qualification is specific to board, firmware features, owner/member role
and selected numeric limits. Owner Add/Commit, joining and recovery can peak above
ordinary sends. Include runtime/radio reserves, stack, internal RAM, PSRAM,
serialization copies, fragmentation, persistent indexes and cleanup capacity.
`no_std` does not mean allocation-free; upstream MLS decoding includes infallible
allocations.

A plaintext test wrapping key does not protect secrets from flash extraction.
Software-reset recovery does not qualify electrical interruption, rollback
resistance or secure erasure. The [embedded support guide](embedded.md)
and [secure-flash contract](../../store/docs/secure-flash.md) describe maintained resource tooling and those limits.
