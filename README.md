# Personal Reticulum Suite

Fresh Rust workspace for the Reticulum triumvirate:

- `personal-rns` — pure Reticulum engine and wire contract.
- `personal-rnsd` — thin daemon host around the engine.
- `personal-lxmf` — LXMF application layer above `personal-rns`.

The build directive is copied in [docs/build-ethos.md](docs/build-ethos.md).
The short form is:

> Port the contract, not the implementation. Build one pure engine, and let each platform bring a thin host.

The previous in-tree implementations are reference material only and live in
the parent repository under `archive/rns-legacy/`.
