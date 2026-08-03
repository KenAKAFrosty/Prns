# Synchronized release-readiness baseline

Inventory was repeated on 2026-07-21 after fast-forwarding `main` to
`68e6f4ef97d3f9c63ad4df45d1e933b2208c8ecd`. This is the pre-reorganization
failure record; it keeps environmental and generated-state failures distinct
from product regressions.

| Observation | Baseline result | Disposition in the validation hub |
| --- | --- | --- |
| Root Clippy | Four `clippy::drop_non_drop` errors in resource receive/send closure lifetimes. | Fixed with lexical closure scopes; root Clippy now passes with `-D warnings`. |
| Legacy stock-RNS environments | Both `benchmarks/reference/.venv` and `.rpc-venv` contained RNS 1.3.9, not the then-required 1.4.0. | Ordinary tests no longer discover them. `prepare-oracles` creates ignored, version-checked RNS 1.4.2 environments under `validation/.venv/`; legacy environments are cleanup candidates. |
| Mutation output | Ignored `mutants.out` described obsolete `personal-rns` source paths and polluted filesystem-based drift inventory. | Registry drift uses tracked-source inspection. Mutation output is generated beneath `validation-artifacts/`, and current survivors must match reviewed semantic fingerprints. The stale directory is a cleanup candidate, never release evidence. |
| Comment ledger | `scripts/comment-census.txt` encoded exact comment-line totals, including paths moved by this reorganization, rather than a semantic quality contract. | The census and ledger were deleted. Formatting, Clippy, documentation, registry, orphan, and release-contract gates carry the useful intent. |
| Integration capstones | Parallel execution of live shared-instance capstones produced two different timeout flakes; each test passed in isolation. | The registered capstone command uses `--test-threads=1`; the complete serial workspace passes. |
| Local desktop link | The synchronized macOS host lacked SDL2, so a local desktop link could not complete. | Linux and macOS CI/release lanes install SDL2 explicitly before building the desktop face. |

Post-sync product checks did not expose a Hopspot identity-custody regression:
the Hopspot core suite, root workspace, configuration suite, daemon workspaces,
release contracts, deterministic oracle suite, Kani matrix, and bounded fuzz
matrix all passed locally. Linux live interop, Android hardware, sanitizer/Miri,
and hosted exact-SHA qualification remain evidence that must come from their
registered qualified runners. Fresh mutation triage remains separate scheduled
audit evidence.
