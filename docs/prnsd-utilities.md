# Prnsd Utilities

Prnsd provides the stock RNS 1.3.8 utility roles as direct, prefixless subcommands. From a source
checkout, invoke them through `cargo prnsd`; an installed daemon exposes the same commands directly
through `prnsd`.

| Prnsd command | Stock utility role | Purpose |
| --- | --- | --- |
| `prnsd status` | `rnstatus` | Show interface, transport, traffic, announce, path-request, and link status |
| `prnsd path` | `rnpath` | Inspect paths and rates or perform supported path and blackhole management operations |
| `prnsd probe` | `rnprobe` | Measure delivery and round-trip behavior to a destination |
| `prnsd id` | `rnid` | Create, inspect, import, export, sign, validate, encrypt, decrypt, announce, and request identities |
| `prnsd cp` | `rncp` | Send, receive, and fetch files over Reticulum |
| `prnsd x` | `rnx` | Request command execution and optionally serve an execution endpoint |

The public command names are intentionally prefixless. Stock names remain only where interoperability
requires them, such as destination aspects, request paths, identity allow-list locations, and wire
formats. Each command's `--help` output is the source of truth for its stock-compatible option
surface.

## Shared-instance boundary

`status`, `path`, `probe`, `cp`, and `x` are one-shot clients of an already running local shared RNS
instance. They do not start or stop the managed daemon. Start Prnsd separately, then run a utility:

```sh
cargo prnsd --detach
cargo prnsd status
cargo prnsd path --table
```

Most `id` operations are local and do not need a daemon. Identity requests and announcements attach
to the shared instance because they use the network.

`cargo prnsd status` therefore reports RNS network state. Managed-process operations remain
`cargo prnsd`, `cargo prnsd logs`, `cargo prnsd restart`, and `cargo prnsd stop`.

## File transfer security

Start a receiving endpoint with `cp --listen`. The default accepts no remote identity until one is
explicitly allowed with `-a` or a stock-compatible `allowed_identities` file. `--no-auth` makes the
receiver public and must be an explicit choice.

Remote fetching is disabled unless `--allow-fetch` is supplied. Use `--jail PATH` to constrain fetch
requests to a canonical directory boundary. Authentication and the fetch jail are independent: a
public receiver with fetching enabled should still use a jail unless unrestricted file access is
deliberate.

```sh
prnsd cp --listen -a 00112233445566778899aabbccddeeff --save ~/received
prnsd cp report.bin DESTINATION_HASH
```

## Remote execution security

Start an execution endpoint with `x --listen`. Its default also permits nobody. Supply each allowed
requester with `-a` or a stock-compatible `allowed_identities` file. `--noauth` deliberately exposes
command execution without requester authentication; Prnsd never enables it implicitly.

The serving side uses the same RNX request and result protocol in the reusable `personal-rns`
runtime. Tokio hosts can opt into the process-command handler used by the CLI. Embedded and other
hosts can plug in their own typed handler without providing a shell, process API, or terminal.

```sh
prnsd x --listen -a 00112233445566778899aabbccddeeff
prnsd x DESTINATION_HASH "uname -a"
```

`--noid` suppresses client identification and is distinct from the listener's `--noauth` policy.

## RNS 1.3.8 compatibility

The suite tracks stock RNS 1.3.8 behavior at the CLI and protocol boundaries. The automated Python
oracles run the RNS 1.3.9 security release, whose utility wire semantics remain compatible with that
target. Where a utility has two roles, the tests exercise both directions:

```sh
bash scripts/rnstatus-interop-smoke.sh
bash scripts/rnpath-interop-smoke.sh
bash scripts/rnprobe-interop-smoke.sh
bash scripts/rnid-local-interop-smoke.sh
bash scripts/rnid-network-interop-smoke.sh
bash scripts/rncp-interop-smoke.sh
bash scripts/rnx-interop-smoke.sh
```
