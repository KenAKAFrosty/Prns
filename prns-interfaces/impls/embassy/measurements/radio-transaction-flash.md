# T-Echo radio transaction FLASH measurement

The comparison uses the exact prior implementation at
`f474f4281a686d98066dcb542a39017bb8c93940`, Rust 1.98.0 on macOS ARM64, and
the registered T-Echo S140 6.1.1 recipe. Both builds use the configured release
profile, fat LTO, the original FLASH origin `0x26000`, and its 626,688-byte
application region. No profile or RUSTFLAGS override is applied.

```console
CARGO_BUILD_JOBS=2 ./tools/prns build embedded resources report --target t-echo-s140-v6
```

The prior build fails the FLASH guard by 384 bytes. The changed transaction
borrows the immutable previous configuration rather than copying it into its
nested async state. Variant-only checks use `matches!` instead of command
payload equality. The changed build passes the same guard.

| Allocated section | Prior implementation, bytes | Borrowed state, bytes |
| --- | ---: | ---: |
| `.text` | 544,996 | 544,428 |
| `.rodata` | 62,116 | 62,108 |
| `.data` | 19,668 | 19,668 |
| `.vector_table` | 256 | 256 |

The allocated sections shrink by 576 bytes. The remaining FLASH margin is
small; this is not an additional allocation budget. Compiler versions and
future types can change the result, so the configured firmware resource matrix
remains authoritative. This measurement does not establish runtime stack use,
RF behavior, or throughput.

Existing transaction tests exercise hardware failures, rejection while traffic
is quiesced, restoration of the previous channel, publication failures, and
publication acknowledgement before traffic resumes. Run the radio tests with
both feature configurations:

```console
cargo test --locked --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features lora lora::
cargo test --locked --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features lora-2g4 lora::
```
