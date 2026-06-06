# External-implementation drivers

The `announce-256` comparison includes other Reticulum implementations, measured the same
way as ours: the same wire bytes (`../scenarios/announce-256/packets.hex`) replayed through
each impl's real parse → Ed25519 verify → store path, best-of-50 min wall time, on the same
host. announce-256 is ~97% Ed25519 verify, so the spread is a crypto-backend story.

Each `<impl>/` holds **our** harness + a one-command `run.sh` + a README. We never vendor
upstream source — `run.sh` clones the **pinned** upstream into a gitignored `.upstream/`,
builds our harness against it, and writes rows into
`../results/<host>/<scenario>/<impl>.jsonl` in the same schema every driver emits. So:

```sh
cd benchmarks && ./external/leviculum/run.sh
```

reproduces that one column from a clean checkout. A sibling **`run-mt.sh`** reproduces the
impl's `announce-parallel` row (the same corpus sharded across threads, single-thread vs all
logical cores) the same way. Implementation metadata (language, crypto backend, repo, pinned
ref, license) lives in `../implementations/<slug>.json`; the rendered comparison table joins
the two. `lib.sh` holds the shared clone/emit helpers.

## Adding an implementation

1. Write a harness that replays the corpus through the impl's validate-announce path and
   prints `RESULT resolved=<n> per_sec=<f>` (parse + verify + store, best-of-50).
2. Write a `run.sh` that sources `lib.sh`, `clone_pinned`s the upstream into `.upstream/`,
   builds + runs the harness, and calls `emit_rows`.
3. Add `../implementations/<slug>.json` (language, crypto_backend, role, repo, pinned_ref,
   license; `maturity: "partial"` if the upstream list marks it not-yet-complete).
4. Run it, then `cargo run --bin render_results` to refresh the tables.

For the parallel scenario, add a `run-mt.sh` + harness that sweeps `[1, cpu_count]` threads and
prints `RESULT resolved=<n> lo=<t> lo_per_sec=<f> hi=<t> hi_per_sec=<f>`, then calls `emit_mt_rows`
(pass `announces_verified` as its last argument if the harness is verify-only).

Numbers are comparable only within a host; toolchain versions are stamped per row. Licenses
vary (AGPL, MIT, Apache, EPL, …) — we distribute only our harness source and the measured
numbers, never upstream code.
