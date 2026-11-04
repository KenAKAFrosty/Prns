# Group resource and recovery policies

The [managed owner](../usage.md), [record service](../service/README.md)
and [MLS adapter](../security/mls-rs/README.md) enforce different policies.
This guide identifies which selection owns a limit; it does not define another
profile, negotiate with peers or supply application defaults.

## Authenticated profile and local budgets

| Selection | What it bounds |
| --- | --- |
| `GroupProfile` | Per-group member count, record body, MLS plaintext/wire/snapshot and encoded enrollment frame. It is retained in Create and authenticated by enrollment. |
| `CatalogLimits` | Optional group-count quota, catalog bytes, rows, transaction and maintenance work, unsolicited invitations and canonical namespace limits. |
| `RedbLimits` | Namespace keys, values, staging, live objects/bytes and durable receipts. |
| Service/MLS limits | Retained records, envelopes, targets, outputs, proposals, key packages, prior epochs and admitted MLS wire/snapshot bytes. |
| `ClientLimits` | One accepted-job input allowance shared across clients on the owning lane. It excludes responses and caller-owned unpolled inputs. |
| `PageLimits` | Rows and returned owned bytes for catalog, invitation, member, retained-operation and enrollment-work pages. Decoding/scratch is separate. |
| Local queue/cache options | Pending storage jobs, redb cache and optional resident validation proofs. These do not cap stored conversation count. |
| Network work policy | Framing, ingress, concurrent work, per-peer scheduling and retry timing. These are not airtime or delivery guarantees. |

The canonical declarations and validation live in
[`profile.rs`](../src/profile.rs),
[`CatalogLimits`](../src/enrollment/indexed/policy.rs),
[`RedbLimits`](../store/redb/src/lib.rs),
[`ClientLimits`](../src/client/inputs.rs) and
[`PageLimits`](../src/page.rs).
`resolve_group_profile` uses owning serializers and checked arithmetic to derive
record/enrollment representation bounds from selected policies. It does not
reserve a maximum-sized roster or promise that all admitted fields can reach
their maxima simultaneously. Carrier support is checked before network readiness
and before enrollment accepts unsupported profiles.

Stored catalog and namespace policy is immutable on ordinary reopen. Reopening
with a different fresh-create selection does not migrate data. Unsupported or
partial formats are rejected intact.

## Cleanup and quota exhaustion

Ordinary admission accounts encoded state and reserves the future facts needed
for Remove, Close, terminal delivery and local Delete. Liabilities follow actual
admitted memberships and outstanding work; unused member capacity is not charged
as if those members existed. A removal does not erase the former member's Close
notification obligation. Invitation changes and terminal cursor growth are also
funded before they become required cleanup.

Capacity failure preserves the committed state. It is not permission to drop
unsettled receipts, evict live crypto or issue a replacement request. Reclaiming
completed evidence requires the owning retirement rule and replay floor.
Maintenance progresses bounded phases; reading a work page does not run it.
Consumers that drive the owner themselves must schedule maintenance explicitly.

Logical byte quotas do not measure filesystem size or process RSS. Redb pages,
allocator overhead, copies, upstream MLS allocations, queue metadata and transient
serialization require separate resource allowance. Current record/message pages
have a checked row limit of 1–100 rather than the catalog's byte-bounded result
contract. A consumer choosing large record bodies must budget those responses
and any accumulated pages.

## Delayed delivery across routine rekeys

The MLS adapter supports a bounded number of prior epochs and upstream
out-of-order receive. Skipped-key retention and forward derivation follow
upstream MLS behavior; the adapter does not impose exact key-count or receive-work
budgets. Its storage checks can reject an expanded snapshot after the transient
MLS allocation and computation have occurred. Prior-epoch retention is
count-based, not a trusted-clock grace period.

The managed client exposes no periodic rekey scheduler. Managed Add and Remove
purge old receive epochs and fence old application/proposal retries. A locally
accepted record can remain in history, but this does not authorize unseen old-era
traffic or disclosure to a newly added member. The adapter's lower-level rekey
operation must not be described as a managed daily refresh or automatic
post-compromise recovery policy.

## Storage assurance and retention

The transaction coordinator distinguishes definite rejection, committed output
and uncertainty requiring recovery. The backend must provide atomic publication
and durable operation evidence. Application side effects happen after commitment
and need their own idempotence policy.

The optional redb archive retains local history until Delete within its explicit
quota. Protocol outputs and receipts have their own retention/reclamation rules.
Acknowledging output can reclaim its bytes while its operation ID remains fenced.
No library guarantees that arbitrary offline time fits the selected storage.

Native redb stores plaintext and logical deletion does not scrub old pages or
backups. The opt-in secure NOR backend has a separate
[flash contract](../store/docs/secure-flash.md); a plaintext test wrapping key
provides no physical confidentiality. Neither crash recovery nor a same-disk counter proves
resistance to restoring a complete old valid image.

## Discovery and protocol outcomes

Reachability, membership, local storage and remote receipt are distinct facts.
An announce advertises a service destination. A successful Link authenticates
its peer. A saved invitation decision is durable intent. A ready group has
committed its membership checkpoint. A durable endpoint receipt confirms record
acceptance by that incarnation. None substitutes for the next fact.

The [announce policy](../transport/reticulum/docs/announce-strategy.md) and
[progress API](../progress.md) expose local scheduling and observation age.
An offline or unknown peer is not evidence of rejection; a completed transport
exchange is not evidence of human reading or external command execution.
