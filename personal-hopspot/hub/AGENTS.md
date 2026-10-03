# Hopspot Hub candidate contract

Work on the local `hopspot-hub` branch. Leave review candidates uncommitted and
do not push. The roughly 1,000-line slice target is a scale guideline; keep a
larger change together when required for cohesive implementation and evidence.

Core ownership follows `domain_primitives` and `state_machines`. Keep validated
values independent of machines. Keep machine state, transitions, input/output
contracts, tests, and architecture/behavior expectations with their owner.
Use private modules and curated public exports. Inherit the hub workspace's
lint baseline explicitly and keep production core `no_std`.

Every machine operation, including queries, has its own `StepInputOf` input
and exact outcome. Constructors and private helpers may remain methods. Do not
add comments, including doc comments. Put necessary prose in owning Markdown
documentation and express contracts through names, types, and tests.

Before presenting each candidate, run the verification commands in `README.md`.
Reuse Pipecircuit's Cargo aliases and mutation configuration; do not add custom
verification runners. Require 100% LLVM production-source function, line, and
region coverage and no surviving, timed-out, or unclassified mutants. Tests, snapshot
checks, and proof harnesses are outside the production coverage denominator;
production files may not be excluded to make the gate pass. Report caught and
unviable mutants separately. A failing or missing evidence lane is not approval
readiness, and must be reported accurately rather than waived implicitly.

Each machine keeps code-derived `architecture.rs` and compiled-test-derived
`behavior.rs` expectations, following pipecircuit-http1. Primitives with their
own behavior tests keep a behavior inventory as well. Review snapshot changes;
the verification gate must not accept them automatically.

Look for bounded Kani proof opportunities in every slice. Use property tests
for larger state spaces when formal verification is impractical. State proof
bounds and property-test scope. Preserve upstream type contracts in test
fixtures, and do not claim unexecuted proof harnesses passed.

Triage coverage gaps and mutants for actual behavioral significance. Strengthen
assertions before changing production code. Any production change prompted by
mutation analysis needs an independent correctness or design rationale. Do not
add exclusions for survivors without a reviewed equivalence argument.
