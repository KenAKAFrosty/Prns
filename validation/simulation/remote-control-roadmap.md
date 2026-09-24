# Remote Control simulation roadmap

Baseline: `558295da4`. This is the implementation ledger for the agreed qualification plan.

## Contracts

Unauthorized admission remains silent. Already admitted requests may finish after revocation; watches are canceled when their grants disappear. Authority activates only after confirmed durability. Lost responses and canceled callers do not undo committed authority. Uncertain publication retains transaction ownership. Ordinary flushes share the authorization owner's revision checks.

Durable authorization currently serializes new request admission. Watch timers and cleanup must continue, and requests resume after settlement. Keys stay pinned. App messages retain the 96-byte request/response bounds. Capabilities derive from installed handlers and configured service, separately from grants. Application state needs no Remote Control traits.

Production changes are narrow ownership seams and independently demonstrated fixes. Use named policy enums, typed errors, explicit fixture budgets and one authoritative owner. No radio mutation, new streaming feature, Embassy watch producer, physical hardware qualification or broad adjacent cleanup is included.

## Milestones

1. **Controlled persistence and fixtures:** inject a Tokio persistence-I/O driver shared by ordinary flush and authorization work; retain native filesystem execution. Gate operations and script storage/confirmation outcomes. Extend fixtures to administrator, differently granted operator and outsider. Distinguish graceful shutdown and abrupt removal.
2. **Durable authority and recovery:** local/remote authorize, update, revoke, regrant, unchanged/missing/capacity outcomes, permission restrictions, canceled callers, ambiguous publication, lost responses, rollback and two fresh restores. Combine whole-node cases with existing journal byte-cut evidence.
3. **Revocation races:** admitted requests, pending response lanes, reserved/active watches, permission-only removal, full revocation, rapid regrant, identifier reuse, link loss and shutdown. Old completions must not retire replacement leases.
4. **Multiple controllers and pressure:** identity/response isolation, bounded request/response/watch/reader saturation, stalled readers, deadlines, cancellation and reconnect. Prove watch progress during durable work and fresh traffic after pressure relief.
5. **Pairing and restart:** complete production approval/rejection flow, invitation/identity failures, expiration, replay, concurrent attempts, independently interrupted controller/target persistence and lost completion. Restart each side and both, preserve pinned trust, reject changed keys and stale boot work.
6. **Inventory convergence:** topology/configuration/peer changes between pages and watch polls; coalesced invalidation, gaps, overflow, reconnect/refetch. Compare inventory after quiescence without promising frozen pagination or inventing measurements.
7. **Runtime parity:** shared contracts across Tokio/Tokio, Tokio/Embassy, Embassy/Tokio and Embassy/Embassy. Use native persistence owners and coordinated clocks. Assert capability differences, isolate retained Embassy static fixture storage and compare byte traces within each fixed pairing.
8. **Generated campaigns:** typed actions and independent expectation ledger. Routine: six seeds × four pairings × four families = 96 cases, at most 32 actions. Extended: 256 seeds × four pairings = 1,024 cases, at most 128 actions. Replay twice; retain versioned JSON artifacts and standalone replay commands. Deterministically reduce failures while preserving prerequisites and retain originals. Register the extended lane with a one-hour timeout and isolate cases in fresh processes.

## Verification

For each milestone run focused simulator/owner regressions before broader checks. At completion run shared protocol, Tokio/Embassy host, headless tests/build, registered simulation and embedded persistence recovery, strict clippy, formatting/docs/registry checks and both generated corpora. Record exact commands, host, results and limitations in qualification notes. Existing tooling-layout hygiene failures are recorded separately.

## Progress

1. Complete: shared controlled persistence execution, real storage, four-node authority fixture and explicit graceful/abrupt lifecycle.
2. Complete: local/remote authority, publication/rollback/cancellation, exact capacity, lost replies and repeated restore, plus native journal byte-cut evidence.
3. Complete: admitted unary work, reserved/active watch revocation, same-link/stream-ID replacement behind a real Resource response lane, rapid regrant and lifecycle owner regressions.
4. Complete: two-controller response/identity isolation, exact router/watch/reader pressure, independent watch progress, deadlines and recovery.
5. Complete: production approval/rejection/expiry, invitation and identity failures, concurrent/replayed requests, lost completion, independent persistence cuts, pinned keys and stale boot work.
6. Complete: live page churn, configuration/peer refetch, coalesced invalidation, explicit sequence-gap workflow, reader overflow and reconnect.
7. Complete: all four runtime pairings with real FileStore/flash-journal owners, coordinated clocks, accurate capability differences and exact within-pair replay in isolated processes.
8. Complete: typed bounded corpora, independent ledger, two-run replay, versioned retained artifacts, standalone replay/reduction and registered routine/extended lanes. All 96 routine and 1,024 extended cases passed, two fresh fixtures each.

See [campaign qualification](measurements/remote-control-campaigns.md) for coverage boundaries, narrow production fixes, commands and final evidence. No hardware or other-platform result is implied.
