# Journal power-loss foundation

The host campaign executes the production no-std `FlashJournal` against a small,
bounded implementation of the async NOR-flash traits. Journal framing, CRC,
verification, generation selection and restore remain production code. The model
owns only persistent bytes, flash constraints, operation traces and power cuts.

Six 256-byte pages provide the two timebase pages and two two-page arenas. Reads
and writes require four-byte alignment; erases require page alignment. Programming
cannot change zero bits back to one. Invalid operations refuse the whole value
without modifying storage or consuming a trace entry. The trace fails explicitly
at 128 operations; it never evicts evidence.

A cut selects an operation ordinal and the number of bytes completed before
power disappears. Subsequent valid I/O returns `PowerLost`. Reboot constructs a
fresh flash driver and production journal from the surviving image; it does not
reuse journal cursors or in-memory compaction state.

## Generated campaign

The baseline already contains two committed generations, leaving meaningful data
in the inactive arena. A successful compaction supplies the bounded I/O manifest.
The campaign then interrupts every byte-prefix boundary of each write and erase,
and both boundaries of every verification read. Each trial begins with the same
baseline image and verifies its operation trace matches the reference prefix.

There are 694 cut points across 18 compaction I/O operations. Expected recovery is
independent of restore output: before the complete final four-byte commit word,
the prior epoch and complete ordered records must survive; after it, the new epoch
and complete ordered records must survive. A second reconstruction must return
the same result. A full write followed by lost acknowledgement is explicitly a
committed durable state despite the operation returning an error.

The two payloads are opaque journal records using existing kind tags. This proves
atomic journal-generation recovery, not validity or transactional activation of
controller grants or discovery groups. Model tests independently check illegal
bit transitions, bounds/alignment refusal, exact torn-write/erase images and
sticky power loss.

## Scope and next boundary

Production impact: none. This is host-only test infrastructure and one additional
direct dev-dependency on the already-locked async flash traits.

The model covers deterministic contiguous byte-prefix tears, not every possible
bit pattern, physical erase physics, wear, timing or a specific chip geometry.
Operations complete immediately; this is not yet simulated asynchronous storage
latency or cancellation while an I/O future is pending. No wall-clock sleeps or
OS files are needed.

This directly strengthens the common journal used by embedded persistence. It is
not a Tokio/Embassy full-node reboot test: wiring the model through Embassy's
persistence owner and exercising Tokio's distinct store/restore path remain
follow-ups. Boot reads, append, timebase rollover, corrupt-media recovery and
runtime activation/rollback also need separate campaigns. Existing focused
production journal tests remain in place.

## Verification

Cargo commands use `CARGO_INCREMENTAL=0` on macOS arm64:

```console
cargo test --locked -p prns-simulation --test journal_power_loss -- --nocapture
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
python3 validation/run.py run --suite virtual-device-simulation
cargo test --workspace --locked --quiet
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
./tools/prns repo.notices.check-inputs
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The four focused tests passed, including all 694 cut points. The registered
simulation suite passed in 81.597 seconds including compilation. Workspace,
clippy, format/docs, registry and website checks passed. The locked notice bundle
was regenerated after the manifest change; no package version was added.
