# go-reticulum — announce-256 driver

Measures [go-reticulum](https://github.com/svanichkin/go-reticulum) (Go) on the shared
`announce-256` corpus: `NewPacket` + `Unpack` + `ValidateAnnounce` (the Ed25519 verify +
store), best-of-50 min wall time. Conformance is the count of announces that validate
(`ValidateAnnounce == true`), identical to the RNS reference's `resolved` metric.

## Run

```sh
./run.sh
```

Needs Go (≥ 1.26) on `PATH`. Clones the pinned upstream into `.upstream/` (gitignored),
drops `main.go` into a subpackage, `go run`s it, and writes
`../../results/<host>/announce-256/go-reticulum.jsonl`.

- **Upstream:** https://github.com/svanichkin/go-reticulum @ `06621cc`
- **License:** MIT — we vendor only `main.go` (our code) + the numbers.
- **Crypto backend:** Go stdlib `crypto/ed25519` (arm64/amd64 assembly).

## Parallel scenario

`./run-mt.sh` measures the `announce-parallel` scenario — 2560 distinct announces sharded
across `[1, runtime.NumCPU()]` goroutines (the verify parallel; the store serialized behind
go-reticulum's global mutex), single-thread vs all logical cores — and writes
`../../results/<host>/announce-parallel/go-reticulum.jsonl`.
