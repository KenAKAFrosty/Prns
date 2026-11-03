# Service runtime

`prns-service-runtime` assembles selected service owners, shared storage, identity
admission, and their startup/shutdown lifetimes. Product policy and installation
bootstrap belong to the consuming application.
Its default feature set is empty. `identities` provides physical
store attachment and a single-lane owner over the existing identity engine;
`groups` selects the concrete local group adapter. Optional `contact-card`
exposes the canonical identity owner's offline signed claims; it selects no group
or messaging service. Optional `archive` selects
durable history at initialization; `groups-network` supplies bounded host routing
for the selected network adapter. Independent identity and group
libraries and their standalone owners remain usable without this crate.

Select `standalone` for an owner that creates and retains its service executor
and host. It includes `groups-network`; `connections` additionally selects the
host's saved-profile binding. The lower `groups-network` factory instead uses a
supplied host and executor, whose lifetime remains the embedding's responsibility.

## Choose a construction path

Use the independent identity or group owner when one domain is enough. Select
this runtime when those domains must share identity admission, a storage worker
and coordinated shutdown.

| Construction | Host and execution lifetime |
| --- | --- |
| `groups::create_new` / `open_existing` | Local identity and group state without a network carrier |
| `groups::network::create_new` / `open_existing` | Borrows the embedding's host and executor |
| `groups::standalone::Owner` | Creates and retains its service executor and configured host |

For a standalone consumer:

1. Select explicit storage/resource options and choose `create_new` for new state
   or `open_existing` for existing state. The offline constructors need no host
   configuration.
2. Await `ready()`, then obtain identity clients and group clients for configured
   identity/service bindings. Local history does not require network readiness.
3. For an offline owner, first attach a configured host with `start_networking`.
   Await `network_ready()` before borrowing the host.
4. Await `stop_networking()?.wait()` to finish network teardown while retaining
   local clients and history.
5. Retain and await the completion returned by `close()` to drain the entire
   owner. Dropping a client or a waiter does not establish cleanup.

## One owner and ordinary clients

`local::Owner<I, O, S, L, D, J>` owns one identity catalog/process lock and one
bounded worker. It uses `prns_identities::owned::run`, its canonical
identity client, lifecycle controller, retained effects and reserved control
admission. The construction factory receives strictly owned local stores, the
validated startup catalog, a weak progress waker and an ordinary same-lane command
port. The port carries no lifecycle-control capability. The factory may validate local state;
it must not activate live effects before the scoped driver protocol.

`groups::{create_new, open_existing}` select `groups::Local` and the existing
opaque group job as that owner's extension. `groups::client` returns the ordinary
`prns_groups::Client` bound to an immutable `IdentityReference`. A typed
route function can embed that domain job in a multi-domain assembly's sum type.
There is no copied method/model list or application command facade.

The selected group owner retains one canonical `InputBudget` for its physical
lane. `owner.submission()` clones that allowance together with the existing
command port; pass it to `groups::client`. The activated network driver and
repeated standalone `groups(reference)` calls share the same allowance.
`Options::new` requires the owning `ClientLimits`; `with_client_limits` replaces
that explicit selection before construction.
Input accounting stays with accepted jobs through execution, even after a reply
waiter is dropped. It does not account for unpolled caller inputs, responses or
executor scratch space. Custom multi-service embeddings construct one
`SubmissionState::new::<I>(input)` using their identity-owner capacity, give a
clone to `groups::Local::new`, and retain that exact state in `Submission` or
`groups::Owner::from_local`. Separately wrapping the same budget creates a
different observation owner and is not equivalent.

With networking selected, this state also owns at most `I` read-only runner
observation sources. Ordinary clients look up the full identity reference without
allocating entries. Only an admitted runner publishes a source, and its retained
lifetime guard removes that exact identity/activation binding on completion or
unwind. Clients acquired before activation follow later observations; clients
from a closed physical owner cannot observe a reopened owner. The existing
per-runner group/peer bounds and control/store-generation fences still apply.
These observations do not change dispatch authority, notifications or durable
delivery receipts.

