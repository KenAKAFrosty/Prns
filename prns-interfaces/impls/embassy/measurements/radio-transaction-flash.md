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

## T-Echo S140 7.3.0 and the Nordic packet pool

The next comparison uses `4df6b4603e708982031a2e4675f121b8dc22e554`,
the same compiler and the registered `t-echo-s140-v7` recipe, with no profile,
RUSTFLAGS, FLASH or RAM limit changes. The prior build exceeds FLASH by
3,936 bytes.

```console
CARGO_BUILD_JOBS=2 ./tools/prns build embedded resources report --target t-echo-s140-v7
```

The SX126x band adapter initializes directly through the existing validated
configuration and driver operation, rather than nesting the legacy async
adapter. This saves 352 bytes in the measured FLASH overflow. Tests compare
the complete initialization command trace with the legacy adapter and check
that an unsupported band is rejected before SPI access.

The Nordic backend's [L2CAP pool](../../../../personal-hopspot/embedded/nrf52840/src/runtime/bluetooth_auto/l2cap_pool.rs)
uses zero-initialized claimed flags instead of initially true free flags.
The 4,527-byte pool consequently moves from initialized `.data` to zeroed
`.bss`. All nine 502-byte packet buffers remain available, with the same
atomic ordering and exclusive slot ownership. Static lifetime is required
when claiming a slot so outstanding packet pointers cannot outlive storage.
The adapter and pool changes together pass the unchanged FLASH guard.

| Allocated section | Prior implementation, bytes | Direct adapter and zeroed pool, bytes |
| --- | ---: | ---: |
| `.text` | 544,428 | 544,148 |
| `.rodata` | 62,108 | 62,108 |
| `.data` | 19,668 | 15,140 |
| `.bss` | 121,584 | 126,112 |
| `.vector_table` | 256 | 256 |

The pool changes initialization storage, not total static RAM. The allocator's
host component tests exercise every slot, exhaustion, release and reuse, and
simultaneous packet claims. They compile the production module directly:

```console
cargo test --locked -p prns-simulation --test nrf_l2cap_pool
cargo test --locked --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features lora-2g4 radios::sx126x::tests
```

These measurements do not establish runtime peak stack usage, RF behavior, or
throughput. Transaction support remains enabled on every configured radio
board; the full configured firmware resource matrix remains authoritative.
