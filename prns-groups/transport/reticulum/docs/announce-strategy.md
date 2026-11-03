# Group destination announces

Announcements advertise destination reachability and public keys. They do not
carry membership, inventory or receipts. New records use peer-directed
synchronization rather than an announcement per message.

## Current policy

Each active local identity uses `prns.groups` for enrolled synchronization and
`prns.groups.enrollment` for identified bootstrap requests. Those destinations
multiplex all its groups and use empty announce application data. Their public
identity associates the services, but their payloads reveal no group list or
inventory head.

The [group runner](../../../src/announces.rs) owns a process-local automatic timer
and one-slot manual lane. The default `SyncPolicy` uses six hours; callers can
select another supported interval or manual only. Positive jitter adds up to
25 percent. A 60-second cooldown bounds admitted batches. `Requested` means the
local host accepted the destination commands, not that a peer received them.
`Throttled`, `Busy` and `Unavailable` retain their local-admission meaning.

The separate [identity scheduler](../../../../prns-identities/runtime/README.md#shared-announce-scheduling)
is available to embeddings that select it. It durably reserves a shared batch
budget, applies owner cooldown/fair selection and fences dispatch after storage
failure. Failed or uncertain submission spends that reservation. An embedding
using it must avoid a second automatic timer for the same services.

Manual-only selection disables automatic startup and periodic work; explicit
manual requests and requested path responses remain available. Group count does
not add destinations. Multiple active identities do add service traffic, and
interface rate control can delay or suppress announcements. Configured intervals
are not a total radio-traffic budget or a delivery deadline.

## Reachability and ownership

`connect_identified` requests a missing path and checks the destination public
key against the expected identity before Link establishment. It does not wait
for a periodic announce. Registered SINGLE destinations answer requested discovery
under the transport's cache/interface rules.

A failed endpoint Link attempt can retire its exact attempted route and request
rediscovery without discarding newer authenticated activity or a replacement
route. Requested repair has its own destination throttle. It is separate from
unsolicited refresh and still obeys interface pacing. Announcements, local radio
restoration and an arbitrary successful Link do not prove group membership or
record receipt.

## Verification and observation

Owning tests cover timer/cooldown policy, bounded manual admission, and the
separate durable scheduler's reservation/restart behavior. Carrier tests exercise
requested discovery independently of unsolicited refresh. These checks do not
measure physical airtime or battery use.

For an [isolated LoRa trial](lora-qualification.md), distinguish unsolicited
announces, requested path responses and group exchanges. Forwarding, retries and
other services contribute to aggregate load; a quiet group timer does not prove
a quiet radio.