The group manager derives History, Recovery, LocalMutation or ServiceMutation
from each actual operation. The adapter checks the real current identity
coordinator on the same lane and retains its exact runtime mutation permit when
needed. The full group request ID stays in the domain's durable evidence; a
separately tagged hash identifies the live mutation lease. Result recovery runs
before fresh mutation admission. Dropping a permit never asserts settlement.

## Explicit durable preparation

The root contains `identities/` and `domains/`. Creating an identity does not
create any service database. An explicit retained AddService prepares that
service before the catalog records it as configured. One immutable identity
binding, selected schemas and their first durable observation stamp commit in
one transaction when the first file is created. Subsequent selected services
share that file and physical cache owner. Each retains its own schema and codec.

The group preparation unit is enrollment, retained submissions and the group
change stamp, plus an explicitly selected archive. Archive records and receipt
updates commit in the same physical transaction as their canonical domain changes.
The archive preserves history until explicit deletion, rejecting writes visibly
when the configured identity-wide record/byte budget is full. Reopen honors the
persisted selection and limits without migration or backfill.
`Options::new` requires the canonical catalog policy and shared client input
limits alongside local storage and retention policy. Reopen and interrupted
AddService recovery honor the stored catalog selection, even when the policy
selected for fresh initialization changes. Invalid selections fail before any
installation files are created.
Complete absence permits explicit initialization; complete
supported state permits exact recovery after a lost catalog acknowledgement.
Partial, corrupt, mismatched or unsupported state fails without replacement.
Ordinary reopening requires every configured schema to exist. Removing a service
does not erase its historical data; a bound history client can reopen existing
supported storage without preparing a new schema.

`Stores<I>` holds at most I physical owners and checks the explicit per-file and
total cache budgets. Database views are borrowed sequentially inside synchronous
owner handlers. Group-owned `Attachment` retains subscriptions and identity/store
incarnation binding across these short views. Attachments close before physical
stores. Local history ownership does not retain a private activation key.

`Options::with_validation_cache_entries` selects a bounded resident working set
per group attachment. The default is one entry; this is independent of stored
group count. The attachment retains the exact physical database allocation and
exclusively lends its validated namespace evidence to each lexical manager.
Cold misses still inspect every row; small caches may thrash. Attachments and
their cached handles are declared before physical stores so teardown drops them
first. Ordinary clients and observation handles hold neither database ownership
nor a cached proof. Physical close completion must release these handles before
the same files can be reopened.

Startup/initialization failures can leave files whose publication or schema
transaction did not complete. Preserve those files and the exact caller-owned
identity operation; never delete them or silently fall back from open to create.
A file whose immutable binding did not commit is an explicit failure requiring
inspection, not permission to invent a replacement identity or schema.

## Local capability and close semantics

`groups::Local` supports local history, retained-result observation and exact
recovery. It reports `NetworkUnavailable` for Enable because it owns no carrier.
`groups::network::{create_new, open_existing}` select the concrete retained
network driver over the same catalog engine, physical stores and ordinary group
client. `OwnerOptions` supplies explicit storage limits, carrier bounds,
synchronization policy and an initial optional announcement interval.

`Owner::announcements(reference)` returns the canonical identity announcement
client (`settings`, `set_settings`, `announce`). `None` selects manual-only
announcements. The file-backed scheduler under `domains/announces` owns one
shared cooldown and round-robin selection across identities and restarts.
Suspension clears pending announcements without refunding spent airtime budget.
Dispatch requires activated endpoints and an eligible interface. A `Throttled`
result means the request was coalesced into pending work; it is not a delivery
receipt. Changing the constructor's initial interval does not replace persisted
settings.

