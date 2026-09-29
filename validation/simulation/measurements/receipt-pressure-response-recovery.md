# Receipt-pressure response recovery

Local macOS arm64 candidate based on `183e21073`, 2026-09-26.

## Discovery and production change

The preceding [expiry slice](live-link-response-expiry.md) reclaimed an assembly
when its request timed out between segments. This simulator slice exercises
receipt displacement instead: a single-slot receipt table must retain a newer
request and report the displaced request as `Culled`.

Before the fix, all four new runtime/endpoint cases correctly delivered the first
provisional segment, culled that request and completed the replacement Echo
request, but a later three-segment request on another link failed with
`ResponseTransferFailed(TransferCorrupt)`. The displaced assembly still occupied
the receiver's only slot. Reusing the original link alone would hide that row
by replacing it.

`CulledReceipt` now carries the displaced packet hash, including the rejected-new
receipt when the table has zero capacity. All five engine settlement call sites
preserve this identity. Culling and expiry share one core cleanup operation that
requires both the owning link and the matching response request ID. There is no
new stored column, firmware buffer, wire format or capacity change. The temporary
culling result grows; compiled resource evidence must determine the actual cost.

The existing expiry ownership matrix protects this shared cleanup against clearing
another request, another link, request-body assemblies or unsolicited assemblies.
New receipt tests check whole displaced values, retained replacement policy and
deadline, and zero-capacity refusal identity. Existing packet-send culling tests
now verify the actual displaced wire packet hash.

## Simulator contract

The segmented fixture now explicitly selects its fixed receipt capacity: two for
existing scenarios, one for pressure cases. Its one incoming/outgoing Resource
and assembly slots and 512-byte transfer window remain unchanged. Other table
choices remain the host fixture's; no shipping board profile is used or modified.

Each ESP32/Apple and nRF52/BlueZ pair runs with both request directions:

1. Observe exactly the first verified segment of a three-segment file through
   the raw request journal, including its whole value, link, index and request ID.
2. Drop subsequent advertisements for that link, leaving other traffic and
   keepalives intact. Issue a real buffered Echo request to displace the receipt.
3. Require the full replacement Echo value and measured RTT, plus exactly one
   `Culled` settlement for the old raw request and no additional chunks.
4. Advance twenty seconds of coordinated virtual time while withholding retries.
   This drains the sender's actual watchdog before probing receiver capacity;
   it is not a wall-clock sleep. The Embassy responder must journal Echo success
   followed by exactly one Resource timeout. The drop budget is sixteen frames
   and the entire operation has a thirty-second virtual deadline.
5. Remove the loss rule and complete a new three-segment file on a different
   existing link, then exercise refusal, raw success and repeated buffered success
   on the original links. BLE stays connected and no link-closure journal appears.

The first raw segment remains provisional: direct journal consumers must await
successful terminal settlement before publishing a whole response. This slice
does not claim to test buffered partial-body disposal on culling, mid-transfer
culling, continuation refusal, cumulative value accounting, physical radios or
byte-for-byte replay with controlled production entropy.

## Benchmark checkpoint before this slice

`CARGO_INCREMENTAL=0 cargo benchmark --smoke` passed 34/34 cells, 34 conformant
samples and zero validation errors on clean commit `183e21073`. Local ignored
run ID: `b3a4133f-2c17-48f6-91ca-3b4aa0f5925e`. Pinned stock interpreted RNS 1.5.4
was verified; energy was not requested. The first sandboxed attempt could not
resolve PyPI; rerunning with network access provisioned the pinned reference and
completed the matrix. Nothing was published.

Rough Prns-only readings compared with the recent complete run on the same M4
(`5cc5649b-df34-4355-a55c-ce504e8633b2`, source `d253e964f`):

| Workload | 500 ms smoke | Prior 30 s, three-sample median |
| --- | ---: | ---: |
| Single packets | 41.5k/s | 43.8k/s |
| Link messages | 76.4k/s | 78.3k/s |
| Requests | 30.3k/s | 30.8k/s |
| Maximum Resource segment | 331 MB/s | 330 MB/s |
| Maximum Resource segment, matched policy | 320 MB/s | 336 MB/s |
| 64-segment stream | 657 MB/s | 680 MB/s |
| 64-segment stream, matched policy | 662 MB/s | 700 MB/s |
| Raw relay | 241 MB/s | 243 MB/s |
| Resource relay | 3.23 GB/s | 3.32 GB/s |
| Resource relay, matched policy | 2.80 GB/s | 2.78 GB/s |

No obvious throughput cliff appeared. Several readings are a few percent lower;
the unequal durations, tiny sample count, host scheduling and different commits
do not establish a regression or prove its absence. In particular, the stream
cells completed only five or six transfers. Request p50/p99 was 0.130/0.182 ms.
This smoke checkpoint predates the receipt-culling changes described above; it
does not measure this new candidate's performance.

## Candidate verification

Host Cargo checks used `CARGO_INCREMENTAL=0`; the canonical firmware command did
not override its environment. Passed:

- `cargo test --locked -p prns-core cull --quiet`: nineteen tests passed, one
  existing test ignored. `cargo test --locked -p prns-core receipt_pressure --quiet`
  passed the new whole-value ownership test.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet`:
  42 tests passed, including all four previously failing pressure scenarios.
- Ten final-candidate repeats filtered to `culled` passed all forty scenario
  executions after the mutation audit restored the source.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six registered suites passed. Miri/ISA scenarios are regression evidence,
  not direct target execution of the new receipt-pressure capstone.
- `./tools/prns build embedded resources report --target t114`: static sections
  142,980 bytes plus four bytes padding, runtime reservation 69,632 bytes and RAM
  headroom 376 bytes. Measured stack evidence leaves 10,352 bytes, with its existing
  unresolved-call/frame gaps. Those figures are unchanged. Flash headroom is
  145,508 bytes, 120 bytes more than the preceding slice; no speed claim follows
  from this code-size difference.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Formatting, whitespace and touched-document relative-link checks.

Focused mutation audit:

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config --in-place -p prns-core \
  --file prns-core/src/engine/settlement.rs \
  --re 'culled_settlement|retire_response_assembly' \
  --timeout 60 --build-timeout 180 \
  --output validation-artifacts/mutation/receipt-pressure -- --lib receipt_
```

Baseline passed; removing the cleanup or inverting its ownership comparison was
caught. Replacing `Settlement` with `Default::default()` was unviable because the
type has no `Default`. No survivors or timeouts; this is a focused owner audit,
not a claim about the full mutation surface.

`./tools/prns doctor embedded-assurance` still reports insufficient disk for the
full resource matrix and missing Renode. `./tools/prns build embedded resources contracts --check`
still refuses the pre-existing stale `tools/release/flasher_memory_contracts.py`;
neither that file nor its inventory inputs changed. The full board matrix,
platform pilots and physical hardware tests were not run.
