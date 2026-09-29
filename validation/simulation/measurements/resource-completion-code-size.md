# Resource completion code-size checkpoint

Local macOS arm64 comparison against `8153aece4`, 2026-09-26.

The checkpoint push caught a firmware flash overflow after the accumulated
response-ownership fixes. T-Echo S140-v6 exceeded FLASH by 1,536 bytes.
Removing the latest dependent-wake refresh experimentally still overflowed by
1,280 bytes; that correctness fix was restored and remains intact.

## Retained change

Keep Resource conclusion and whole-segment delivery out of callers' inline
expansions. Whole delivery shares its callback boundary through a borrowed
trait object; it allocates nothing, but uses indirect event calls. Split assembly
advancement returns a small named conclusion independent of the callback type;
the adapter emits the same terminal journal. Assembly state is cleared before
emitting its completion, while the engine remains exclusively borrowed.

No wire format, admission rule, buffer capacity, board feature, partition, stack
reservation, or timeout policy changes. This is shared-core code-size work,
not a new simulator-discovered protocol failure.

## Measurements

Canonical firmware commands use the configured builder environment, without
host Cargo overrides:

```console
./tools/prns build embedded resources report --target t-echo-s140-v7
./tools/prns build embedded resources report --target t114
```

The T114 exact-prior/candidate comparison is 625,292 / 619,500 image bytes:
5,792 bytes saved. Both retain 142,996 static RAM bytes, 360 bytes remaining
after the existing 69,632-byte stack reservation, and a 59,280-byte largest
modeled direct-call chain (10,352 bytes inside the reservation). These are
partial static stack measurements, not a full worst-case stack proof.

The candidate T-Echo S140-v7 image is 622,540 bytes, with only 52 bytes of
flash headroom. It fits, but the remaining margin is explicitly tight.
Static RAM is 141,504 bytes with 1,852 bytes remaining after reservations.

Outlining conclusion alone fixed v6 but left v7 1,376 bytes over. Outlining
whole delivery reduced that to 416 bytes. Separating split advancement from
the callback reduced it to 96 bytes; sharing whole delivery's callback then
linked. Other outlining experiments that increased size or made no difference
were removed. No contract or compiler-profile relaxation was used.

## Host performance smoke

Built `resource_profile` separately from the exact prior source and candidate:

```console
CARGO_INCREMENTAL=0 cargo build --locked --release --manifest-path benchmarks/Cargo.toml --example resource_profile
```

Saved binaries were run sequentially in prior/candidate/candidate/prior/prior/
candidate order, with no competing verification builds during measurement.
Arguments were `256 1048575 8` and `20000 2048 64` (transfers, payload bytes,
warmup transfers). Wall times in milliseconds:

| Payload | Prior samples | Candidate samples |
| --- | --- | --- |
| 1,048,575 bytes | 662.635, 640.595, 640.770 | 681.996, 649.055, 645.065 |
| 2,048 bytes | 201.455, 176.772, 172.397 | 171.591, 171.635, 175.291 |

Median wall time changes are approximately +1.3% and -2.9%, respectively.
Frame accounting is identical. This brief engine-only, unsolicited Resource
smoke shows no obvious material slowdown; it does not establish embedded cycle
cost, segmented-request throughput, or end-to-end performance equivalence.

Focused Resource receive tests pass (142 tests). Broader checkpoint verification
runs through the normal root/workspace and pre-push ladder; the initial matrix
attempt stopped at the overflowing v7 target and is not a full-matrix pass.
