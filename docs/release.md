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

## Build the portable daemon

From the repository root, build the canonical local `prnsd` artifact with:

```sh
cargo prnsd build
```

This performs a locked release build with the optional OTLP support compiled
in and prints the absolute path to the executable. The normal paths are
`prnsd/target/release/prnsd` on macOS and Linux and
`prnsd\target\release\prnsd.exe` on Windows. OTLP export remains inactive
unless an endpoint is configured.

The printed executable is self-managing and can be copied to another location
on the same platform. It does not need Cargo or a repository checkout at run
time. Build options such as `--target` or `--profile` can be supplied after
`cargo prnsd build`; the no-option command is the canonical local artifact.

| Command | Behavior |
| --- | --- |
| `prnsd` or `prnsd start` | Start if needed, show the visual header, and attach to the log |
| `prnsd --detach` | Start if needed, wait for readiness, and return to the shell |
| `prnsd restart [OPTIONS]` | Gracefully replace the managed daemon |
| `prnsd status` | Report `starting` or `running`; return status 3 when stopped |
| `prnsd logs` | Show recent output and follow the running daemon; return status 3 when stopped |
| `prnsd stop` | Show recent output, request graceful shutdown, and follow the final logs |
| `prnsd run [OPTIONS]` | Run in the foreground for a terminal or native service manager |

`prnsd` and `cargo prnsd` share one per-user managed session. Repeated starts
reattach without starting another process, and Ctrl-C detaches without stopping
the daemon. Set `PRNSD_STATE_DIR` to create an isolated session for testing or
advanced multi-instance use.

The default state directories are:

- Linux: `${XDG_STATE_HOME:-~/.local/state}/prnsd`
- macOS: `~/Library/Application Support/prnsd`
- Windows: `%LOCALAPPDATA%\prnsd`

The directory holds the versioned session record, readiness and shutdown
coordination, human and JSON logs, and one rotated predecessor for each log.
On Windows it also holds the managed launch copy, allowing the source executable
to be replaced while the daemon is running. Its files are private to the current
user under the platform's normal permissions.

This portable session survives the launching terminal, but it is not a
boot/login service and does not restart itself after a machine reboot or a
crash. A future launchd, systemd, or Windows Service definition should invoke
`prnsd run` as its foreground process rather than nesting this session manager.

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
