# Asynchronous and overlapping core qualification

Implementation ledger for the approved core qualification slice. Production
behavior changes require an independently explained defect and a minimized
regression. Validation controls are compiled only with `simulation-control`.

## Milestones

- [x] Controlled Tokio workers reuse production queues, execution, publication,
  accounting and completion routing; native worker tests remain separate evidence.
- [x] Three-node overlapping traffic fixture covers ten execution/runtime profiles,
  real persistence, bounded Resources/channels and implemented watch capabilities.
- [x] Public outcomes and available diagnostic evidence agree under injected faults,
  cancellation, retirement, backpressure and recovery.
- [x] Routine 120-case and extended 1,920-case replayable campaigns pass, including
  two independent runs, bounded reduction and compact passing artifacts.

## Contracts

Controllers and target enforce independent grants. Admission rejection remains
silent. A canceled local waiter does not withdraw a sent request or undo committed
authority. Unsupported capabilities remain absent. No connection-drop policy,
automatic announcement policy, enrollment UX, browser API expansion or hardware
qualification is introduced.

The common matrix has Tokio/Tokio, Tokio/Embassy, Embassy/Tokio and
Embassy/Embassy pairs. Each Tokio-containing pair runs inline, one controlled
worker and four controlled workers; Embassy-only uses its actual inline runtime.
The two controllers use the same selected controller runtime.

Routine inputs are four seeds (0, 1, 42, 0x5eed), ten profiles and three families
(completion lifecycle, mixed pressure, recovery/attribution), with 32 actions.
Extended inputs are seeds 0..63, the same profiles/families, with 64 actions.
Every case compares complete traces from two fresh fixtures within its profile.
Passing artifacts retain typed summaries, inputs and digests; fixed references
and failures retain full traces. Original failure evidence is never overwritten.

## Completion

Record owner tests, existing simulator/persistence suites, portable and ordinary
builds, headless checks, strict lint/format checks and registry checks. Record
before/after native measurements if shared hot paths change. Keep resource
occupancy, intentionally retained Embassy static allocations and physical device
performance separate. Any demonstrated unresolved correctness defect keeps this
slice incomplete.

The slice is complete. [Commands, measurements, limits and existing tooling
hygiene failures](measurements/core-work-qualification.md) are recorded with the
qualification owner.
