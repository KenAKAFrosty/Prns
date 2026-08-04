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
full commit is kept in the footer title text. The release candidate process
packages that exact commit directly into the hosted website as `source.zip`
plus `source.zip.sha256`; ordinary Dioxus and embedded-site builds never write
or inherit those release artifacts.

The ZIP is the one authoritative full-repository source snapshot. It includes
the website implementation under `docs/website/` and the NomadNet page source
under `personal-hopspot/core/src/node_pages.rs` and
`personal-hopspot/core/src/node_pages/`. Candidate validation regenerates the
ZIP from the stamped commit byte-for-byte, checks its SHA-256 sidecar, and
requires both source areas before signing. To package the current checkout
manually:

```sh
./tools/prns release source package -- --output target/source.zip
```

After the Rust toolchain is installed, the equivalent Cargo convenience command
is:

```sh
cargo tools release source package -- --output target/source.zip
```

Official candidate creation performs this packaging before any website, browser
playground, or firmware release build. It also writes
`metadata/source.json`, containing the canonical version, full commit, byte
length, SHA-256, and NomadNet routes. Source-enabled consumers receive that
identity through `PRNS_SOURCE_ARCHIVE`, `PRNS_SOURCE_VERSION`,
`PRNS_SOURCE_COMMIT`, `PRNS_SOURCE_SIZE`, and `PRNS_SOURCE_SHA256`; enabling the
`source-archive` Cargo feature without all five matching values fails the build.

Heltec V4 and T-Beam Supreme candidates first build the compact application and
preflight its measured size plus the archive, a 64 KiB embedding allowance, and
the mandatory 1 MiB factory-partition reserve. A passing target is then rebuilt
with the archive as one flash-backed static, shared by SoftAP `/source.zip` and
the NomadNet `/file/source.zip` route. The completed source-enabled image is
measured again. If either capacity check fails, the builder keeps the compact
image, serves the non-source NomadNet page, omits its source metadata, and
records `capacity-downgrade` in `metadata/source-capabilities.json`. XIAO
ESP32-C6 and T-Echo always use the non-source route set and do not compile the
named-file response branch or larger outgoing window. Source-enabled browser,
desktop, Android, and iOS release builds use the same feature and archive
identity; ordinary development and hot reload builds leave the feature off.
Native transfers retain only the static flash/archive borrow and continuation
offsets between proofs. They copy, encrypt, transmit, and retry at most one
256 KiB plaintext segment, then reuse that window after its proof; the full ZIP
is never copied into RAM.

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
| `prnsd logs` | Show recent output and follow the running daemon; return status 3 when stopped |
| `prnsd stop` | Show recent output, request graceful shutdown, and follow the final logs |
| `prnsd run [OPTIONS]` | Run in the foreground for a terminal or native service manager |
| `prnsd i2p doctor` | Check I2P router and SAM 3.1 readiness without starting the managed daemon |
| `prnsd i2p setup` | Print guided platform installation, SAM enablement, and a validated interface stanza |
| `prnsd interfaces [COMMAND]` | Guided typed interface editing, grouped validation and repair, and explicit live apply |

`prnsd status` is the prefixless RNS network-status utility, not a managed-process status command.
It and the other RNS 1.4.2-compatible one-shot utilities are documented in
[`docs/prnsd-utilities.md`](prnsd-utilities.md).

`prnsd` and `cargo prnsd` share one per-user managed session. Repeated starts
reattach without starting another process, and Ctrl-C detaches without stopping
the daemon. Set `PRNSD_STATE_DIR` to create an isolated session for testing or
advanced multi-instance use.

Release builds include the default `tray` feature. Once the daemon is ready,
the Prns mark appears in the macOS, Windows, or Linux system tray. Its menu
shows live interface health, opens an attached Prns terminal, runs network
status or the guided interface editor, reveals the effective configuration
folder, and stops the daemon through the normal graceful shutdown path,
including the final persistence flush. Direct `prnsd run` sessions are labeled
as foreground sessions and do not offer managed-log attachment. A missing
desktop session or Linux StatusNotifier watcher only disables the tray and
records `tray_unavailable`; it does not prevent `prnsd` from running.

Native service packages and other deliberately headless builds can omit the UI
dependencies:

```sh
cargo build --manifest-path prnsd/Cargo.toml --release --no-default-features \
  --features tokio-host,observability
```

The official container uses the narrower `tokio-cloud-host` profile, mandatory
persistence, and a digest-pinned multi-architecture image. Native archives,
container operation, Railway publication, backups, rollback, SBOMs, signatures,
and provenance verification are documented in
[`docs/deploy-prnsd.md`](deploy-prnsd.md).

The separate public `ghcr.io/kenakafrosty/prnsd-staging` package supports live Docker and Railway rehearsal from an exact protected `main` commit before release readiness. Its immutable commit tags and staging evidence are intentionally outside release custody and cannot satisfy suite signing or promotion.

The unified suite retains the flasher's established physical-acceptance
boundary. The protected suite public review, signed physical acceptance and
flasher release record, and protected deployment qualification remain
independent gates; stable promotion verifies all of them before moving the
GitHub Release or GHCR tags.

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