With `contact-card` selected, the enrollment destination advertises a canonical
signed card containing only the identity's `published_name`; `local_label` stays
private. Its 148–276 bytes fit the carrier's unratcheted announcement budget.
Committed name edits update registered app data through the same scoped host,
without sending traffic or clearing queued announcement intent. New metadata
must satisfy the optional card name policy before commit. Existing metadata
outside that policy omits the card while messaging remains available; an edit
to a valid published name enables the card. A signed card is a public service
claim, not human verification, saved-contact state, membership or delivery proof.

Network construction projects payload support through the transport's owning
frame rules, including the synchronization Submit wrapper. Persisted profiles
must fit before readiness; fresh Create/Offer/Respond use the same support on
the domain lane. A smaller carrier selection never rewrites a group's immutable
profile. Offline owners do not acquire this carrier constraint. Custom network
drivers apply the same checks to already-attached and future group catalogs.
The selected client allowance must also fund the full maximum carrier frame
plus `ClientLimits::required_job_bytes`' owned-job overhead. Invalid construction
budgets fail before filesystem effects; later input saturation returns Busy.

Install `network::Router` in the host's embedding before host startup,
then build `network::Context` from that exact router, the resulting `HostClient`
and one supplied Tokio executor handle. The embedding callback owns group input
before any foreign event queue. `Router::on_event_ref` also permits a typed
embedding to route another selected protocol in the same callback.
The embedding must keep the actual host owner and running executor alive until
all identity owner completion observers have resolved; an executor handle alone
does not keep its runtime running.

The driver registers both canonical group endpoints with ingress staged closed,
retains their exact endpoint tokens before its first asynchronous registration,
and owns a runner waiting for the canonical identity activation confirmation.
Only that confirmation supplies the runner's immutable network permit and opens
its restrictive dispatch lease. Every queued embedding callback is checked against
that captured owner/generation, including reads and recovery calls. `Running`
means the activated capability is owned, not that a peer is reachable or a message
was delivered. Post-confirmation task failures fence dispatch and remain visible
through `Context::diagnostics`; observing a diagnostic neither changes durable
enablement nor proves resources have drained.

Pause and owner close fence dispatch synchronously, stop/join the retained runner,
settle actual host operations, and finish both exact endpoint retirement barriers
before removing the callback route and releasing its key lease. Conservative
outgoing Link candidates after closure-history overflow still pass through those
barriers. Partial registration failures retain both endpoint tokens, so cleanup
retires only this owner and preserves a conflicting foreign registration.
Cleanup never closes the supplied host or its interfaces. Exact uncertain receive
recovery uses the original identity permit and Attachment continuation, including
during closing.

Durable group obligations come from the group library's bounded dependency scan,
not UI conversation counts. The owner advances at most four membership
namespaces per observation poll and schedules subsequent pages with its weak
waker. Only a complete unchanged stamp becomes a report. Active memberships,
pending work and unresolved operations remain blockers; passive archives and
replay fences are retained without becoming permanent retirement blockers.
Missing/corrupt/incomplete observations never become a zero report.

`close()` fences admission synchronously and returns a retained completion
observer. Dropping an observer or the owner does not abandon accepted effects.
Actual service cleanup and key leases drain through the existing ephemeral
termination authority, preserving durable Enabled/Paused intent. The owner does
not spawn an executor per service or depend on an ambient Tokio runtime. Startup
is blocking and should run off a UI thread. Domain callbacks remain on the owned
lane, outside queue locks; their work must be bounded and synchronous.

## Standalone startup policy

`groups::standalone::Owner` retains asynchronous startup and teardown over that
same local engine and one owned host created from the supplied configuration.
`Options::startup_recovery` is explicit:
`Dormant` leaves saved identities inactive; `RecoverCurrent` schedules canonical
`IdentityClient::recover` for the exact saved Enabled identities and identities
with accepted pending work or recovery cleanup. It does not begin Reserved
operations, change desired state, or initialize absent configured schemas.

`create_new_offline` and `open_existing_offline` construct local data without a
`HostConfig`, interfaces, permission requests or network recovery. The matching
`*_offline_with_connections` methods retain a separately owned profile store.
Use this boundary before optional OS foreground-service or network admission so
an admission failure still leaves data accessible. Start networking later with
its explicit configuration and fresh platform embedding.

