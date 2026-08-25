# Validation and release readiness

For the beginner verification ladder, start with
[Testing changes](testing.md). This document owns the deeper suite registry,
evidence, proof, interoperability, and release aggregation model.

The [stock-RNS interoperability test checklist](interop-checklist.md) records
the working set of behaviors Prns exercises against the reference
implementation.

The `list`, `matrix`, and `run` commands accept an explicit host selector:

```console
python3 validation/run.py list --platform current
python3 validation/run.py matrix --tier pr --platform any
python3 validation/run.py run --tier pr --platform current
```

`current` selects portable suites plus suites for the detected host. `any`
selects only portable suites. `linux`, `macos`, `windows`, and
`android-device` select that exact platform. Explicit incompatible suite runs
and empty selections fail closed.

Prns keeps tests close to the code that owns their assertions. Unit tests,
property tests, compile-fail documentation, and private Kani proof bodies stay in
their crate. The `validation/` tree is the control plane for suites that cross a
crate, process, toolchain, platform, or implementation boundary.

## One executable inventory

`validation/manifest.toml` is the machine-readable inventory. It assigns every
suite an ID, domain, tier, platform, toolchain, command, timeout, input set, and
artifact location. Cargo manifests still own test targets, Cargo-fuzz still owns
fuzz targets, and Rust source still owns Kani harnesses; the validation registry
discovers those native definitions and rejects drift instead of copying their
assertion inventories.

The dependency-free operator interface is:

```console
python3 validation/run.py verify
python3 validation/run.py verify --check-tools
python3 validation/run.py list
python3 validation/run.py list --domain interop --tier release
python3 validation/run.py matrix --domain kani --tier release
python3 validation/run.py run --suite integration-capstones
python3 validation/run.py run --domain oracles --tier pr
```

`verify` fails on duplicate or malformed suites, missing commands and inputs,
unregistered Cargo workspaces, Kani proofs, fuzz targets, interop peers, smoke
scripts, stale exemptions, invalid tool pins, or malformed mutation triage. Use
`verify --check-tools` when the pinned deep-validation tools are expected to be
installed locally.

A successful verification narrates what it proved instead of returning only an
opaque success token. Counts come from the live manifest and native source
discovery, so the output also gives an operator a quick orientation:

```text
[verify] Suite policy: 108 total suites (53 pull-request, 98 release, 105 scheduled); IDs, tiers, platforms, toolchains, commands, timeouts, and artifact paths are valid.
[verify] Cargo ownership: 54 manifests are registered, valid, and repository-owned; 24 first-party lockfile workspaces are inventoried; 22 unique workspace roots own formatting.
[verify] Native discovery: 20 Kani proofs and 9 fuzz targets exactly match their source owners.
[verify] Asset ownership: 71 oracle/interop/smoke assets are registered; 1 documented exemption is current; nothing is orphaned.
VALIDATION_REGISTRY_OK
```

The explanatory lines are for people; the final token remains a stable hook for
automation. Commands whose stdout is a data interface, such as `matrix` JSON and
`list` TSV, send their explanatory summary to stderr so existing consumers can
parse stdout unchanged.

Documentation is intentionally not configuration. Commands shown here are
examples; the manifest remains authoritative.

## Layout and ownership

- `validation/integration/` is the public-API, cross-crate capstone workspace.
- `validation/fuzz/` owns fuzz targets, seed corpora, and reproducer artifacts.
- `validation/oracles/` owns deterministic Rust-versus-stock-RNS comparisons.
- `validation/interop/` owns live stock-RNS peers and process-level smoke cases.
- `validation/hardening/` owns sanitizer, Miri, coverage, and unsafe-code tooling.
- `validation/mutation/` owns cargo-mutants configuration and reviewed survivor
  triage.

Release-tooling tests live under `tools/tests/`. Product-specific platform and
WebAssembly tests remain with the product they exercise. Private RPC codec tests and
Kani harnesses remain source-local, where they can exercise private behavior;
the registry only supplies focused execution commands.

## Tiers

The `pr` (pull-request) tier is deterministic and bounded. It covers registry
integrity, normal tests and lints, release contracts, product build lanes,
deterministic stock-RNS comparisons, and essential interop.

The `release` tier adds every proof, every bounded fuzz target, full oracle and
utility interop coverage, sanitizers, Miri, and the remaining shipping-platform
evidence. A release result is acceptable only when all required results are
bound to the exact candidate commit.

The `scheduled` tier is allowed to spend longer on fuzzing, diagnostics,
coverage, mutation analysis, unsafe inventory, and hardware/network simulations.
Scheduled evidence is useful for maintenance but cannot substitute for exact-SHA
release evidence. The physical Android runtime suite remains registered here and
requires its device-qualified runner, but it is separate from hosted release
readiness unless an Android application release explicitly places it in scope.

