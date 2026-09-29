# One manual clock for frame and BLE media

macOS arm64, based on `7162b4bd1`, 2026-09-27.

## Driver extension

`ManualMedium::FramesAndBle` owns handles to one frame medium and one BLE lab.
Their initial ticks must match; construction refuses mismatched origins rather
than silently resynchronizing either medium. Every validation checks both media
against each other and the runtime clock. Advancing one or both externally is
an error, and no corrective advancement is attempted.

The next step is the earliest frame delivery, BLE advertisement, or supplied
runtime deadline. Both media reach that exact tick before actors can poll again.
Same-tick effects have an explicit order: BLE, frames, then runtime time. BLE
validates its bounded emission work before mutation; running it first ensures
budget refusal cannot prematurely deliver a frame. Once it succeeds, frame
advance can only fail on backwards time, already excluded by the common schedule.
This relies on the existing prohibition against concurrent medium mutation.

Reports preserve both complete per-medium outcomes. The clock owner has no new
per-node allocation or scan: its added scheduling work compares the two medium
schedules. Each medium retains its existing internal scheduling costs. This is
not an arbitrary collection-of-media scheduler or an implicit Tokio timer reader;
callers still supply known runtime deadline boundaries and explicit poll budgets.

Seven focused tests verify coincident frame/BLE/timer deadlines, earliest-event
selection from either medium, caller boundaries, nonzero origins, backwards
time, clock overflow, mismatched construction, and drift in either or both media.
The budget-refusal test has a frame due at the same future tick as too many BLE
emissions: both traces and the runtime snapshot must remain identical after
refusal. Reducing advertising work permits a successful retry of that step.

## Real-node bridge

A three-node manual-fleet case contains a frame-only endpoint, a transport owning
both a frame interface and a real BLE Auto supervisor, and a BLE-only endpoint.
The BLE supervisors use CoreBluetooth/BlueZ protocol profiles over virtual GATT
with 20-byte data values. These are not native operating-system Bluetooth stacks.
Two frame endpoints, two radios, one BLE connection and eight actor slots are
explicitly bounded. Crypto remains inline inside the real Tokio node actors.

Announcements must cross the bridge in both directions. Production route
introspection verifies two hops and the exact ingress interface at each endpoint.
Links in both directions carry exact 256-byte payloads, exercising real BLE
fragmentation as well as frame forwarding.

Isolating the frame segment leaves both cross-medium requests pending through
49 milliseconds and times them out at exactly 50. During the outage, a separate
link from the bridge to its BLE peer continues exchanging. Both medium clocks
remain equal through advertisement events and timer steps. Restoring frame
reachability permits both original crossing links and the BLE-local link to
exchange again without rebuilding any node or resetting the clock.

Orderly shutdown leaves no actors, frame deliveries or BLE connections. Both
frame attachments and both radio attachments detach exactly once. Neither trace
truncates, and there are no frame receive or BLE observation drops.

## Verification and limits

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --lib mixed_ --quiet`.
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet a_real_transport_bridges --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

Shipping behavior, wire formats and firmware memory budgets are unchanged.
This extends the host simulator, not firmware or physical-radio support. OS
entropy remains in production crypto and announcement jitter, so byte-for-byte
replay is still not claimed. No native networking, firmware build, Miri/ISA,
hardware smoke or benchmark run is claimed for this slice.
