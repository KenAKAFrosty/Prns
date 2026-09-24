# Deterministic Remote Control qualification

The `remote_control` integration target runs three production Tokio nodes: a
target, its explicitly granted controller, and an unlisted controller. The
fixture supplies controller/target secrets, engine entropy, handle/interface
entropy, path identifiers, the boot epoch, outer actor scheduling and a 1 ms
manual clock. Crypto executes inline. Cold connection setup requests a path;
an identity already learned from that discovery uses the existing cache.
There is no automatic announcement policy.

The target uses the established Hopspot inventory projection against real
runtime snapshots. Inert fixture interfaces and a HaLoW-family fixture fleet
supply controlled status through the normal interface/supervisor APIs. These
are simulated status sources, not radios or hardware measurements. Peer pages
preserve unavailable rate and radio measurements. Application state is `()`;
Remote Control handlers are installed independently of it.

## Contracts exercised

The 17 tests cover these behaviors:

- Unidentified links, unlisted identities, absent grants and missing request
  permissions produce no Response packet and expire at the explicit 50 ms
  request deadline. The app handler is not reached. Identifying an existing
  link then enables the granted controller's requests.
- Discovery and execution intersect installed capabilities with grants. A
  configured host operation without an executor stays absent and silent.
- Authenticated malformed requests return typed protocol errors; app rejection
  returns an app operation error. Verified controller identities reach the
  handler. The typed controller API exchanges every payload length from zero
  through 96 bytes; a 97-byte request never reaches the app handler.
- Real runtime interface and peer inventories traverse three pages in ID order,
  preserve unavailable measurements, and stop exposing a removed supervisor.
- Six simultaneous permitted and six unpermitted requests remain attached to
  their own identity and waiter. The case repeats under three scheduling seeds.
- Watches start with sequence 1, heartbeat after five seconds, invalidate on
  interface addition/removal, refuse the ninth subscription and duplicate
  admission, end on link closure and restart at sequence 1 on a fresh link.
- Local cancellation drops the waiter while an already transmitted request can
  still execute. Its eventual response cannot settle a different request.

The generated request/response campaign selects six fixed scheduling seeds,
two packet boundaries and four faults: loss, 10 ms delay, duplication with a
second copy after 10 ms, and arrival after the request deadline. That is 48
cases, each run twice, plus a clean baseline per seed. The watch campaign adds
18 cases of initial channel-frame loss, delay or duplication, also run twice.
It checks channel retries and exactly one initial event followed by sequence 2.
Each fault case proves subsequent traffic succeeds and compares whole
observations and complete medium traces, including packet bytes and shutdown.

Fixture limits are explicit: three medium endpoints, 64 receive frames, 256
pending deliveries, 20 actors, 8,192 polls per quiescence step, 128 app
invocations, and 32,768 retained trace events. Trace eviction, queue-capacity
loss, leaked actors, pending deliveries and unmatched endpoint teardown fail
assertions. Failing cases are not retried. These finite replay checks do not
prove all schedules, topologies or internal runtime interleavings deterministic.

## Finding and runtime correction

The first watch experiment failed the manual driver's no-spawn invariant with
`SpawnedTasks { count: 1 }`. The Tokio producer previously used a spawned task
per watch. Watch futures now belong to a bounded registry, with an owned watch
driver polled alongside request handling. A request router awaiting an
operation cannot prevent watch timers or cleanup from progressing. Node
shutdown drops the futures synchronously instead of relying on task aborts.

Reservations still happen synchronously at admission. Retiring watches retain
capacity until completion, and completion checks the same lease before removing
its entry. Grant and link cancellation rules, initial-response ordering,
write/cleanup deadlines and wire formats are preserved. A panicking status
callback closes its control link and releases the lease without unwinding the
request router. Focused host tests cover these properties and heartbeat delivery
while request handling remains blocked.

## Boundary to carry forward

This fixture deliberately configures `NoPersistence`. Live grant changes correctly
return `Unavailable` rather than silently becoming ephemeral. Tokio's durable
authorization adapter currently uses blocking filesystem workers, which the
manual driver cannot schedule. This campaign therefore tests explicit initial
grant configurations and refusal of nondurable mutations; it does not claim
end-to-end deterministic live revocation or rollback.

The next slice should expose controlled storage completion at the durable
transaction owner, then combine real grant mutation, watch cancellation,
write/confirmation failure, rollback and reboot with the existing journal
power-loss cases. Keep admission silent and durable grants mandatory. Full-node
Embassy/mixed Remote Control campaigns and physical G4/Heltec qualification
remain separate work. This is not radio-performance, large-fleet or arbitrary
failure-sequence evidence.

## Verification

On macOS arm64, run:

```console
cargo test --locked -p prns-simulation --features controlled-time --test remote_control --quiet
cargo test --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --quiet
cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib --quiet
cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml --quiet
cargo test --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo clippy --locked --manifest-path prns-runtime/impls/tokio/Cargo.toml --lib --tests -- -D warnings
```

The registered simulation suite automatically discovers this target. Its input
fingerprint also includes `personal-hopspot/core`, the inventory projection
owner. There are no changes within the configured mutation-analysis surface.

The final Remote Control target passed 17 tests. Tokio host tests passed 306 with
one ignored; Embassy host tests passed 174; headless tests passed six, and its
native build succeeded. The root default tests and both strict clippy commands
passed. The registered simulation suite passed in 79.494 seconds, including
build-lock waiting and compilation; this is not a runtime performance result.
Its structured evidence is in
`validation-artifacts/results/virtual-device-simulation/result.json`.

Additional verification commands:

```console
cargo build --locked --manifest-path personal-hopspot/headless/Cargo.toml --quiet
cargo fmt --all -- --check
cargo fmt --manifest-path prns-runtime/impls/tokio/Cargo.toml -- --check
cargo doc --locked -p personal-rns --no-deps --document-private-items --quiet
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

These passed. `bash validation/hygiene/fmt-docs.sh` stops at its tooling gate,
and `./tools/prns verify` reports the same existing layout failure: committed
`personal-hopspot/headless/scripts/network-lab/{lease.sh,prepare.py,radio.sh}`
remain outside the required tooling implementation directory. That failure
predates this change. The independent formatting and Rust documentation
commands above passed; the full hygiene gate did not. No physical hardware,
other host operating systems, target ISA execution or full PR lane was run.