`ready()` acknowledges local data construction. It does not wait for a host,
interfaces, or identity activation. Identity records, contacts, group history,
and retained-result observations remain on the same data lane while networking
is stopped or startup fails. Fresh service mutations still require their normal
canonical identity admission; availability of history does not grant dispatch.

`network_status()` reports the separate network phase, latest failure and host
generation. `network_ready()` observes the most recently accepted transition.
`stop_networking()` returns a retained completion: it drains actual service
work and key leases before retiring connections and joining the host. It does
not close storage or change saved Enabled/Paused intent. Dropping a transition
observer does not cancel it. A fresh `start_networking(host_config)` consumes
explicit host policy; native platforms use `start_networking_with_embedding`
to supply fresh one-shot interface preparation. Clients for the previous host
generation stay fenced; reborrow them after the new generation is Ready.
The selected `StartupRecovery` policy applies to each network start.

`close()` (foreign `shutdown()`) is the separate full owner close. It fences the
local lane, drains accepted work, then releases interfaces, executor and storage.
Only full close completion allows reopening the same files with another owner.
Network cleanup that cannot prove completion remains Unresolved and retains its
resources; it never turns into a successful stop or permission to replace them.

A failed local close preserves its typed identity or storage cause. `Phase::Unresolved`
and `cleanup_complete`, rather than the cause's variant, determine whether cleanup
has finished. Storage failures include a stable reason and diagnostic detail;
host and connection failures still carry diagnostic text where their Rust types
have no owning foreign projection.

Post-activation network task diagnostics currently remain on the network context;
the standalone status exposes identity diagnostics only. An unexpected runner
failure can therefore leave construction readiness visible without exposing that
service failure. Ordinary peer/network failures retry inside the runner. This
reporting limitation does not make Ready a claim of connectivity or drainage.

Selected-service attachment failures retain their canonical group or identity
classification, or the physical-store cause, in `ServiceRuntimeError::Attachment`.
Its diagnostic detail is capped at 1024 UTF-8 bytes. Failed startup publishes the
same cause through `ready()` and retained status, including after cleanup. This
diagnostic is separate from identity admission classification and never permits
replacing, migrating or deleting saved state.

## Owner checks

Run the registered checks from the repository root:

```sh
python3 validation/run.py run --suite service-runtime-minimal --suite service-runtime-identities
python3 validation/run.py run --suite service-runtime-without-connections
python3 validation/run.py run --suite service-runtime-tests --suite service-runtime-clippy
```

The minimal checks cover empty and identity-only feature selections. The
standalone check keeps saved connections disabled; the all-feature checks cover
the selected group/network/archive and connection owners. Suite definitions own toolchain and platform selection; these commands do not establish phone, radio, or signed-package behavior.

`tests/local.rs` exercises real production construction, dynamic schemas,
ordinary group clients, admission before retained acceptance, retained
subscriptions, unsupported network activation, exclusivity and strict reopen.
Its private settings fixture selects a second actual file-backed service using
the same worker/database and an ordinary typed sum. It exercises a lost schema
acknowledgement, stable dependency observation, settings writes, key-retaining
activation and delayed cleanup after owner loss. The private service is a test
consumer, not a production service catalog entry. The network driver fixture uses the production factory and actual staged host
endpoints to verify close/reopen, lost owner handles, partial registration,
post-confirmation startup failure, stale generation rejection and endpoint cleanup
after closure-history overflow.

`tests/network.rs` starts two public network owners with catalog-generated keys
and no preinstalled service destinations. It issues signed group-service cards
while paused, configures/enables groups, obtains explicit peer consent, and
exchanges generic records and authenticated receipts over real Reticulum Links
and Resource transfers. Pause/owner close/reopen/Enable exercises offline catch-up;
terminal group Close, archived history and explicit local Delete use the same
ordinary client workflow helpers as the independent group-only consumer. These
are host-level peer and ownership checks, not mobile or physical-device acceptance.