## Stock-RNS environments

Ordinary `cargo test` never searches for or silently uses a local Python
environment. Oracle and live interop suites require explicit interpreters that
contain the RNS version pinned for that evidence domain. Prepare reproducible local
environments with:

```console
python3 validation/run.py prepare-oracles
python3 validation/run.py run --domain oracles --tier pr --platform current
python3 validation/run.py run --domain interop --tier pr --platform current
```

The runner sets `SMOKE_PYTHON` and `RPC_SMOKE_PYTHON` for each registered suite
after verifying `RNS.__version__`. CI may provide those variables directly, but
the same version check still applies.

## Evidence

Every invocation writes versioned evidence beneath
`validation-artifacts/results/<suite-id>/`:

- `stdout.log` and `stderr.log` preserve the complete process output.
- `result.json` records schema version, suite and domain, exact commit SHA,
  resolved command, host platform, tool versions, timestamps, duration, exit
  state, timeout state, and spawn errors.

Set `PRNS_VALIDATION_ARTIFACTS` to use another evidence root. A release
qualification job combines downloaded results only after checking their schema,
status, suite coverage, and commit binding:

```console
python3 validation/run.py aggregate \
  --tier release \
  --expected-sha 0123456789abcdef0123456789abcdef01234567
```

Missing, failed, skipped, or differently bound suites make aggregation fail.

## Deep tools

The exact validated cargo-fuzz, cargo-mutants, and Kani versions live in
`validation/manifest.toml`. CI installs those exact versions. Local operators can
check them with `verify --check-tools` before starting a long run.

Kani proofs are discovered from `#[kani::proof]` in `prns-core/src`. They are
classified by subsystem in the manifest and become isolated matrix entries.

Fuzz targets are discovered from `validation/fuzz/Cargo.toml`. The runner gives
each target a bounded runtime, a distinct evidence/reproducer directory, and a
writable corpus copied beneath `validation-artifacts/`. Checked-in seed corpora
remain immutable source and are never selected by cleanup.

Miri and each sanitizer run as separate suites so one failure does not hide the
rest of the hardening matrix. The helper implementations remain under
`validation/hardening/`, but operators should invoke their registered suite IDs
through the runner when release evidence is required.

## Mutation triage

`validation/mutation/config.toml` defines the mutation surface. The mutation
runner emits cargo-mutants results, fingerprints each missed or timed-out mutant
from stable semantic fields, and compares them with
`validation/mutation/triage.toml`.

Mutation analysis is a scheduled or manually dispatched audit, not a
pull-request or release gate. Its findings require human classification. A
useful mutant normally results in a stronger behavioral test. Mutation output
alone is not a reason to rewrite production code; any production change needs
an independent correctness or design rationale, and performance-shaped code
requires comparative measurement before acceptance.

Every accepted survivor must have a lowercase SHA-256 fingerprint, a concrete
reason, a reviewer, and an expiry date. New, changed, timed-out, stale, or expired
entries fail the audit. An accepted cargo-mutants nonzero exit is therefore never
an unreviewed blanket waiver: the checked-in triage must exactly match the
current unresolved set and the baseline must have succeeded.

## CI and release qualification

`.github/workflows/ci.yml` runs the bounded product lanes and exposes one
fail-closed aggregate. Every shipping lane is a dependency of that aggregate;
GitHub's `skipped` result is not green.

`.github/workflows/deep-validation.yml` runs Kani, fuzzing, full oracle/interop,
sanitizer, and Miri jobs independently with `fail-fast: false`.

`.github/workflows/mutation-audit.yml` runs the sharded mutation audit monthly
or by explicit dispatch. Its findings remain outside ordinary CI and exact-SHA
release qualification, and the workflow must not be configured as a required
status check. A red audit means its evidence needs operator triage; it does not
mean the product build or candidate failed.

`.github/workflows/release-readiness.yml` is manually dispatched with a full
commit SHA. It checks out that exact object, records evidence in each platform
job, downloads all results, and creates a single release manifest only when the
entire release tier passed for that SHA.

Hosted acceptance still depends on an enabled Actions account with no billing or
spend-limit failure. A locally green tree cannot replace hosted platform and
exact-SHA evidence.

## Cleanup

Cleanup is dry-run-first:

```console
python3 validation/run.py cleanup
python3 validation/run.py cleanup --apply
```

The candidate set comes from registered Cargo workspace targets plus explicit
web, Android, fuzz, mutation, oracle-environment, Python-cache, and validation
outputs. The command refuses paths outside the repository and does not select
editor configuration, credentials, runtime identity/state, fuzz corpora, or
other source-owned data.
