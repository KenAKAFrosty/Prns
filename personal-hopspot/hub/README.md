# Hopspot Hub

Hopspot Hub is the desktop and mobile management application for embedded
Hopspots. This standalone workspace currently contains only its deterministic
Rust core. It is local development on `hopspot-hub`; review slices stay
uncommitted until approved.

## Core organization

`core/src/domain_primitives` owns validated labels, device identifiers, and
enrollment, connection, interface request correlation values, and pairing candidate provenance. It has no dependency on state
machines.
`core/src/state_machines/device_registry` owns the registry's temporal state,
input/output protocol, transitions, and behavioral tests. Primitive validation
tests live with their primitive. Modules remain private behind the curated
exports in `lib.rs`. The `state_machines/interface_inventory` owner collects
interface pages for one connection and publishes complete results.
`state_machines/usb_pairing_discovery` owns the bounded set of USB pairing
candidates and its explicit clock.

New core work follows these ownership lanes. Add circuitry only when there are
concrete participants to compose. The standalone workspace owns the Rust and
Clippy baseline, and the core explicitly inherits it through `lints.workspace`.

Machine steps use `Result<Outcome, Error>` only for invariant failure paths.
Their ordinary outcomes remain flat inside `Ok`; steps with no invariant
failure path return their outcome directly. In particular, configured capacity
limits, missing records, stale callbacks, and peer-response failures are domain
outcomes, not invariant errors.

`CreateDevice`, `BeginEnrollment`, `BeginConnection`, and `RefreshInterfaces`
return step-specific errors for exhausted identifiers. `AdvanceUsbPairingDiscovery`
returns an error for backward time, and `ObserveUsbPairingAvailability` returns
an error when the observation is ahead of the discovery clock. These errors
preserve existing state and retain the rejected input or device context where
the operation previously returned it. Architecture snapshots show the `Ok` and
`Err` branches separately.

## Device registry

`hopspot-hub-core` uses `no_std` and Pipecircuit 0.0.16 with explicit alloc-backed
storage. `DeviceRegistry` owns one bounded `WarpTable`, reserved at construction.
Device labels contain at most 128 UTF-8 bytes, must contain a non-whitespace
character, and preserve the supplied text. Labels may be duplicated.

Each operation has its own `StepInputOf<DeviceRegistry>` implementation and
exact outcome: `CreateDevice`, `RenameDevice`, `ForgetDevice`, `ReadDevice`,
`ListDevices`, `BeginEnrollment`, `CancelEnrollment`, `FailEnrollment`,
`CompleteEnrollment`, `BeginConnection`, `ConfirmConnection`, and `EndConnection`.
Invoke them through `StateMachine::step`. Transport and participant contracts
belong to the later integration slice.

`ReadDevice` returns an owned `DeviceSnapshot` that survives subsequent changes.
Read and forget outcomes keep bounded state inline, with local Clippy
expectations for variant size differences, to avoid allocating on these steps.
The creation error likewise preserves the rejected bounded label inline, with a
local `result_large_err` expectation so exhaustion requires no new allocation.
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
state and connection state, including outstanding tokens and any live link ID.
The caller remains responsible for cancelling physical work and closing links.
It neither revokes target permissions nor erases a controller identity.
Replacement of an existing pairing is deliberately refused until an explicit
replacement operation is designed.

## Remembered-device connections

`BeginConnection` only accepts paired records. Its `Connect` outcome gives the
transport adapter an opaque `Connection` token carrying the local device ID,
fresh nonwrapping generation, and expected target identity. Duplicate begins
report the active attempt or session without starting another connection.
Connection and enrollment generations are independent.

The trusted adapter supplies `ConfirmConnection` only after PRNS authenticates
the expected target on the reported `LinkId`. The registry checks the attempt
and identity before publishing `Connected`. These in-process values do not
constitute authentication proof. Wrong-target confirmation leaves the pending
attempt intact so the adapter can explicitly settle it.

`EndConnection` settles either an attempt or a live session, preserving its
reason and returning the live link when one exists. Snapshots distinguish
`NotConnected`, `Connecting`, `Connected`, and `Disconnected`. Cancellation,
timeout, transport loss, and authentication failure leave remembered pairing
intact. Ending or forgetting a record invalidates subsequent callbacks; an
old callback cannot settle a newer connection, even if the link ID is reused.
Tokens are scoped to their originating registry lifetime.

This slice owns connection bookkeeping and emits a connection intent. The
network adapter, deadlines, and automatic reconnect scheduling while active
remain integration work. No configuration is queued or
replayed by these steps.

## Interface inventory

Create one `InterfaceInventory::<CAPACITY>` for each confirmed connection. The
caller selects its interface limit explicitly; staging and published data use
separate bounded buffers with no heap allocation. The collector is subordinate
to the registry's connection lifecycle. The future circuit must route
`CloseInterfaceInventory` when that connection ends or its device is forgotten,
and construct a new collector for a newly confirmed connection. It does not
observe registry changes automatically.

`RefreshInterfaces` issues an opaque `InterfacePageRequest`. Its `request()`
returns PRNS's `RemoteControlRequest::InventoryInterfaces`; `connection()`
identifies the connection on which the adapter must send it. The adapter routes
the decoded `RemoteControlInterfaceInventory` back with that exact token.
Every continuation returns the next token and cursor. Duplicate begins report
`Busy`; stale pages or failures cannot alter the pending refresh. Tokens belong
to one collector lifetime; do not reuse an old collector's callbacks if a
collector is recreated for the same connection.

