# Remote Control durable authority qualification

The implementation follows [the roadmap](../remote-control-roadmap.md). The baseline packet campaign remains in [Remote Control qualification](remote-control.md).

## Controlled I/O foundation

Tokio persistence now accepts an explicit `PersistenceIo` driver. Native construction continues to execute filesystem jobs on the blocking pool. The recipe-managed worker and authorization transactions use the same driver; flush revisions and authorization ownership still belong to their existing shared owner. Execution labels distinguish authorization begin/store/confirm/finish, ordinary flush revision/commit and vault work. The driver does not decide grants, activation, rollback or retry timing.

The four-node fixture adds a separately granted operator beside the administrator and unlisted controller. Each scenario owns isolated real `FileStore` storage. The controlled driver executes I/O within the manual actor, can gate selected operation completion and supplies typed scripted write/confirmation outcomes. Node reconstruction preserves fixture keys and changes boot entropy explicitly. Initial authority is provisioned with the existing snapshot/flush API before the scenario begins.

Three whole-node cases cover gated writes followed by two restores, published candidates with failed/missing/different confirmation followed by recovery, canceled callers after publication, and failed candidate writes followed by durable rollback. Whole typed grant tables, packet transcripts and I/O observations are compared. The published-unconfirmed case deliberately writes a real durable file then withholds confirmation; it tests owner behavior under uncertainty, not a filesystem crash mechanism.

On macOS arm64 the 20-test Remote Control target passed; the native Tokio library passed 306 tests with one ignored. Focused native persistence tests passed 20. Strict Tokio library/test and simulator Remote Control clippy passed. Commands:

```console
cargo test --locked -p prns-simulation --features controlled-time --test remote_control --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib persistence --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --quiet
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --tests -- -D warnings
cargo clippy --locked -p prns-simulation --features controlled-time --test remote_control -- -D warnings
```

At this foundation checkpoint, the remaining roadmap milestones were not yet qualified. The completed evidence is recorded in [campaign qualification](remote-control-campaigns.md); no hardware or other-platform result is implied.

## Management, revocation and pressure

The target now passes 31 tests. Added full-node cases exercise remote revoke/regrant and capability changes under three replayed actor seeds, protected administrator identities, exact capacity exhaustion, unknown revocation, interrupted boots at authorization begin/store/confirm/finish, lost committed-success responses, and terminal rollback failure followed by two successful restores. Failed rollback produces the existing typed node failure; it does not continue serving uncertain authority.

Revocation preserves the agreed admission boundary: an app operation already admitted completes, while later work produces neither a response packet nor another handler invocation. Removing only watch permission ends the watch while inspection remains usable. Regrant and reconnect recover sequence 1 and capacity. An ended stream retains its reader registration until link closure; these cases do not invent same-link unsubscription. An actual full node awaiting gated authorization storage continues watch heartbeats, then resumes ordinary requests after settlement.

Two trusted identities saturate 256 active handlers and 1,024 queued requests. Two additional requests expire without executing. Every successful response is compared with its originating actor's complete payload and verified handler identity, then both controllers successfully send fresh requests. A separate 34-heartbeat case overflows one unread 32-chunk reader while another controller continues receiving; the failed reader reports the typed overflow source and reconnects successfully. `personal-rns` now reexports that already-public Tokio failure type so facade consumers can inspect it without string matching.

Pressure cases explicitly use larger fixture actor/frame/trace budgets. The long heartbeat case initially exhausted the baseline trace budget; the corrected test retains the whole trace rather than accepting eviction. No production capacity, quota, wire protocol or scheduling policy changed. Pending watch-response-lane races and combined lifecycle permutations were still open at this checkpoint; the completed whole-node case is documented in [campaign qualification](remote-control-campaigns.md).

The 31-test target and its strict clippy command passed on macOS arm64. The previous native library/persistence results still apply because this slice changes tests and a type reexport only; those libraries were not retested after the reexport.

## Pairing and convergence

The Tokio target suite now has 39 passing tests. Production pairing approval persists both target grants and controller pins, with two independent restores of each side. Invalid invitation proofs stay silent; controller/target rejection and expiry do not publish trust. Storage cuts before publication and at transaction finish restore each owner's surviving trust separately. Replaced target/controller keys do not inherit pinned access/grants. Old app futures are destroyed on target reconstruction; fresh traffic uses a closed/reopened control link and a bounded completion horizon.

Inventory qualification changes interfaces and peers between pages, then refetches from `First` after quiescence against independently constructed identifiers. Watch polling coalesces intermediate changes into invalidation, and reconnect starts a new resync sequence. The simulator's config decoration reports its actual connection state; radio measurements remain unavailable. There is no frozen pagination guarantee.

Graceful fixture teardown drives up to 2.5 seconds of controlled time: the native route-save debounce may delay a worker by two seconds. Abrupt reconstruction remains a separate actor cancellation operation.
