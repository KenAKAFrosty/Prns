# Group behavior contract

This guide summarizes the implemented managed-group behavior. The canonical
operations and error types are in the [group API](../src/api.rs); construction,
resource selection and optional features are described in [usage](../usage.md).
The lower [record service](../service/README.md) also supports pre-enrolled
fixed membership without the managed catalog.

## Membership and control

A managed group has one fixed owner and an immutable selected `GroupProfile`.
The owner signs a chain of control states and authorizes one-member Add/Remove
transitions. Membership binds the complete endpoint/incarnation/signing-key and
Reticulum identity, not only a display name or identity hash. Joining checks the
selected profile, exact roster and MLS transition before committing membership.

Create and user invitation decisions are durable local operations. A saved
acceptance is not a completed join: the group becomes ready only after its MLS
state reaches the required control checkpoint. A summary may therefore describe
an unready group with no authenticated control. Contact cards identify an
invitation service; they neither grant membership nor guarantee reachability.

Members may communicate while the owner is offline using their current
locally authenticated membership. An offline device cannot know that the owner
has made a newer decision. This is cached membership, not a globally ordered
admission service that prevents every stale send during a partition.

## Publication and receipts

A publication commits its body, original metadata, target incarnations and exact
ciphertext with crypto state before it becomes available for delivery. Results
and retries are scoped to the original control and local store incarnation.
An uncertain result must be recovered under that same request; replacing its ID
can create a second operation.

An endpoint receipt authenticates durable acceptance by that endpoint. Targets
remain fixed after publication. A receipt from another incarnation cannot satisfy
one of them, and expiry or removal cannot turn an unfinished target into delivery.
Transport acknowledgements and user-interface display are separate observations.

All current MLS members are in the encryption audience, including members omitted
from a record's target list. Application schemas and effect execution remain the
consumer's responsibility. A received command is not proof that its external
effect ran exactly once.

## Changes and terminal states

- **Add:** commits the signed successor, MLS Add and exact Welcome together.
  Owner-authored records with missing current-era receipts block a new Add until
  they are received or definitely expired. This does not prove that every member
  has drained its offline work. A previously committed Add replays its Welcome.
- **Remove:** the owner revokes one exact non-owner incarnation. Remaining peers
  receive the control transition; the removed peer receives a separate signed
  terminal proof. Old-era live delivery is fenced.
- **Leave:** a non-owner saves its departure request and stops local publication
  and content synchronization. The owner must perform the corresponding Remove.
  An offline owner leaves departure pending.
- **Close:** the owner publishes a terminal proof and retires local crypto state.
  Peers stop when they learn it. Close also supersedes a prior Removed state;
  removed peers do not need newer MLS keys to verify the proof. The group ID
  cannot be reopened.
- **Delete local:** hides local generations and fences their later completions
  before bounded physical namespace cleanup. Owners Close first. Local deletion
  is not remote departure; an already admitted Leave or terminal outbox survives
  until its obligation is settled.

Add and Remove are authorization barriers. Existing local plaintext may remain
readable, but old-audience ciphertext is not re-encrypted for newly admitted
readers. Removal/Close take precedence over finishing delivery. See the owning
[enrollment contract](../enrollment.md) for cancellation, invitation expiry,
control replay and cleanup reservations.

## Rejoin and history

Readmission after removal uses a fresh endpoint credential and store incarnation.
A previous generation can remain a read-only history source; it is never reset
into a new crypto session. Local Delete fences every covered generation, so late
callbacks cannot repopulate it or delete a replacement membership.

The optional archive preserves local records and receipts until explicit deletion
within its selected quota. Without it, history is limited by protocol retention.
Neither mode transfers earlier plaintext to a new member. History retention and
network-key retirement have different purposes and limits.

## Time and capacity

Applications supply trusted time with explicit uncertainty. An unknown clock or
an interval overlapping a deadline establishes neither admission nor expiry.
Request retirement floors and immutable original inputs prevent absent expired
IDs from being executed as fresh work.

Profiles specify supported member and encoded-byte bounds. Local catalog,
namespace, input, page and working-set budgets remain independent. Admission can
fail even below the declared member ceiling; it must preserve existing state and
fund cleanup rather than silently evict obligations or weaken authentication.
The library's eight-member default is a selectable policy, not a hard native
maximum. See [resource policies](policies.md).

## Unsupported behavior

Owner transfer, multiple administrators, automatic periodic rekey, outside writers,
blind custody, permissioned history sharing and strict online admission are not
managed-group operations. The portable adapter exposes some lower-level crypto
operations, but their presence is not a managed policy or a device qualification.
[Security](../security/docs/security.md) and [embedded support](../core/docs/embedded.md)
describe additional boundaries.