`ReceiveInterfacePage` checks ascending interface IDs across pages and the total
capacity. It publishes only after the final page, preserving upstream interface
kinds, modes, link status, enabled state, and counters. An empty final inventory
replaces earlier data. Duplicate or backwards IDs and capacity overflow fail the
refresh; partial data is discarded. This is complete pagination, not a claim
that all remote interfaces were sampled at the same instant.

`ReadInterfaces` returns owned data plus `NotRequested`, `Receiving`, `Ready`,
`Failed`, or `Closed` status. During refresh or failure, any published entries
are the last complete result and must be displayed as such. `InterfaceRefreshFailed`
settles transport, deadline, permission, or decoding failures. Closing returns
any pending request for cancellation, clears all data, and permanently refuses
reuse. A stale-page outcome returns the rejected page to its caller.

Network dispatch, timed polling, interface names/configuration, live watches,
and interface power changes remain follow-up work.

## USB pairing discovery

USB Auto is the first live transport target. PRNS already owns USB scanning,
handshaking, lanes, and reconnects through
[`UsbAutoHost`](../../prns-interfaces/impls/tokio/src/usb_auto/mod.rs) and its
[native attachment](../../prns-interfaces/impls/tokio/src/usb_auto/host.rs).
The existing [desktop runtime](../desktop/src/desktop/runtime.rs) demonstrates
that attachment. Hub does not introduce another serial protocol.

`UsbPairingDiscovery::<CAPACITY>::new(interface, now)` requires the actual USB
Auto interface ID and the runtime's monotonic time. Its five operations are
`ObserveUsbPairingAvailability`, `AdvanceUsbPairingDiscovery`,
`ReadUsbPairingCandidates`, `SelectUsbPairingCandidate`, and
`ClearUsbPairingCandidates`. All use `StateMachine::step`.

Observation accepts PRNS's `RemoteControlPairingAvailabilityObservation`, which
the engine emits after signature verification and direct-hop admission. Hub
admits only observations from the configured USB interface. It rejects arrivals
later than its clock, already expired observations, and older or equal-time
updates for the same endpoint. Newer observations replace that endpoint's
provenance and expiry, even when the list is full. Other endpoints are refused
when capacity is reached; no existing candidate is evicted.

The runtime must advance discovery to its current time before processing an
observation, rendering a list, or handling selection. Advancement expires
candidates at their deadline and refuses backward time. Snapshots include
`as_of` so their freshness is explicit. Owned snapshots survive later updates
and clearing. Clearing removes the list without changing its clock or interface;
the runtime must stop or drain old events when resetting the discovery session.

Selection rechecks membership and returns the current candidate, preserving its
endpoint, source interface, observation time, and expiry. It does not consume
the candidate, record enrollment, reserve an attempt, or send a packet. The
future circuit will handle this explicit user intent and the registry's attempt
lifecycle. Availability alone never initiates pairing. Presence in this list
means pairing was advertised within its lifetime; it does not guarantee the
device is still plugged in or that pairing will succeed.

The pairing endpoint is ephemeral and is not the remembered authenticated
target identity. Raw USB locators and announcement app data are not interpreted
as device identity or labels. Existing PRNS pairing requires an invitation code;
the planned code-free headless enrollment requires separate protocol work.

This slice implements the deterministic discovery owner. Attaching USB Auto to
a Hub runtime, routing its events, identity persistence, pairing initiation,
authenticated connection, and automatic inventory dispatch remain integration
work. It has not opened a physical USB device.

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
map model. Connection properties exercise arbitrary connect/confirm/end histories,
reject all retired tokens, and check another device remains unchanged. Maximum
generation is covered by a deterministic exhaustion test. These stateful checks
use proptest because importing the full PRNS graph still blocks Kani.
Interface inventory properties vary sorted interface sets and page boundaries,
roundtrip real PRNS request/response bytes, and compare the complete published
result with the original entries. Deterministic tests cover invalid ordering,
overflow, failure recovery, exhaustion, and reconnect invalidation.
USB discovery fixtures construct signed availability packets, run PRNS's
ingress and deferred signature verification, and feed genuine observations to
the machine. Its property test compares arbitrary arrival, expiry, selection,
and clear histories with an independent bounded model. The same PRNS dependency
restriction makes property tests the practical lane for this machine; no new
Kani result is claimed for discovery.
The isolated proof package avoids an existing `prns-core` Kani
compilation failure in its request-set proof (a `u32` shift by 32); the main core
crate's Kani lane is not claimed to pass.

Both workspaces retain lockfiles. No physical device or non-host platform is
qualified by these core checks.

## Next boundaries

USB discovery now exposes an explicit selection step; runtime pairing dispatch
still requires that user intent. Remembered devices will
reconnect automatically without replaying configuration commands. Headless
first enrollment will use an unowned boot window, direct-only admission,
code-free automatic approval, and owner authority. Network-driven interface
status and power controls follow; a change affecting the requesting controller's management
interface will need a device-owned confirmation deadline and rollback.

Per-installation controller identity comes first. Controller sharing, explicit
device replacement, firmware installation, clusters, and relationship views
remain later capabilities. None is represented as implemented by this registry.
