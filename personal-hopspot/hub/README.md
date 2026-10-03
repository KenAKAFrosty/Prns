# Hopspot Hub

Hopspot Hub is the desktop and mobile management application for embedded
Hopspots. This standalone workspace currently contains only its deterministic
Rust core. It is local development on `hopspot-hub`; review slices stay
uncommitted until approved.

## Core organization

`core/src/domain_primitives` owns validated labels, device identifiers, and
enrollment correlation values. It has no dependency on state machines.
`core/src/state_machines/device_registry` owns the registry's temporal state,
input/output protocol, transitions, and behavioral tests. Primitive validation
tests live with their primitive. Modules remain private behind the curated
exports in `lib.rs`.

New core work follows these ownership lanes. Add circuitry only when there are
concrete participants to compose. The standalone workspace owns the Rust and
Clippy baseline, and the core explicitly inherits it through `lints.workspace`.

## Device registry

`hopspot-hub-core` uses `no_std` and Pipecircuit 0.0.16 with explicit alloc-backed
storage. `DeviceRegistry` owns one bounded `WarpTable`, reserved at construction.
Device labels contain at most 128 UTF-8 bytes, must contain a non-whitespace
character, and preserve the supplied text. Labels may be duplicated.

Each operation has its own `StepInputOf<DeviceRegistry>` implementation and
exact outcome: `CreateDevice`, `RenameDevice`, `ForgetDevice`, `ReadDevice`,
`ListDevices`, `BeginEnrollment`, `CancelEnrollment`, `FailEnrollment`, and
`CompleteEnrollment`. Invoke them through `StateMachine::step`. Transport and
participant contracts belong to the later integration slice.

`ReadDevice` returns an owned `DeviceSnapshot` that survives subsequent changes.
Its outcome keeps the bounded snapshot inline, with a local Clippy expectation
for the variant size difference, to avoid allocating on each read.
`ListDevices::<CAPACITY>` returns IDs in a stack-backed bounded vector, or
`InsufficientCapacity` with the required count. It never returns a partial list.
List order follows table storage and is not a sorting contract. Ordinary steps
allocate no new heap storage. Duplicate target checks scan the bounded table.

A planned record has a stable local `DeviceId`. `BeginEnrollment`
consumes an existing Remote Control attempt ID and its expected target public
identity, then issues an `Enrollment` token. This slice starts at the identified
protocol attempt; discovery, user selection, and obtaining that attempt are
subsequent integration work. Device and enrollment IDs are scoped to one
registry lifetime and are not a persistence or cross-registry address format.

Completion requires the same token and target. The trusted integration must
supply it only after authenticated Remote Control enrollment and required
persistence succeed. These inputs are in-process contracts, not proof objects
suitable for accepting from an untrusted peer. The hub does not duplicate the
Remote Control cryptographic protocol or own private keys here.

Cancellation and failure return the record to planned. Each retry gets a fresh,
nonwrapping generation even when supplied the same protocol attempt ID. Stale
results cannot modify the new attempt. Different devices can pair independently;
a target already bound to a paired record cannot be bound to a second record.
A failed completion leaves its pending attempt intact for explicit settlement.
Failure outcomes preserve the semantic reason; future adapters own additional
transport diagnostics.

Forgetting removes only the local record and returns its previous enrollment
state, including any outstanding token. The caller remains responsible for
cancelling physical work. It neither revokes target permissions nor erases a
controller identity. Replacement of an existing pairing is deliberately refused
until an explicit replacement operation is designed.

## Verification

From this directory, run:

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path verification/kani/Cargo.toml --all -- --check
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo build --locked -p hopspot-hub-core --lib --no-default-features
cargo llvm-cov clean --workspace
cargo hub-coverage
cargo hub-mutants
cargo kani --manifest-path verification/kani/Cargo.toml --lib --output-format terse
```

The [Cargo aliases](.cargo/config.toml) follow Pipecircuit's
[protocol coverage alias](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.cargo/config.toml)
and [pre-push checks](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.githooks/pre-push).
The [mutation configuration](.cargo/mutants.toml) follows its
[cargo-mutants configuration](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.cargo/mutants.toml),
without the WebSocket-specific exclusions. No hooks are installed by this slice.

Coverage requires zero uncovered production functions, lines, or regions. Test
files, architecture/behavior snapshots, and proof harnesses are outside that
denominator; no production source is excluded. This is source coverage, not
exhaustive branch or application verification. Mutation candidates cover the
whole core except proof bodies. Require zero missed or timed-out mutants and
report caught and unviable counts separately.

Mutation testing runs in place so the existing PRNS path dependency resolves
normally. Run it without concurrent edits or checks; cargo-mutants restores each
source mutation. Its reports live in `target/hub-mutants/mutants.out`. Coverage
data remains in `target/llvm-cov-target`. Tool requirements are `cargo-llvm-cov`
with LLVM tools, `cargo-mutants`, and an installed Kani toolchain.

`architecture.rs` derives its machine graph from Rust source using
`pipecircuit-visualization`. `behavior.rs` inventories the compiled tests; it is
not itself evidence that they passed. To accept an intentionally reviewed
snapshot change in this nested workspace:

```sh
CARGO_WORKSPACE_DIR="$PWD" UPDATE_EXPECT=1 cargo test --lib remains_reviewable
```

Tests use PRNS's test-support feature to construct protocol attempt IDs; that
feature enables `std` in the test dependency graph. Production builds omit it.
Allocation-error and exhausted-ID translation are checked with synthetic storage
outcomes, without provoking host memory exhaustion or executing 2^64 inserts.
The registry advances enrollment through one borrowed row, keeping validation
and mutation under the same owner without an intervening fallible lookup.

The Kani package in `verification/kani` imports the actual label primitive source
and proves preservation/rejection for every two-byte ASCII input with unwinding
assertions enabled. The bound does not cover all Unicode or label lengths;
Unicode property tests complement it. Registry properties exercise arbitrary
cancel/retry histories and compare record creation/removal with an independent
map model. The isolated proof package avoids an existing `prns-core` Kani
compilation failure in its request-set proof (a `u32` shift by 32); the main core
crate's Kani lane is not claimed to pass.

Both workspaces retain lockfiles. No physical device or non-host platform is
qualified by these core checks.

## Next boundaries

Discovery requires user selection before pairing. Remembered devices will
reconnect automatically without replaying configuration commands. Headless
first enrollment will use an unowned boot window, direct-only admission,
code-free automatic approval, and owner authority. Interface status and power
controls follow; a change affecting the requesting controller's management
interface will need a device-owned confirmation deadline and rollback.

Per-installation controller identity comes first. Controller sharing, explicit
device replacement, firmware installation, clusters, and relationship views
remain later capabilities. None is represented as implemented by this registry.
