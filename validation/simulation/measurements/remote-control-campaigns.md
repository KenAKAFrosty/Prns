# Remote Control simulation qualification

This completes the [eight-milestone roadmap](../remote-control-roadmap.md), building on the [durable authority evidence](remote-control-authority.md). Qualification ran on macOS arm64 with Rust/Cargo 1.98.0. These are host simulation results; physical radios, hardware flash, other operating systems and real-target firmware builds are separate qualifications.

## Coverage and ownership

The four-node Tokio fixture has an administrator, a differently granted operator and an outsider. Storage operations run against actual `FileStore` files through the shared controlled persistence driver. The fixture distinguishes graceful shutdown from abrupt actor removal and records explicit boot generations. Gates and scripted outcomes exercise admission before confirmed publication, uncertain publication, cancellation, rollback, failed rollback and repeated restores. Existing native journal tests supply byte-cut and compaction evidence rather than treating a simulated confirmation failure as a physical power cut.

The focused Remote Control target contains 44 tests. It covers silent admission rejection, capabilities separately from grants, payload bounds and verified handler identity; local and remote authority changes; protected administrators and exact table/router/watch/reader capacities; two-controller response isolation; production pairing approval, rejection, expiry, changed keys and independent durability; live pagination, coalesced invalidation, gaps, overflow and refetch.

The pending-watch race runs through whole nodes. A separate ordinary application endpoint returns an incompressible 1,024-byte Resource response. Delaying its actual Resource proof holds the target's response lane. A watch is reserved behind it, revoked and replaced after regrant using the same link and stream ID. The withdrawn request emits no response; its later cleanup cannot retire the replacement, whose resync and heartbeat are verified. This endpoint exists only in the fixture and does not change the 96-byte `AppMessage` limits. Native registry/runner tests additionally cover blocked writer cleanup, pending reservation identity, link closure and shutdown behavior.

Pairing admission tests identify real links and submit the production binary envelope. A claimed/identified identity mismatch, repeated begin and competing controller remain silent without replacing the original confirmation. A lost completion is replayed exactly, publishes authority once, and survives target reconstruction. Other tests use the normal controller API to persist both grants and pinned access, including two independent restores of each side. No response or timeout is treated as proof that a particular denial occurred.

Inventory is compared after quiescence with independently constructed interface and peer identifiers. Pages remain live during changes; there is no frozen pagination promise. Sequence-gap testing uses an explicit authorized test producer to inject sequence 3 after sequence 1, checks the typed gap, refetches inventory, then reconnects. It does not claim that losing one radio packet necessarily creates an application sequence gap on the reliable byte channel. Reader overflow is separately produced by 34 unread heartbeat intervals while another controller remains healthy.

## Runtime pairings and generated cases

The shared BLE fixture runs Tokio/Tokio, Tokio/Embassy, Embassy/Tokio and Embassy/Embassy controller/target pairings. Each node uses its real persistence owner: `FileStore` or `EmbeddedFlashPersistence` over a NOR image with actual flash-journal records. Snapshots derive from live runtime status, interface counts and the established Hopspot projection. Unavailable radio/rate readings stay unavailable.

Both runtimes share the medium's controlled clock. Persisted Embassy timebases can advance the local logical floor independently of the other node; controller pairing therefore uses the controller-local availability deadline, as the public API expects. Discovery advances one origin second before replacing a departed path because announce acceptance compares whole origin seconds. Deterministic event selectors are explicit fixture configuration; production fairness defaults are unchanged.

Embassy does not advertise a watch producer. Tokio targets do. Raw request exchanges verify the shared capability/admission contract across all four pairings, while typed Tokio watch readers additionally exercise their implemented controller behavior. Within each fixed pairing, fresh runs must match full packet bytes and native persistence traces. Different runtime pairings share semantic expectations, not an assertion of identical physical framing or storage bytes.

| Corpus | Cases | Actions per case | Fresh fixtures per case |
| --- | ---: | ---: | ---: |
| Routine: 6 seeds × 4 pairings × 4 families | 96 | 32 | 2 |
| Extended: 256 seeds × 4 pairings | 1,024 | 128 | 2 |

Families are authority, request/parser/app behavior, pairing and inventory/watch recovery. A typed action program drives an independent grant/boot/admission ledger. It predicts capability/grant intersection, verified identity, invocation count, silent rejection, recovery and runtime-specific support. Each lifecycle action includes its own setup/cleanup prerequisites. The launcher pins a copy of the simulator executable for each invocation so concurrent builds cannot replace it. Four bounded child workers run independent cases; every case has a fresh process, and its two fixtures are reconstructed independently. This also bounds retained Embassy static allocations to one case process.

