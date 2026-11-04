# Shared synchronization progress and recovery

`Client::synchronization_progress(group)` is a synchronous, bounded observation
of the runner, available through the Rust and generated foreign clients. It is
independent of text, UI, and durable record receipts. See
[generic records](generic-records.md) for publication, retained records, change
cursors and per-record authenticated delivery evidence.

The snapshot reports whether a runner owns the observation source, whether its
last observed configuration fits the observation budget, that configuration's
control reference, and per-peer scheduling phase,
last observed pending-work presence, remaining scheduling delay, failure count,
observation age and a typed issue. `pending: None` means unknown. A retry delay
is not an arrival estimate. `running` does not establish a connected peer,
mobile background execution, or successful delivery. Device receipts remain the
durable service's authority; neither progress nor a Link proves application
execution or a human read.

The tracker retains at most 32 recently configured groups and 256 peer
observations across those groups. Selecting another group does not erase all
previous observations; older groups are evicted when either budget is needed.
Partial or evicted groups report `observed: false`. This means unknown or
incomplete activity, never a delivery failure. The observation cap never evicts
a delivery obligation. Consumers must compare `control` with the selected
conversation's authenticated control before presenting the peer rows. A mismatch
is an older membership view, not current activity. An empty scheduler step is not
proof that another group's observations should be erased. A failed configuration
read applies its issue only to the selected group. Control head, store
incarnation, endpoint and runner-owner generation fence updates. Dropping the
runner marks its observations unknown, removes retry estimates, and leaves the
database untouched. A replacement runner begins with unknown progress and
reconstructs work from durable records, exact ciphertext and receipts. Old-owner
observations cannot overwrite its state even with unchanged group membership.

`SyncPhase` and `SyncIssue` are allocation-free core enums. The native snapshot
uses bounded owned collections and a short metadata lock. It stores no message
content or secret material and requires no extra database or executor. Consumers
should display observation age and distinguish historic issues from a new local
admission error. Refreshing the projection does not initiate network traffic.

## Recovery decisions

Fresh durable work and current authenticated peer/group exchanges may shorten
an existing wait to the contact-spacing floor. Repeated hints coalesce and do
not reset the failure exponent or bypass capability-probe cooldowns. The scope
includes the group, committed control head, local store incarnation, endpoint
incarnation and authenticated Reticulum identity. An arbitrary successful Link,
announce, or local interface restoration is not this evidence.

When another group/enrollment exchange occupies the same physical peer's slot,
an authenticated exact-scope hint is retained until that slot becomes available.
The current scope is rechecked before use; changing control or store clears the
credit. It does not open another concurrent exchange or wake a group without a
pending outbound obligation. An attempt already active for the hinted group
completes under its existing policy.

Inventory repair receives a turn after four completed Submit exchanges,
including authenticated rejection. Failed path lookup, Link handshake and lost
responses still advance retry watermarks/backoff, but do not consume this repair
quota. This avoids requiring an empty repair round before the first message can
be delivered after an outage. Unsupported/oversized Submit still falls back to
the existing bounded inventory protocol. Receipt reconciliation remains active.

Without fresh work or authenticated scope-specific reachability evidence,
recovery still waits for normal backoff. The scheduling policy does not promise immediate
delivery after a radio toggle, reset every group's retries on resume, or establish
always-on mobile execution.

## Host evidence

Unit regressions cover observation bounds, control/store/owner fencing, runner
drop/restart, monotonic retry estimates, exact-scope recovery held behind a busy
physical peer, preserved contact floors/backoff and repair fairness after failed
transport. Existing independent-endpoint tests cover prompt push and receipts,
offline-author catch-up, stopped/restarted runners, persisted pending work,
multiple groups, constrained carriers and recovery without manual resending.
These host checks do not qualify physical phones or background execution. Keep
separate evidence tied to the exact consumer, build, devices and network. Partial event capture cannot establish continuous diagnostic coverage.
