# Queued Embassy persistence through power cuts

The next recovery boundary executes the real Embassy persistence mailbox and
owner, not direct journal writes. Tests live beside that private owner so the
simulator does not require a new public maintenance API. Existing owner tests
were moved into their own module; production implementation text is unchanged.

One campaign admits a group replacement with room for an append. The second
begins with a full journal and requires the owner to persist its compaction
budget, erase the inactive arena, copy the confirmed state, commit that generation
and append the replacement. Each turn uses the real bounded persistence step.
Admission must initially be pending; settlement must occur within 32 turns.

The successful run supplies an operation manifest. Fault trials cut every byte
prefix of every write and erase, plus both boundaries of every read. There are
65 append cuts and 1,291 compaction/write cuts. The trace is capped at 128 entries;
no entries may be silently dropped. Each interrupted run must match the reference
prefix exactly. Independent model assertions check torn byte images, partial
reads and sticky power loss using the existing owner's test flash underneath.

Every interrupted request must settle as `Flash` failure without publishing the
candidate. A fresh owner, engine and exchange then restore the surviving byte
image twice. Recovery must yield the complete confirmed snapshot before the final
record commit word completes, and the complete candidate afterward. Lost
acknowledgement can therefore mean a failed request but committed durable state;
failure is not treated as proof that the write did not happen.

## Scope

Production behavior: unchanged. This is test coverage and module organization.
The fixture models contiguous prefix tears. The original campaign lets the owner
observe a sticky flash error and settle before discarding volatile state. The
abrupt-removal extension below instead drops pending I/O before error handling.
Neither is instruction-level power removal. Reboots retain bytes only, not owner
state or journal cursors.

This proves queued group persistence and compaction recovery. It does not prove
radio activation/rollback, controller-grant transactions, Tokio storage, a whole
running-node reboot, physical flash physics or target ISA execution.

## Verification

The registered `embedded-persistence-recovery` suite runs all owner tests on PR,
release and scheduled ladders, including these fault campaigns. The separate
`virtual-device-simulation` suite retains the radio and lower-level journal
campaigns. Miri source ownership follows the extracted directory; this slice
does not claim a new Miri or firmware qualification run.

```console
CARGO_INCREMENTAL=0 python3 validation/run.py run --suite embedded-persistence-recovery
CARGO_INCREMENTAL=0 cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib
CARGO_INCREMENTAL=0 cargo clippy --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --all-targets -- -D warnings
```

On macOS arm64, all 144 Embassy library tests passed, including all 32 tests in
the registered recovery suite (3.651 seconds). The registered simulation suite
passed in 89.013 seconds including compilation/build-lock waiting. Workspace
tests, Embassy clippy, format/docs, registry validation, website tests and all
16 assurance-selection tests passed. These durations are verification run times,
not production performance measurements.

## Abrupt removal while I/O is pending

Each of the same 1,356 cut points now also suspends the flash future after exactly
the selected byte prefix. The harness polls the real persistence turn, requires
`Pending`, then drops that future. The request must still be pending, no write
failure diagnostic may have occurred, and the published configuration must still
be the confirmed value. The function then discards the owner, engine and mailbox;
only the flash byte image is passed to the two fresh restore owners.

The trace and complete flash image must exactly match the corresponding reported
error trial. Independent flash-model tests poll suspended operations repeatedly
to prove they neither finish nor repeat their storage mutation. Recovery uses the
same commit-word oracle, including fully committed writes whose acknowledgement
never reaches the owner. This adds owner cancellation during pending storage I/O,
not recovery by resuming a cancelled owner in place or a whole-node reboot.

The extension passed all 145 Embassy library tests, Embassy all-target clippy,
format/docs, registry and website checks on macOS arm64. Both fault modes run in
the existing registered recovery suite. No production source changed; the wider
radio simulation, firmware matrix and Miri were not rerun for this test-only
extension.

## Continued use after recovery

Every surviving cut image now boots another fresh owner and submits a distinct
`after-reboot` group configuration through its real mailbox. Success must publish
the full new snapshot, and two subsequent fresh boots must recover that snapshot.
This closes the gap between readable recovery and continued durable use.

All 65 append-cut images accept the new write immediately. Of the 1,291
compaction-cut images, 1,188 first return the typed `Capacity` outcome because the
persisted wear budget disallows another compaction yet. Those cases must perform
no flash I/O and retain the recovered snapshot, both at boot and one millisecond
before the independently expected deadline. At the exact deadline the same owner
must accept and durably complete the new request. The original 100 ms attempt is
rounded up to a 60,000 ms budget marker, then the configured daily interval is
added; the test does not derive its expected deadline from the owner's result.

Reported-error and abrupt-removal trials already require identical surviving
byte images, so one continuation runs per distinct cut case. This remains
owner-level group persistence evidence, not live radio activation or a complete
running-node lifecycle. No production behavior changed.

The continuation extension passed the registered recovery suite (33 tests), all
145 Embassy library tests, all-target Embassy clippy, registry and website checks
on macOS arm64. The full radio simulation, firmware matrix and Miri were not
rerun for this owner-test-only change.
