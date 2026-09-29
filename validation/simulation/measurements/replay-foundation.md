# Replay foundation: explicit fixture timeline and normalized observations

macOS arm64, based on `ceed8eeba`, 2026-09-27.

The [roadmap and input audit](../replay-roadmap.md) records the three slices,
source owners, transcript contract and limitations. The existing production
timeline API was sufficient; this round adds no shipping runtime API, entropy
override, feature flag or allocation. All manual-fleet boots now select the
same explicit epoch plus coordinated runtime elapsed time instead of retaining
the default wall-time origin.

Two tests compare actual real-node results: three independent boot/link/echo/
restart/echo runs must produce equal normalized transcripts, while two runs
with distinct equal-length response values must differ. The four-node fixture
retains eight actor slots and a 4,096-event trace. It rejects trace loss and
checks complete attachment cleanup after observation. This is neither full
packet replay nor evidence of deterministic worker scheduling or OS entropy.

## Verification

Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet replay --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
git diff --check
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
```

Both focused replay tests and all 86 manual-fleet tests passed. All-target
clippy, root/workspace tests, repository formatting/docs checks, tool/validation
registry verification, website tests and `git diff --check` passed. The
registered simulation suite passed in 125.189 seconds, including build-lock
contention; this is not a performance comparison. Existing ignored tests remain
ignored. Firmware, hardware and other-host lanes were not rerun for these
test/documentation-only changes.
