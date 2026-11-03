# LXMF wire library

Use `prns-lxmf-wire` to parse, compose and validate LXMF messages for existing LXMF
peers. LXMF and PRNS groups are separate application protocols; consumers can
select either or both. Wire handling is independent of applications, networking,
storage and React Native.

The `wire/` library is allocation-free and `no_std`. Parsing borrows its input
and applies caller-supplied limits. The pinned Python corpus exercises the
supported wire subset; it does not establish transport delivery or persistence.

## Verification

Run from the repository root:

```sh
cargo test --locked -p prns-lxmf-wire
./tools/prns build lxmf verify
```

The verification script checks the portable build, Rust tests and lints, pinned
Python vectors and vector fixture tests. It requires the
`thumbv7em-none-eabihf` Rust target and Python 3.13.7; `uv` can provision the pinned
interpreter. The test environment defaults to `target/interop/lxmf-1.1.0`;
`LXMF_VENV` can select an existing environment.

## Source provenance

See [wire provenance](wire/PROVENANCE.md) for the original bounded wire source,
pinned Python authority and deterministic fixture corpus.
