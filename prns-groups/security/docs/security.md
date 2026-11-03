# Group security and privacy

The implemented private group path uses the
[`mls-rs` adapter](../mls-rs/README.md), a fixed owner and authenticated member
bindings. The [integration contract](mls-integration.md) records the
selected upstream crates and adapter policies. This guide describes the current
trust and exposure boundaries; it is not a security audit or a promise that every MLS
profile, storage backend or device is qualified.

## Authority and credentials

Each managed member binds a Reticulum identity to an MLS endpoint, incarnation
and signing key. The owner signs the roster and its predecessor relationship.
The adapter checks the actual MLS roster against that authenticated successor
before committing it. Only the fixed owner commits managed Add/Remove changes;
closure is a separately signed terminal decision.

Contact cards and verified Link identities authenticate routing claims. They do
not establish group membership, human identity or consent to an invitation.
Names are presentation data. A fresh readmission uses a new credential and local
store incarnation, preserving fences against delayed work from an older one.

## Content and metadata

| Observer | Current exposure |
| --- | --- |
| Current MLS member | Application plaintext and metadata for the group. A record's target subset does not narrow the encryption audience. |
| Invitation/control peer | The enrollment metadata sent to it, including the signed membership/profile facts needed to join or catch up. Enrollment uses an authenticated Reticulum Link; catalog copies remain plaintext locally. |
| Routing infrastructure | Reachability and observable traffic patterns. End-to-end content protection does not hide size, timing or service contacts. |
| Holder of a native database or backup | Stored plaintext history and serialized secret state. Native redb has no built-in at-rest encryption or rollback protection. |

The carrier uses SINGLE destinations and identified Links, not Reticulum GROUP
multicast. The announce payloads contain no group list or membership inventory.
The MLS envelope still has public protocol fields such as group and epoch; this
is not a traffic-analysis or anonymity system. No padding or cover-traffic policy
is implemented by the group owner.

## Reticulum protections and their limits

Link and Resource completion are transport observations. They do not prove a
remote application committed the record. The group service's authenticated
receipt binds the exact record and target incarnation and is persisted with
consumed receive state. Neither receipt kind proves a human read or an external
application effect.

A valid member can copy plaintext, retain keys or disclose content to others.
Removing that member cannot revoke plaintext or old ciphertext already available
to it. Delivery targets and UI visibility are not confidentiality mechanisms.

## Membership barriers and key lifetime

Managed Add and Remove establish authorization barriers. Updated peers purge
prior receive epochs and reject stale live application/proposal retries. Exact
control and enrollment outputs remain available for recovery. Old local history
can remain readable; it is not re-encrypted into a new audience.

A partitioned sender can use locally cached membership before learning a Remove.
The implementation does not impose an online admission gate on every record,
so it cannot promise that no stale ciphertext is ever released after an unseen
owner decision. Removal and closure take effect at a peer when it authenticates
and commits the corresponding transition.

The lower MLS adapter exposes update/rekey operations and bounded prior epochs.
The managed API does not schedule periodic member refresh, an inactive-member
removal timer, a timed grace period or automatic compromise recovery. Owner-only
updates would not by themselves refresh every compromised member's secrets.
See retention policies.

## Atomic processing and recovery

Crypto state, application admission, exact ciphertext and operation evidence share
one transaction. Authentication or control-validation failure discards the staged
attempt. Authenticated application rejection instead retains consumed receive
state and its rejection decision so replay cannot reprocess it as fresh content.
An uncertain publication fences new operations until the original result is
recovered. Retry never regenerates ciphertext from an old snapshot.

The redb transaction participant publishes catalog/archive facts with the same
crypto mutation. Local Delete first fences generations and stale completions,
then reclaims namespaces in bounded steps. It preserves terminal and departure
obligations that must survive deletion. These are durability and authorization
properties, not proof of physical media erasure.

## Storage protection

Native storage uses private-file ownership checks but stores plaintext. Redb
pages, filesystem snapshots and backups can retain deleted data. Restoring a
complete valid old image is outside the rollback claim of ordinary crash recovery.

The separate secure NOR backend encrypts objects and retires obsolete capsule
keys according to its [storage contract](../../store/docs/secure-flash.md).
A plaintext test wrapping key provides no confidentiality against flash
extraction. Hardware key provisioning, electrical interruption and physical erase behavior require their own evidence.

Zeroizing selected owning buffers reduces some exposure. It does not establish
that upstream serialization, allocation, foreign conversions or operating-system
copies leave no secret remnants. Upstream infallible allocation also prevents a
general recoverable-OOM claim.

## Scope and review entry points

The managed profile has no owner transfer, threshold governance, anonymous writer,
blind-custody service or permissioned history-transfer operation. A router carrying
group traffic is not a member. A local MLS fixture is not a networked embedded
member. There is no silent downgrade to public content when private admission fails.

Review [credential and MLS policy](../mls-rs/src/policy.rs),
[transaction composition](../mls-rs/tests/composition.rs),
managed lifecycle and
[embedded boundaries](../../core/docs/embedded.md) together with the selected
dependency graph. Host tests and strict lints exercise those contracts; they do not
replace cryptographic review or platform-specific storage qualification.