Version 1 JSON artifacts retain the case, observations, byte traces, persistence traces and typed failure class/stage/action. Worker diagnostics are retained alongside them. Every invocation gets a new `run-NNNN` directory. On failure, deterministic chunk deletion followed by individual deletion preserves valid inputs and the same failure in both runs. Runtime failures must retain the same diagnostic; replay divergence has its own class. A focused reducer test proves that necessary preceding actions survive and original evidence remains unchanged. Standalone reduction first replays the original: if the failure has been fixed, it retains the old original and fresh baseline and refuses to present an obsolete failure as a newly reduced one.

## Bugs found and corrected

- Both Embassy topology drivers applied `merge` while combining wake deltas from an inline completion. That turned a conditional `AtMost` route ceiling into an exact deadline and could replace the earlier pairing route expiry with a seven-day deadline. They now compose deltas, preserving the existing schedule algebra. The all-four-pairing contract and native topology suites cover the correction.
- Restoring and later removing a route with an empty announce history could panic in the growable history owner. Histories are lazily materialized, so removal now materializes empty slots before moving the last row into the hole. A direct sparse-history regression and the reduced mixed-runtime restart case cover this correction.

The reducer also exposed mistaken fixture assumptions about administrative capability discovery, controller-local deadlines, announcement freshness and fair event selection. Those were corrected in the fixture/oracle without changing the production contract.

## Reproduction

Routine cases are part of the registered simulator suite. The extended lane is explicit, registered for scheduled/release runs with a one-hour timeout:

The extended lane uses the repository's `simulation` Cargo profile: optimization
level 2 with debug assertions and overflow checks enabled. This is the same
profile as the extended core-work campaign. It retains all 1,024 cases, 128
actions per case, four isolated workers, and two matching fresh-fixture traces
per case. The release run on `47065befee08a5e0d62fe6f55a50aa3cf1cdc62c`
used the unoptimized test profile and reached 832 qualified cases before its
3,600-second limit. That run is incomplete qualification, not a passing result.

A macOS arm64 comparison replayed seed 42 across all four runtime pairings
under both profiles, alternating execution order. Complete semantic artifacts
matched in every pairing, including both fresh fixtures' packet and persistence
traces. Excluding compilation, the unoptimized runs took 17.641 seconds and the
optimized runs took 12.415 seconds (1.421× faster). The full optimized campaign
was running concurrently, so this is a comparative sample, not an isolated
throughput measurement or a substitute for completing the whole campaign.

```console
python3 validation/run.py run --suite virtual-device-simulation --suite embedded-persistence-recovery --suite registry
python3 validation/run.py run --suite remote-control-simulation-extended
./tools/prns repo.simulation.remote-control.replay --case PATH/TO/case.json
./tools/prns repo.simulation.remote-control.reduce --case PATH/TO/original.json
```

Artifacts live under `validation-artifacts/results/remote-control-simulation/{routine,extended,replay,reduction}/run-NNNN/`. Campaign failure messages name the original, reduced artifact and exact replay command. These artifacts are local validation outputs, not checked-in source.

Owner and product checks:

```console
cargo test --locked -p prns-core -p prns-runtime --lib
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib
cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib
cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml --features wifi-halow,websocket,wifi-auto
cargo build --locked --manifest-path personal-hopspot/headless/Cargo.toml --features wifi-halow,websocket,wifi-auto
cargo clippy --locked -p prns-simulation --features controlled-time --test embassy_ble --test remote_control -- -D warnings
cargo clippy --locked -p prns-core -p prns-runtime --lib -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --tests -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib --tests -- -D warnings
```

The routine corpus passed all 96 cases and the extended corpus passed all 1,024 cases in 1,385.04 seconds, each with two matching fresh-fixture traces. Extended artifacts are in `extended/run-0001`; the routine artifacts from the final registered run are in `routine/run-0003`. The final executable-pinning change was additionally checked with all 96 routine cases in 31.90 seconds (`routine/run-0004`) and standalone replay; it does not change generated actions or scenario execution.

The final registered simulator suite passed 417 Rust tests with seven explicitly ignored, including the 44 focused Remote Control tests. Core/shared-runtime checks passed 2,093 and 85 tests respectively; Tokio passed 306 with one ignored; Embassy passed 174. The registered embedded persistence slice passed 59. All 11 headless tests and its feature-enabled host build passed. Headless lifecycle tests require loopback listener access, which the initial sandboxed run lacked; the permitted rerun passed. Strict checks above and Rust formatting passed.

`validation/run.py` registry verification passed. `tools/prns verify`, the corresponding two tooling registry assertions, and `fmt-docs.sh` still fail on three existing `personal-hopspot/headless/scripts/network-lab` implementations outside the tooling control plane. This qualification does not move those unrelated scripts or include the independent website edits.

Streaming remains the next product slice: reuse controller identity and permissions, then design delivery and flow control separately from bounded request/response.
