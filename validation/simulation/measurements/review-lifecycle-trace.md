# Review repairs: teardown, trace inspection and layout assertions

macOS arm64, based on `97873956a`, 2026-09-27.

Whole-runner destruction now retires all actor wakes before dropping futures in
the private runtime context, matching explicit cancellation. A focused regression
checks two pending actors, destructor clock observations and stale wakes after
destruction. This corrects host simulation lifecycle behavior; it does not change
shipping runtimes. Destructors retain the existing nonblocking/no-spawn contract.

Frame-medium `inspect_trace` provides a borrowed view while holding the medium
lock. Its callback must not reenter the medium, poll actors or block. The restart
boundary observers now inspect that view rather than cloning the entire retained
packet history on every poll. Tests check wrapped order, truncation counts,
suffix/reverse iteration and identical underlying frame addresses. The owning
snapshot API remains available. No benchmark or measured speedup is claimed.

The fixed-link layout test derives its minimum saving from this target's type
sizes, allowing aggregate alignment slack. The earlier 224-byte result remains
an actual Nordic checkpoint measurement, not a universal ABI requirement. No
shipping storage representation changes in this repair.

Verification uses host Cargo with `CARGO_INCREMENTAL=0`: focused cancellation,
trace-view, fixed-layout and manual-fleet tests; core/simulation all-target
clippy; root and workspace tests; the registered `virtual-device-simulation`
suite; formatting and diff checks. This does not claim a new i686, firmware,
Miri/ISA or hardware run.
