# Live-link response expiry and assembly reclamation

Local macOS arm64 evidence, 2026-09-26, candidate based on `919c0a187`.

## Detection and correction

The preceding [buffered interruption tests](buffered-resource-interruption.md)
recovered only after old-link retirement. This slice keeps BLE connected and
discards only advertisements for the selected response link after the first
advertisement. Other frames, including keepalives, pass normally. The first
verified segment has already created an assembly; continuation retries cannot
reach the requester. Its ordinary buffered API eventually returns `Timeout`.

Before the correction, the next three-segment request on another Reticulum link
failed rather than delivering the full file. A negative control disabling only
the cleanup reproduced `ResponseTransferFailed(TransferCorrupt)` in all four
independent runtime/endpoint cases. Both links share the receiver's one
assembly slot. Retrying only on the original link would hide the retained row:
starting a chain there replaces that link's assembly. The alternate-link probe
exposes the unavailable slot without reconnecting, recreating a node, or directly
inspecting its private state.

Shared-core receipt expiry now removes an assembly only when both its link and
`AssemblyCorrelation::Response(request_id)` match the expired request. The
receipt already stores the packet hash; `ExpiredReceipt` now carries that hash
to settlement so the request ID survives removal of the receipt row. This adds
no persistent table column, buffer or capacity. It does enlarge the temporary
expiry result; it is not a claim of identical compiled stack/code size.

## Regression coverage

Four independent mixed-runtime cases cover Embassy and Tokio requesters with
ESP32/Apple and nRF52/BlueZ simulated endpoints. Each requires:

- The buffered request returns exactly `SendError::Failed(Timeout)`.
- At least one matching advertisement is discarded within an explicit loss
  budget, and the rule is removed after settlement.
- BLE remains connected; neither node journals a Reticulum link closure.
- A different link on the same nodes delivers exactly three segments, the full
  literal 1,200-byte file, and one successful terminal result with measured RTT.
- Original links remain usable for raw and repeated buffered requests, including
  zero-budget refusal and complete success. Traces and radio cleanup remain bounded.
- The Embassy responder settles once with Resource timeout in the dropped-response
  case and once successfully for the alternate-link recovery request.

A no-allocation core-engine ownership matrix checks before/at/after expiry,
exactly one settlement, matching cleanup, and preservation of absent, unrelated,
request-body, unsolicited, and other-link assembly state. Preserved assemblies
still complete with the correct byte total. Existing receipt tests now compare
the complete expired value including its packet hash.

The integration-only wire gate gains a header-scoped loss rule with a caller-set
maximum drop count. Exceeding it fails the test rather than silently forwarding.
Tests cover first-frame pass-through, repeated loss, unrelated links, keepalives,
malformed frames, disarming/rearming and no paused-clock advancement. Frames are
discarded before the virtual sink; this models selective frame loss, not a native
GATT failure. No payload copies, growing queue, public simulator API or shipping
BLE behavior is added.

## Verification

Host Cargo commands used `CARGO_INCREMENTAL=0`; the canonical firmware build did
not override its environment. Passed:

- `cargo test --locked -p prns-core receipt_expiry --quiet`.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet`
  (38 tests).
- Ten final-candidate repeats of that command filtered to `stalled`: all forty
  regression executions passed after restoring the cleanup.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy`.
- `python3 validation/run.py run --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`.
- `./tools/prns build embedded resources report --target t114`: static sections
  142,980 bytes plus four bytes padding; runtime reservation 69,632 bytes; RAM
  headroom 376 bytes. Stack evidence leaves 10,352 bytes in that reservation,
  with the report's unresolved-call/frame gaps still applicable. These values
  match the preceding resource evidence. Flash headroom is 145,388 bytes,
  forty fewer than the preceding report. This is one firmware profile, not an
  all-board build or a complete worst-case stack proof.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify` and
  `python3 validation/run.py verify`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`,
  root formatting, whitespace and touched-Markdown relative-link checks.

Focused mutation audit:

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config --in-place -p prns-core \
  --file prns-core/src/engine/settlement.rs --re timeout_settlement \
  --timeout 60 --build-timeout 180 \
  --output validation-artifacts/mutation/live-link-expiry -- --lib receipt_expiry
```

Baseline passed. The inverted ownership comparison was caught; replacing the
settlement with `Default::default()` was unviable because `Settlement` has no
`Default`. No survivors or timeouts. This focused audit supplements the explicit
cleanup-removal negative control; it is not a full mutation-surface audit.

`./tools/prns doctor embedded-assurance` still reports less than the required
24 GiB free disk and missing Renode. `./tools/prns build embedded resources contracts --check`
still refuses the pre-existing stale `tools/release/flasher_memory_contracts.py`;
that file and its inventory inputs were not changed. The full resource matrix
and platform pilots were not run. Generic Miri/ISA suites are regression evidence,
not direct execution of this new mixed-runtime expiry scenario on target silicon.

## Boundaries

This fixes request expiry between segments, not receipt culling, application
cancellation, continuation refusal before admission, overlapping whole/split
responses or cumulative delivered-value accounting. Those remain in the
[response-size investigation](../../../prns-core/plans/response-size-accounting.md).
The targeted owner tests directly exercise core state; the four regression
scenarios exercise real runtime request APIs and virtual BLE. Simulated endpoint
names do not constitute native OS/board coverage. Production entropy is not
controlled, so no byte-for-byte replay claim is made. No hardware test is claimed.
