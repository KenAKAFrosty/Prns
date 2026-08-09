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

## Test

```console
cargo test --manifest-path docs/website/Cargo.toml
cargo check --manifest-path docs/website/Cargo.toml
```

The tests verify canonical benchmark-results inclusion and link rewriting,
generated benchmark routes, the flash catalog contract, and the platform
claims the site is allowed to make.

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
