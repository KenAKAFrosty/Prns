# Repository tools

`tools/` is the single home for commands that build, package, sign, install,
flash, or otherwise operate on the repository and its release products. Checks
and proofs live under `validation/`; Git-triggered integration lives under
`.githooks/`.

Start with:

```console
./tools/prns list
./tools/prns explain release.candidate.build
./tools/prns doctor getting-started
./tools/prns verify
```

`./tools/prns` is the canonical bootstrap and automation entrypoint. It can
inspect the environment and identify a missing Cargo installation without
requiring Cargo itself. After Cargo is installed, the repository-local
`cargo tools` alias is an exact convenience doorway into the same task
registry:

```console
cargo tools list
cargo tools explain release.candidate.build
cargo tools doctor rust
cargo tools verify
```

For example:

```console
cargo tools guide rust-basics
cargo tools release firmware build -- heltec-v4 target/dev-flash
cargo tools release source package -- --output target/source.zip
cargo tools release candidate build -- target/candidate preview KEY_ID
```

The two forms accept the same task paths and arguments. `cargo tools` does not
own task definitions or setup logic, so bootstrap checks and CI can use
`./tools/prns` without requiring Cargo. Product and daemon commands use the
separate `cargo prnsd` entrypoint.

The declarative doctor profiles are `getting-started`, `node`, `rust`, `docs`,
`tests`, and `benchmarks`. Profiles check the commands and important versions
for one outcome, including the platform C compiler where applicable. They print
setup guidance and never install software. No-argument, task-ID, and domain
doctor behavior remains available.

The operator interface prints every task's purpose and side-effect class before
execution. CI invokes the same named tasks and does not call implementation files
directly. `tasks.toml` is the executable inventory; implementation modules not
intended as commands must be explicitly classified as internal.

To add an operation, put its implementation in the narrowest `tools/` domain and
add one `tasks.toml` entry with its purpose, side effects, platforms, audience,
entrypoint, and prerequisites. If a file is a private helper, classify it in an
`[[internal]]` entry instead. Then route operator and CI callers through the task
ID and run `./tools/prns verify`; unregistered implementations, missing files,
retired root scripts, invalid syntax, and direct CI bypasses fail verification.

Validation retains its separate interface because proving the product and
mutating/building the product are different safety domains:

```console
python3 validation/run.py verify
python3 validation/run.py run --suite registry
```
