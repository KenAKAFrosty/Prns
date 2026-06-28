# Release and version policy

Prns is pre-1.0 software. The current suite version lives in the repository
root [`VERSION`](../VERSION) file and is stamped into the docs site together
with the exact source commit.

## Build provenance

Release builds should stamp both a version and a commit:

- `PRNS_BUILD_VERSION`: overrides the value read from `VERSION`.
- `PRNS_BUILD_COMMIT`: overrides the full source commit detected by `git`.
- `PRNS_BUILD_COMMIT_SHORT`: overrides the displayed short commit. When it is
  not set, the docs build derives it from the full commit.

The docs footer displays the public version and the short source snapshot. The
full commit is kept in the footer title text, and the source ZIP plus
`source.zip.sha256` remain the reproducible source artifacts.

## Pre-1.0 semver

Cargo treats compatibility for `0.y.z` releases around the left-most non-zero
component. Prns follows that convention:

- `0.1.z` means compatible fixes or additive, low-risk public API changes.
- `0.2.0`, `0.3.0`, and later `0.y.0` releases may break public Rust APIs,
  feature flags, wire-adjacent contracts, or host integration points.
- `0.0.z` is reserved for scratch/internal packages that should not carry a
  public compatibility promise.
- `1.0.0` waits until the core engine API, feature selection, daemon boundary,
  and published crate set are stable enough to support as boring defaults.

## Crates and artifacts

Crates keep explicit `version` fields in their own `Cargo.toml` files. The
suite version in `VERSION` should match the primary public crate release unless
there is a deliberate crate-specific release.

Flash artifacts use the same build version by default when their manifest entry
still says `version = "next"`. Release jobs may set `PRNS_FLASH_VERSION` when a
firmware artifact intentionally needs a different prerelease or patch version.

Keep `publish = false` on crates until the release checklist for that crate is
complete. The first public cargo publish should include an audited manifest,
README, license metadata, feature list, docs.rs behavior, and a tag that points
at the exact source snapshot displayed by the docs site.
