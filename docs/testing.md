# Testing Changes

Prns uses a verification ladder. Start with the narrowest test that owns the
behavior, then move outward when a change crosses package or platform
boundaries. Evidence should say what ran, on which host, and whether anything
applicable was intentionally omitted.

## 1. Target the owner

Run a specific package, test target, or test name while iterating:

```console
cargo test --locked -p personal-rns
cargo test --locked -p prns-core type1_header_round_trips
python3 -m unittest tools.tests.test_task_runner
```

Use `python` instead of `python3` on Windows. Package-local READMEs and test
modules own their more specific commands.

## 2. Run the normal core path

```console
cargo test --locked
```

This tests the root workspace's default members. It is the usual pre-handoff
check for a focused core change.

## 3. Run the root workspace

```console
cargo test --workspace --locked
```

This includes every root workspace member. Several platform, daemon, FFI, WASM,
and integration packages intentionally live in their own Cargo workspaces and
are not implied by this command.

## 4. Run affected integration and platform suites

Discover exact registered suites:

```console
python3 validation/run.py list --platform current
python3 validation/run.py list --domain interop --platform any
python3 validation/run.py matrix --tier pr --platform current
```

Platform selection is explicit:

- `current` means portable suites plus suites for the detected host.
- `any` means portable suites only.
- `linux`, `macos`, `windows`, or `android-device` means exactly that platform.

Run one affected suite by ID:

```console
python3 validation/run.py run --suite integration-capstones
```

An explicitly selected incompatible suite still fails closed. A selection that
becomes empty is an error rather than a silent pass.

## 5. Run the longer applicable-host PR lane

```console
python3 validation/run.py run --tier pr --platform current
```

This attempts every applicable selected suite even if one fails and writes
structured evidence beneath `validation-artifacts/`. It is intentionally not
called “everything”: other operating systems, physical Android devices,
scheduled fuzzing, mutation analysis, and release-only evidence have their own
owners.

## Documentation and tooling

When changing repository commands or guides, also run:

```console
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml
```

(On Windows: `.\tools\prns.cmd verify` and `python validation/run.py verify`.)

The tool and validation registries reject missing or unowned implementations.
Website tests reject benchmark-results link regressions and route drift.

## Benchmarks are evidence, not unit tests

Before a full or publishable benchmark run:

```console
cargo benchmark --smoke
```

Read the [benchmark guide](../benchmarks/README.md) for resume, publication,
energy, profiling, and interpretation rules.

## Contribution expectations

When a change touches the configured mutation surface, run a focused mutation
audit over the changed owner before submission and report the command, findings,
and human triage. Mutation findings do not replace the normal correctness,
performance, and platform evidence for the change.

Before handing off a change:

1. Format the languages you touched.
2. Run the narrow owner tests.
3. Run `cargo test --locked` when the root core can be affected.
4. Run exact validation suites for cross-cutting behavior.
5. Report commands and outcomes; do not imply an unrun platform passed.

The deeper [validation reference](validation.md) defines registry structure,
release aggregation, evidence custody, fuzzing, and mutation policy.
