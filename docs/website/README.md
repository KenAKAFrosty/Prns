# Website Development

The Dioxus website is the public site: landing page, platform matrix, web
flasher, and benchmark results. The benchmark pages embed the canonical
`benchmarks/RESULTS*.md` files with `include_str!`, so editing those results
in their owning directory changes the site; repository guides are linked at
their canonical GitHub locations rather than mounted.

## Check and run

The site pins Dioxus CLI 0.7.5:

```console
./tools/prns doctor docs
cargo run -p docs
```

(On Windows, run the doctor as `.\tools\prns.cmd doctor docs`.)

The root `docs` package starts the local development surface. For direct Dioxus
development from this directory:

```console
dx serve
```

First-time Rust or Dioxus dependency downloads may require network access. Once
present, the essential guide content comes from the repository.

The npm manifest overrides Tailwind CLI's pinned `@parcel/watcher` with 2.6.0,
which removes the vulnerable `micromatch`/`braces` chain
([GHSA-vfj7-8cjw-p6xm](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm)).
Remove the override when Tailwind selects a watcher without that dependency.

## Device cards

The flasher lists ThinkNode G4 and Heltec HT-HD01-V2 individually alongside the
firmware boards, with their existing product photos. Their **Set up** actions
open the current device guides; preview cards do not claim browser flashing or
a signed public download.

The 0.3.8 release catalog enables Flash for Vision Master E290-HF, Wireless Stick
Lite V3, both MeshPocket capacities, RAK WisBlock 4631, muzi Base Duo, and
MeshTower V2 under the automated pre-1.0 acceptance policy.

MeshTower V2 currently assumes the stock `HT-n5262` recovery volume and Board-ID,
with S140 6.1.1. The volume and SoftDevice are documented in [the original
firmware PR](https://github.com/KenAKAFrosty/Prns/pull/122); the Board-ID is present
in [Heltec's published 0.9.0 bootloader](https://github.com/HelTecAutomation/Heltec_nRF52/tree/main/bootloader/HT-n5262).
The release owner accepted this mapping for 0.3.8 pending Tony's device-level
`INFO_UF2.TXT` confirmation. It is an explicit assumption, not a hardware test
receipt. The shared identity cannot distinguish MeshTower, T114, or MeshPocket;
the public flasher requires model confirmation and a matching SoftDevice.
MeshTower's release build preserves the thin-LTO setting of its developer build.

## Test

```console
cargo test --manifest-path docs/website/Cargo.toml
cargo check --manifest-path docs/website/Cargo.toml
```

The tests verify canonical benchmark-results inclusion and link rewriting,
generated benchmark routes, the flash catalog contract, and the platform
claims the site is allowed to make.

The T1000-E flasher also offers **Switch firmware → Enter recovery mode**
without preparing an install. It requests the stock UF2 drive from compatible
Hopspot firmware, reports acknowledgement without claiming drive enumeration,
and provides manual recovery and Meshtastic restore instructions. See
[T1000-E recovery](../../personal-hopspot/embedded/nrf52840/RECOVERY.md) for
firmware compatibility, the USB contract, and physical qualification limits.

## Static production build

The production site uses Dioxus fullstack static-site generation. The server
binary exists only while DX discovers and renders the declared static routes;
the deployable artifact is still a static `public` directory.

```console
dx build --web --ssg --force-sequential true --release --locked
cargo run --locked --bin finalize_ssg -- target/dx/reticulum-site/release/web/public
```

The finalization command does not render pages. It verifies Dioxus's output,
copies the rendered `/404` route to the host-standard root `404.html`, and
writes `sitemap.xml` from the same typed route inventory used by SSG.
Sequential mode is deliberate: it prevents the client bundle from racing and
overwriting an already-rendered root page when local builds run both targets in
parallel.

## Hosted boundary

The default website includes the platform matrix, the web flasher, benchmark
results, and the browser playground, and links out to repository guides and
crate READMEs. A release build also advertises its source archive and
checksum after the release process stages those files. An ordinary local
development server does not claim that an unstaged archive exists.

The embedded Hopspot captive page is a separate static firmware asset under
`personal-hopspot/embedded/esp32`; it does not embed this Dioxus application.

Release builds use the repository's named release tasks and set source identity
from the staged candidate. Local development does not manufacture that identity.
See
[Repository tools](../../tools/README.md) and
[Release guidance](../release.md).
