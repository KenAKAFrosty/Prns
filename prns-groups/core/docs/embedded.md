# Embedded group support

The group core and text codec are allocation-free `no_std` libraries. The record
service, transaction coordinator and MLS adapter use `no_std + alloc` with
explicit storage and admission limits. These limits do not cap all transient
upstream MLS allocations.
Managed enrollment, redb storage and the concrete Reticulum carrier use `std`.
They are not complete embedded implementations. A board can route group traffic
without joining a group or holding MLS keys.

## Support and ownership

| Component or target | Current support |
| --- | --- |
| Host owner | Managed lifecycle, record service, borrowed-host networking and history. Redb stores plaintext. |
| Portable libraries | Core types, text codec, record service, MLS adapter and transactional backends with caller-owned resources. |
| Shipping hotspot firmware | Reticulum routing; no complete independently enrolled network group-member service. |
| Blind custodian | No implemented group custody service. |

The [integration boundary](embedded-participants.md) identifies the available
library seams. The [MLS adapter](../../security/mls-rs/README.md) is authoritative
for implemented quotas and crypto operations. No target may silently weaken its
security profile when resources are insufficient.

## Resource contract

A resource claim belongs to a specific board, firmware feature set, owner/member
role and numeric policy. Owner Add/Commit, join and recovery peaks can exceed
ordinary send costs. Physical RAM, linked static headroom, reserved stack,
allocator usage and largest free allocation are different quantities.

| Resource | Required accounting |
| --- | --- |
| Internal RAM and stack | Radio/interrupt reserves, executor futures, crypto work buffers and call chains. PSRAM cannot replace every internal allocation. |
| Heap | Decoded MLS state, snapshot copies, staging, indexes and fragmentation. Upstream infallible allocations prevent a general recoverable-OOM claim. |
| Persistent state | Current/prior crypto epochs, membership facts, records/targets, exact outputs, receipts, replay floors and pending enrollment. |
| Maintenance | Space for the largest admitted transaction, catalog/capsule replacement and safe retirement. A spare compaction arena must fit the live set. |
| Network and work | Largest encoded control, bounded ingress/queues/transfers, retry work, scheduling yields and watchdog deadlines. |
| Time | Trusted clock uncertainty and durable expiry floors. Uptime alone cannot establish expiry across reboot. |

Use the maintained [memory contracts](../../../personal-hopspot/memory/src/profiles/mod.rs)
and [resource validation](../../../docs/embedded-assurance.md) for the actual
firmware. Neither PSRAM presence nor a hotspot resource report allocates a group
budget or establishes the combined workload's limits.

At one 1 KiB record per hour, 14 days of bodies alone is 336 KiB; per-member
publication scales that storage with membership. Targets, receipts, crypto state,
control history and cleanup reservations add to it. Multiple persisted groups
consume quota even while unloaded.

A board backend must commit crypto, application facts and exact output together
before publication. The transport journal's separate record format and batching
policy do not implement that contract. See the
[coordinator](../../store/README.md) and
[secure flash contract](../../store/docs/secure-flash.md).

## Qualification boundary

Library cross-builds establish compilation. Host recovery tests establish their
modeled failure behavior. Linked resource reports describe the selected ELF and
memory contract. None alone establishes a complete networked board member,
protected key storage, electrical recovery or physical delivery through LoRa.
Those claims require the corresponding implementation and device evidence.
