# Fixed link deadline storage checkpoint

Local macOS arm64 candidate based on `b42a87724`, 2026-09-26.

The full checkpoint matrix passed the repaired T-Echo builds, then found T096
60 bytes short of its unchanged 69,632-byte runtime-stack reservation. The same
failure reproduced with the previous Resource code-size repair removed. Compared
with the prior passing linker map, the retained node had grown by 64 bytes.

## Shared-core repair

`FixedLinkTable` stores deadline values and presence flags in separate columns,
instead of paying alignment padding for every `Option<InstantMillis>`. Presence
is explicit: neither zero nor `u64::MAX` is a sentinel. Set, insertion, removal,
and reused slots preserve row alignment. No capacity or timeout policy changes.

The storage extension trait now exposes `timeout_at(index)` instead of borrowing
an `Option` slice. Fixed-heap and dynamic-heap implementations retain their
existing representation; the dynamic heap's indexed timeout search is unchanged.
Default scans retain their original ordering and predicate semantics.

The split-delivery callback also shares an outlined borrowed trait-object
boundary, matching whole delivery. It allocates nothing and preserves event
content/order, but makes the event call indirect. Deadline compaction alone put
T-Echo v7 160 bytes over flash; sharing split delivery restored 108 bytes of
headroom. No compiler profile, board feature, partition or stack guard was relaxed.

## Evidence

- 26 link-table tests, including whole-row property checks across insertion,
  full-table refusal, updates, cancellation, removal, reuse, predicate filtering,
  and earliest-deadline selection. Explicit boundaries cover zero and `u64::MAX`.
- A layout assertion requires at least 224 bytes saved for 32 fixed link slots.
- 512 routing-link tests; root tests; root workspace tests; core all-target clippy.
- `./tools/prns build embedded resources report --all --platform nrf52840`
  passed every Nordic target, including both T-Echo variants, T114, T096,
  both MeshPockets, RAK4631, T1000-E, MeshTower V2, and muzi Base Duo.

T096 retains 143,192 static RAM bytes, with 164 bytes beyond the existing stack
reservation: exactly 224 bytes recovered from the failing layout. T-Echo v7
retains 141,280 static RAM bytes and its image occupies 622,484 bytes. These
margins remain tight and are reported honestly, not treated as comfortable slack.

This is memory-layout and firmware-size evidence. The earlier whole-Resource
microbenchmark does not measure this later change or establish split-transfer
performance equivalence. No physical hardware result is claimed. The full push
verification ladder still has to pass on the committed checkpoint.
