# Asynchronous and overlapping core qualification

This slice adds a non-shipping scheduling seam and qualifications for the real
core and both runtimes. It does not replace production crypto, persistence,
protocol execution or public outcomes with simulated responses.

## Scope and contracts

The three-node BLE star has a primary controller, a target and an independently
paired controller. Actual identities, permissions, encrypted links, application
requests, channels, Resources, FileStore and NOR journal implementations participate.
Application state is ordinary `()`. Announcements are explicit fixture actions.
Unsupported watch and measurement capabilities remain unavailable. Unauthorized
requests remain silent. Canceling a local waiter does not withdraw a request or
undo an admitted application operation.

There are ten profiles: four Tokio/Embassy controller-target pairings, with inline,
one controlled worker and four controlled workers in each Tokio-containing
pairing. Embassy-only uses inline execution. Both controllers use the selected
controller runtime. A separate native full-node loopback test runs real threads
with one and four workers; controlled scheduling does not claim to emulate thread
interleavings or native batch formation.

`simulation-control` is absent from the runtime's default features. Its worker
queues, admission, work accounting, result rings, crypto implementation, wakeup
and manifold consumption are the production owners. The harness selects actual
execution and publication transitions. Worker IDs, job IDs, boundaries, work
kinds, occupancy and retirement are typed and bounded. Feature-disabled builds
contain none of these scheduling hooks.

## Behavioral evidence

- Valid signature execution, independent publication, held results, interactive
  precedence, reversed four-worker completion and actual ring backpressure.
- Retirement before execution, after execution and after publication, with fresh
  pool ownership and old-generation rejection.
- Full-node resource builds held at execution and publication. A second controller
  completes while the primary is pending; releasing the actual result delivers
  the expected bytes. A 16 KiB Resource exercises worker `OpenSpan` publication.
- Closing a link or retiring the primary with a computed resource result cannot
  land that result on replacement links or nodes.
- Concurrent bounded Resources, ordinary requests, channels and authenticated app
  messages retain the correct controller/link/command ownership.
- Channel window pressure settles one `WindowFull` failure; received commands
  settle once. Public outcomes agree with actual Tokio outcome counter deltas.
- Response refusal retains `ResponseTooLarge` across transfer cancellation. The
  link and request capacity remain usable afterward. Lost responses become local
  timeouts; canceled callers do not undo the verified handler invocation.
- Inventory includes both peers, and absent rate measurements stay absent.
  Implemented watches deliver their initial resync and refetch after reconnect.
  Byte streams deliver complete bytes and EOF; duplicate readers are rejected.
- FileStore authority writes can be held while engine channel work continues.
  Fresh endpoint admission waits for the authority transaction, then completes
  after release. This is the current serialized authority boundary; it is not a
  promise of endpoint progress through an indefinitely stalled storage owner.

The common resource window is 1,024 bytes and the radio transmit depth is four.
The large-worker probe has a separately selected 32 KiB window. Budgets include
framing/encryption overhead. Native radio timing, modulation, HaLoW, CPU throughput
and deployment performance are outside this qualification.

## Replay and reduction

Version 1 cases specify seed, runtime/execution profile, family and atomic actions.
The routine matrix is four seeds × ten profiles × three families = 120 cases,
32 actions each. Extended inputs use seeds 0..63 and 64 actions = 1,920 cases.
Each case is process-isolated and compares full traces from two fresh fixtures.
Four bounded child workers use a pinned executable. Wire connection IDs are
normalized; bytes, persistence operations, events, crypto order and static
allocation evidence must match exactly within a profile. Wall-time crypto timing
is excluded from the semantic counter projection.

The extended release campaign uses Cargo's `simulation` profile to optimize
execution while retaining development-profile debug assertions and overflow
checks. Its 1,920 cases, two fresh fixtures per case, process isolation, and trace
comparison remain the same. Firmware and native release build profiles are
unchanged.

On the same macOS arm64 host, six seed-42 cases from the Tokio-only and
Embassy-only profiles produced identical complete semantic artifacts in both
profiles. Alternating profile order and excluding compilation, their combined
process time was 9.522 seconds in `dev` and 4.819 seconds in `simulation`
(1.98 times faster). The complete optimized campaign qualified all 1,920 cases
twice in 1,704.75 seconds; the validation command took 1,846.06 seconds including
compilation. These are host measurements, not a guarantee of CI runner speed.

Passing artifacts keep inputs, observations and SHA-256 trace digests. Seed 42
reference cases and failures retain full traces. Runtime, invalid-input and replay
failures have distinct classes. The bounded reducer preserves the action and
original diagnostic in both fresh runs. Original evidence and every candidate
are retained in separate files. Standalone reduction first reproduces the original
and refuses to claim a repaired failure is still reproducible.

The first routine campaign reduced an invalid fixture sequence to two successive
byte-stream operations that reused a link-owned stream ID. The corpus now checks
reader ownership and closes the old link before starting the next stream. No
production behavior was changed to satisfy that mistaken assumption.

