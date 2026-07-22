# Repository tools

`tools/` is the single home for commands that build, package, sign, install,
flash, or otherwise operate on the repository and its release products. Checks
and proofs live under `validation/`; Git-triggered integration lives under
`.githooks/`.

Start with:

```console
./tools/prns list
./tools/prns explain release.candidate.build
./tools/prns doctor release
./tools/prns verify
```

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
