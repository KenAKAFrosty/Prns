# External implementations — pinned upstream clones

The energy comparison ([`../energy/`](../energy/)) measures six other Reticulum ports against
ours on the same wire corpus. We never vendor their source — instead `energy/build.sh` clones
each **pinned** upstream into a gitignored `<impl>/.upstream/` here and builds our sustained
harness (in `energy/contestants/<impl>/`) against it:

| Port | Language | Upstream | Pin | License |
|------|----------|----------|-----|---------|
| Leviculum | Rust | https://codeberg.org/Lew_Palm/leviculum | `6f366ca` | AGPL-3.0 |
| LXMF-rs | Rust | https://github.com/FreeTAKTeam/LXMF-rs | `30da190` | EPL-2.0 |
| go-reticulum | Go | https://github.com/svanichkin/go-reticulum | `06621cc` | MIT |
| rns-cr | Crystal | https://github.com/jtippett/rns-cr | `514c309` | MIT |
| microReticulum | C++ | https://github.com/attermann/microReticulum | `79b8524` | Apache-2.0 |
| RetiNet | Python | https://codeberg.org/skyguy/retinet | `6039094` | AGPL-3.0 |

`lib.sh` is the shared clone helper (`clone_pinned`). What each implementation *is* — language,
Ed25519 backend, repo, pinned ref, license — lives once in `../implementations/<slug>.json`,
which the rendered table joins for its Language/backend columns and provenance.

To add a port: drop a sustained harness in `energy/contestants/<impl>/`, add its clone + build
step to `energy/build.sh` and its row to `energy/measure.sh`, and an `implementations/<slug>.json`.
See [`../energy/README.md`](../energy/README.md).