```console
python3 validation/run.py run --suite virtual-device-simulation
python3 validation/run.py run --suite core-work-simulation-extended
python3 validation/run.py run --suite core-work-resource-stability
./tools/prns repo.simulation.core-work.replay --case PATH/TO/case.json
./tools/prns repo.simulation.core-work.reduce --case PATH/TO/original.json
```

Campaign artifacts are under
`validation-artifacts/results/core-work-simulation/{routine,extended,replay,reduction}/run-NNNN/`.
Existing Remote Control formats, artifacts and tasks are unchanged.

## Resource qualification

A separate isolated heap probe executes 32 construction/work/fault/recovery/drop
cycles per profile. Actual Tokio resource rows, admission depth, crypto queue depth
and outstanding packet verdicts must drain. Embassy measurements unavailable
through the public metrics API are not invented. Timer queues must remain within
their explicit capacity. Every fixture releases all BLE connections. Full-node
traces separately account for deliberately retained static Embassy test wiring.

After each trace and fixture is dropped, retained heap must fit the measured
static retention plus an explicit 64 KiB auxiliary allowance. The dynamic peak
budget above that measured retention is 32 MiB. These are harness bounds, not
product memory requirements. The probe originally exposed oversized capture
reservation; the fixture now reserves 65,536 wire values and 131,072 medium events,
while requiring zero eviction.

## Verification record

Host: macOS Apple Silicon, 2026-10-01. Physical boards, other operating systems
and ISA execution are not part of this qualification.

The registered simulator suite passed 428 tests with 11 explicitly ignored,
including the new 120-case routine campaign. Routine artifacts from that suite
are in `routine/run-0003`; each case has two matching fresh-fixture traces.
The extended campaign passed all 1,920 cases in 2,380.15 seconds
(`extended/run-0000`), with 640 cases per family and two matching runs per case.
A final artifact audit independently checked all 1,920 recorded pairs. It found
3,780 compact runs and 60 full seed-42 reference runs.

The extended launcher pinned its executable before the fixture module extraction
and capture-reservation reduction. It used 131,072 wire values and 1,048,576
medium-event slots; final-source routine and heap qualifications exercised the
smaller reservations documented above. Case generation, protocol behavior,
controlled worker transitions and counter assertions were unchanged. The pinned
binary is retained with SHA-256 `90cb60a8e8e460b0b4bd534596592a544179f780d16ed17dbe4a8ef30932d1af`.
Existing Remote Control qualification evidence remains intact.

Owner and product results:

| Check | Result |
| --- | --- |
| Root `cargo test --locked` | 2,463 passed; four ignored |
| Core + shared runtime libraries | 2,178 passed; three ignored |
| Tokio with `simulation-control` | 312 passed; one ignored |
| Ordinary Tokio | 306 passed; one ignored |
| Embassy | 174 passed |
| Registered embedded persistence recovery | 59 passed |
| Headless `wifi-halow,websocket,wifi-auto` tests + host build | 11 passed; build passed |
| Worker owner tests | 31 passed, including six controlled-worker tests |
| Native full-node one/four-worker probe | Passed; waits on observed readiness and exchanges concurrent verified requests |

Strict Clippy passed for the simulator with `controlled-time,heap-profile` and
for the Tokio runtime with `simulation-control`. Ordinary Tokio compilation,
Rust formatting, diff whitespace and validation registry verification passed.
The replay task passed against the repaired two-stream case. Standalone reduction
correctly refused the repaired original while retaining it and its fresh passing
baseline. No configured production mutation surface was changed in this slice.

All ten isolated 32-cycle heap profiles passed in 201.64 seconds. The greatest
observed peak was 16,014,668 bytes. All-Tokio profiles retained 184 bytes after
fixture/trace drop. Mixed profiles retained 6,584 bytes beyond measured static
wiring; Embassy-only retained 8,632 bytes beyond that wiring. Static retention
was 1,154,560 bytes for the mixed probes and 2,309,120 bytes for Embassy-only,
accumulated across fresh fixtures and target restarts. These numbers describe
the test harness, not a shipping node's footprint.

`./tools/prns verify` still fails on the existing unowned script implementations
in `personal-hopspot/headless/scripts/network-lab/{lease.sh,prepare.py,radio.sh}`.
The task-runner tests encounter the same existing hygiene issue. This slice did
not relocate those files. The newly registered replay task executes successfully,
and `python3 validation/run.py verify` passes.

The Tokio storage fixture reserves each scenario directory with atomic
`create_dir`, skipping occupied process-ID/counter names within a 10,000-attempt
limit. Reused process IDs and retained files from interrupted workers therefore
remain isolated from a new scenario. Reservation leaves occupied paths intact
and propagates every filesystem error other than `AlreadyExists`. Component
tests force a collision, exhaustion and a missing parent without changing the
process environment. Scenario teardown removes only its successfully reserved
directory.
