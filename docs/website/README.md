# Website Development

The Dioxus website is both the public site and the clone's local documentation
reader. Its hosted/default build mounts canonical repository Markdown with
`include_str!`; editing a guide in its owning directory changes both views.

## Check and run

The site pins Dioxus CLI 0.7.5:

```console
./tools/prns doctor docs
cargo run -p docs
```

The root `docs` package starts the local development surface. For direct Dioxus
development from this directory:

```console
dx serve
```

First-time Rust or Dioxus dependency downloads may require network access. Once
present, the essential guide content comes from this clone.

## Test

```console
cargo test --manifest-path docs/website/Cargo.toml
cargo check --manifest-path docs/website/Cargo.toml
```

The tests verify canonical source inclusion, unique guide slugs, relative link
resolution, fragment preservation, and generated benchmark routes.

## Hosted and embedded boundaries

The default website includes guides, crate READMEs, benchmark results, the
browser playground, and release source downloads. The `embedded-site` feature
is the compact SoftAP bundle for constrained firmware:

```console
cargo check --manifest-path docs/website/Cargo.toml --features embedded-site
```

Repository guide modules are compiled only for the default site. Do not add
their content, dependencies, or routes to `embedded-site`.

Release builds use the repository's named release tasks; local development
must not manufacture release source identity. See
[Repository tools](../../tools/README.md) and
[Release guidance](../release.md).
