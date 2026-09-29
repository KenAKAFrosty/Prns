# Routed transport restart and lost forwarding state

macOS arm64, based on `bf90d25ea`, 2026-09-27.

## Scenario

Two `manual_fleet` cases run six real Tokio nodes: four endpoint leaves and two
forwarding transports. Two leaves attach to each transport, and a bridge joins
the transports. Each of the five segments has two virtual interfaces, each
allowing exactly one neighbor; there is no direct cross-side delivery path.

One case restarts the left transport twice, the other restarts the right twice.
The selected actor is cancelled rather than gracefully stopped. Its three
interfaces detach, and its engine and volatile forwarding state are discarded.
The replacement uses fresh actor/medium endpoint IDs with the same configured
transport identity and logical interface IDs. All five other nodes stay live.

The medium has ten live endpoint slots, 32 receive entries per endpoint, ten
pending-delivery slots and a 65,536-event trace. The runner has twelve actor
slots, enough for six nodes and six simultaneous route observers. Existing
inline crypto keeps production node work inside the manually polled runtime.

Each case checks:

- Before restart, all four leaf destinations have the expected route hop counts
  and ingress interfaces at every other node. Links cross both transports in
  both directions and exchange exact payloads.
- Cancellation leaves five node actors without advancing coordinated time.
  Requests on both crossing links remain pending through 49 milliseconds and
  time out exactly at 50. A same-side pair behind the unaffected transport
  continues to exchange on its original link during that interval.
- The replacement starts with no routes to any leaf. Its three interface IDs
  are unchanged, but every attachment ID is fresh. A second cancellation of
  the old actor returns `NotLive` and cannot affect the replacement.
- After explicit rewiring and leaf announcements, the new transport hears all
  four destinations and the full route inventory converges. Directly attached
  destinations are one hop from a transport, remote leaves are two, same-side
  leaf pairs are two, and opposite-side leaf pairs are three.
- Fresh crossing links have different IDs and return exact payloads in both
  directions. Reissuing requests on the obsolete links still times out exactly
  at 50 milliseconds, even after routes and fresh traffic have recovered.
  The unaffected pair continues using its original link throughout.
- Repeating this cycle reuses the bounded actor/endpoint capacity. Final orderly
  shutdown leaves zero actors and pending deliveries; all sixteen attachment
  IDs have matching detachments, with no receive drops or trace truncation.

## Interpretation and limits

The existing routed partition scenario restores a connection while keeping
both forwarding engines alive, so their old links recover. These cases exercise
a different failure: replacing an engine loses its link-forwarding mappings.
Destination rediscovery is sufficient for fresh links, not resurrection of the
old mappings. Applications still need to establish replacement links; these
tests do not add automatic retries or promise transparent session continuity.

Discovery is explicitly triggered by announcements and bounded to ten seconds
of simulated time with one-millisecond steps. Route assertions inspect actual
production inventories rather than assuming historical announcement callbacks
prove convergence. Surviving routes need not have been replaced if their hop
counts and ingress interfaces remain correct. Production announcement jitter
and crypto still use OS entropy; byte-for-byte replay is not claimed.

No production behavior, wire format or firmware memory budget changes. These
are host Tokio nodes over the virtual frame medium, not native Bluetooth/Wi-Fi
stacks, Embassy firmware or CPU emulators. Cancellation runs Rust destructors;
physical power loss, persistent identity provisioning, flash recovery, alternate
paths, large routed fleet limits and automatic retry remain separate work.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet transport_restart --quiet` (two cases).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (nine cases).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware, Miri/ISA, native networking, hardware smoke or benchmark run is
claimed for this test-only slice.
