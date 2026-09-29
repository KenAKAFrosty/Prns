# Split-correlation ownership

Local macOS arm64 evidence, 2026-09-26, candidate based on `0ee2e4c27`.

## Reproduction and correction

The new admission regression failed before the production edits: after accepting
the first response segment, the engine accepted a continuation naming another
pending request and emitted a Resource pull. This observed failure was the first
case in the test matrix, not an independently observed red run for every variant.
The passing matrix also covers retargeting to a zero-byte-budget request, changing
response to request with the same ID, and dropping the association entirely. Each
refusal leaves both receipt deadlines untouched; the valid original continuation
then completes with exactly the expected bytes and settlement.

A second red reproduction removed only the new cleanup guard while retaining the
new correlation storage. The stale first transfer settled the request as failed
even though a replacement split chain was answering that same request. With the
guard restored, late completion and sender cancellation retire only the old
transfer, preserve the replacement's receipt/deadline, and allow exact completion.
These are controlled protocol reproductions, not evidence of historical user loss.

`AssemblyCorrelation` retains only the semantic wire association: unsolicited,
request ID, or response ID. It deliberately excludes outgoing request policy.
`IncomingAssemblies::begin`, `fit` and `advance` require this explicit association;
there is no permissive omitted-correlation overload. Fixed and heap assembly
tables keep the correlation column aligned through removal and replacement.
Initial continuation validation precedes response policy so inconsistent offers
cannot settle the unrelated request they name. Queued promotion, normal opening,
resumed decompression, advancement and failure cleanup use the same owner state.
No wire format, buffer size or transfer capacity changed.

## What the tests establish

The exact new failure injections are **core-engine tests**, driving real shared
production code with encrypted Resource frames. They do not run inside the
multi-node Tokio/Embassy simulator. The registered simulator suites pass separately
as regression coverage; that is not an end-to-end reproduction of these particular
faults. The new queued-promotion case directly injects changed assembly state at
the bounded queue seam. The existing delayed-decompression case supplies worker
output at the core seam, not through a bzip2 codec.

The test-only assembly module exercises the request/response/unsolicited matrix
with arbitrary full request IDs, refused advancement without progress changes,
exact subsequent completion, and fixed/heap column ownership after swap removal,
reuse and replacement. Tests remain behind `#[cfg(test)]`.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; canonical firmware commands do not.
Completed:

- `cargo test --locked -p prns-core routing::links::resources --lib` (268 tests).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host routing::links::resources --lib`
  (247 tests without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Registered `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `interop-large-request`, `interop-resource-rejection` and
  `interop-hopspot-remote-path` suites through `python3 validation/run.py run --suite …`.
  Interoperability used local loopback sockets.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and
  `cargo run --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`.

## Memory and readiness

The canonical T114 report passes: 142,980 static section bytes plus four bytes of
linker padding, the unchanged 69,632-byte runtime-stack reservation, and 376 bytes
of RAM headroom. Static sections grew by 16 bytes relative to the preceding
slice's report. Flash headroom is 145,428 bytes. The measured stack assessment
has 10,352 bytes remaining, with its existing missing-frame/call-graph gaps;
this is not a complete worst-case stack proof. The report is at
`target/flash-artifacts/resources/configured/reports/t114.json`, collected against
this candidate's production code before the final evidence-document updates.

The first report invocation refused a source snapshot that changed during
collection; rerunning with a stable tree passed. No canonical build flags or
memory guards were relaxed.

`./tools/prns doctor embedded-assurance` reports insufficient disk headroom for
the full resource lane (1.4 GiB available versus its 24 GiB readiness threshold)
and the missing pinned Renode pilot. Resource, Miri and ISA toolchains are present;
the single cached T114 target was still runnable. The generated-contract check
remains red for the pre-existing stale `tools/release/flasher_memory_contracts.py`
entry documented in the preceding slice. That unrelated file was left untouched.

## Focused mutation audit

The zero-context diff selected 38 mutants in `assembly/core.rs`,
`assembly/correlation.rs`, `receive/gate.rs` and `receive/conclude.rs` using
`cargo mutants --in-place --no-config -p prns-core --in-diff`, with the matching
`-f` paths, `--no-shuffle --timeout 30 --build-timeout 180 -C=--locked` and
`-- --lib routing::links::resources`. The environment was
`CARGO_INCREMENTAL=0 PROPTEST_DISABLE_FAILURE_PERSISTENCE=1`. The 268-test baseline
passed; 27 mutants were caught, eight generated `Default` replacements could not
compile, and three survived. None timed out. Evidence is retained under
`validation-artifacts/split-correlation-ownership-mutation/`.

One survivor exposed a coverage gap: changing cleanup's `total_segments > 1`
to `>= 1` could apply the new split-only exception to a whole response. The
existing whole-failure test now gives the waiting split assembly the same request
ID and still requires the whole failure to settle. This preserves existing
whole-response behavior without redesigning whole-vs-split arbitration. No
production change was needed. A focused one-mutant rerun using
`--re 'replace > with >= in EngineState<S>::settle_failed_resource_claim'` caught
that mutant; its evidence is under
`validation-artifacts/split-correlation-boundary-mutation/`.

The two other survivors appear equivalent for reachable in-repository paths:

- `begin` capacity `<` to `<=`: the fixed table's `push` independently refuses a
  full table, while the heap table cannot reach its `usize::MAX` capacity.
  Fingerprint: `5d6d8abfde22346ea390d818f63e2b4ffffbc5e0ded6adfaf7040632712e1be5`.
- Initial admission's count `> 1` to `>= 1`: earlier ingress checks already require
  `1 <= segment_index <= total_segments`; a whole offer therefore cannot pass the
  unchanged continuation-index check. Fingerprint:
  `4d23f23b27bb9d936b10da9dcfa28012c114f493477c19646c341fdee34c694c`.

These findings are recorded for review, not globally accepted or excluded.
`mutation-shard-check` confirms complete evidence for both runs; the original
`mutation-check` remains red for its recorded survivors, while the boundary rerun
passes. The production diff matched its pre-audit snapshot after restoration.
Resource tests in both configurations, the no-default-features build, focused
Clippy, formatting and whitespace checks were then rechecked successfully.

## Coverage limits

Cumulative value limits, advertised stream totals, receipt expiry between segments,
pre-admission cleanup, and whole-vs-split arbitration for the same request remain
in the [owner plan](../../../prns-core/plans/response-size-accounting.md).
No full firmware matrix, Miri/ISA execution, Renode, other host-platform, physical
hardware, or many-node scale tests ran for this slice. Root/workspace tests do not
imply that separately rooted Cargo workspaces passed.
