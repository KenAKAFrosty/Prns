# Seeded actor scheduling

macOS arm64, based on `57a016251`, 2026-09-27.

## Contract

`ManualTaskRunner::new` retains cyclic admission order. The explicit
`new_with_scheduling` constructor also accepts `ManualTaskScheduling::Seeded`.
Version 1 orders admission ordinals by a domain-separated, wrapping-u64
SplitMix64 finalizer, then cycles through ready actors in that fixed order.
It does not randomly choose an actor on each poll. Continuously ready actors in
a fixed finite population receive one turn each per cycle; newly admitted or
newly woken actors enter at their position relative to the current cursor.

The seed is independent of fault generation. The version, seed, actor admission
sequence and wake inputs define the scheduler choices. This is not packet replay:
production cryptographic entropy, external concurrent wake timing, and polling
inside each actor are not controlled. Different seeds explore some alternative
interleavings, not every possible schedule.

The ready index remains a bounded ordered set, with logarithmic insertion,
retirement and selection, without scanning dormant actors. Its key now contains
two u64 values instead of one, including for the default policy. This adds eight
bytes of key payload per ready entry, plus policy/cursor overhead and allocator
effects; no claim of zero RAM cost or benchmarked speedup is made. No shipping
runtime, firmware memory contract or production protocol changes in this slice.

## Evidence

Unit tests pin exact version-one ranks and order, repeatability, distinct seed
orders, cyclic-default behavior, self-wake fairness, coalescing, unchanged time,
stale-waker retirement and completed/replacement actor isolation. A scan-based
reference checks sparse admission/retirement/poll sequences over 4,096 steps per
policy, including extreme ordinals. A separate 1,024-actor test cancels all but
nine actors before a saved-waker storm and checks exact survivor polls and index
counts. Ready actors still prevent clock advancement, including cooperative
deferred wakes.

The existing 128-production-node restart scenario runs under seeds 0, 7 and
u64::MAX in both cohort orders. Each case retains its original assertions:
unaffected traffic, exact timeout boundaries, fresh actor/interface attachment
isolation, obsolete-link refusal, recovered traffic, capacity refusal, coordinated
clocks, complete detach and bounded traces. These are real Tokio nodes over a
virtual frame medium, not native Bluetooth stacks, firmware or physical radios.
Node startup remains serial; concurrent traffic and restart waves exercise the
alternative actor orders.

## Verification

Passed simulation library tests (120), the six seeded fleet scenarios, the
registered `virtual-device-simulation` suite (40.276 seconds), simulation
all-target clippy with warnings denied, root tests, workspace tests, repository
format/docs checks and `git diff --check`. Existing ignored tests remain ignored.
No hardware, firmware build, Miri/ISA run or performance benchmark is claimed.
