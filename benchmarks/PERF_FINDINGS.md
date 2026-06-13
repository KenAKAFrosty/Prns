# Perf findings — the forwarding/throughput path

A micro-win backlog from profiling the **crypto-light per-packet path** (where
incremental overhead stacks), to apply as the engine settles. Capture method and
tools are in [`PROFILING.md`](./PROFILING.md). Re-run after any change with the
iai gate (below) so wins don't silently erode.

## Capture

- **Scenario:** `link-firehose-chain` via `./run_chain.sh self self self` — a
  Link stream (handshake amortized) carried through 5 pure-forwarding relay hops.
- **Target:** a **chain (relay) node** — forwards every packet with **zero
  per-hop crypto**, so every sample is pure engine + framing overhead.
- **Tool:** `perf record --call-graph dwarf` (text-analyzable; samply is the
  interactive equivalent), 40,126 samples over a 12s steady-state window.
- **Baseline:** 18,842 pkt/s, 4.52 MB/s goodput end-to-end through the 5 hops,
  payload 60–420 B.

Self-time is split between **our code** (fixable), the **tokio async floor**
(~6%, architectural only), and the **kernel TCP/skb floor** (~10%, transport).

## Hit-list (relay self-time)

| # | Cluster | self-time | Fix | Lands in |
|---|---|---|---|---|
| 1 | **Dedup structure** | **~9%** | BTreeMap → open-addressing Lemire side-index | `routing/dedup` |
| 2 | Framing + serve loop | ~5.8% serve + ~3.7% encode/decode + ~1.1% memmove | SIMD escape-scan; drill serve/memmove; forward-without-re-encode? | `interfaces/framing`, `reactor/interfaces` |
| 3 | Per-packet bookkeeping | ~1% | cache running totals; lighter on relay role | `reactor` |

Per-packet vs per-byte: hashing + frame-scan + copy scale with **payload bytes**
(bigger MTU won't help — kill the work); dedup-insert + `classify` + ledgers are
**per-packet** (MTU amortizes them).

## #1 — Dedup: BTreeMap → open-addressing Lemire side-index (the headline win)

The transport loop-prevention history is the single biggest engine cost on the
relay:

- `HeapPacketHashHistory::remember` **4.77%** self
- `BTreeMap::insert` **1.28%** (+ `insert_recursing` 0.22%)
- `__memcmp_avx2` **2.83%** — the BTreeMap comparing hash bytes at *every* tree
  level, for a key that is *already* a uniform hash
- (+ SHA-256 of the packet ~1.9% — stays, see parity)

**Current:** a generation-rotation scheme over a `BTreeMap` (`generation_capacity`
~500k; current generation rotates to previous when full — that is its sliding
window).

**Fix:** the dedup is a *set of hashes* (no columns), so adopt the building block
the path table already proved in
[`routing/storage/impls/heap_route_columns.rs:60–127`](../personal-rns/src/routing/storage/impls/heap_route_columns.rs)
(`bucket` / `index_of` / insert / resize):

- Two open-addressing generations (current + previous), each an array of hash slots.
- `bucket = ((packet_hash_bits as u128 * len as u128) >> 64)` — Lemire
  multiply-shift on the packet hash's **own bits**, no re-hash (no SipHash/FxHash).
  Power-of-two `len` can mask instead.
- Full-hash equality on probe, so a bucket collision never false-"seen"s a
  distinct packet (which would silently drop it).
- **Rotate-on-full stays** — preserves the exact windowed-eviction *policy*.

This removes `remember` + `insert` + the `memcmp` (~9%) and reuses an existing
pattern rather than inventing one. The `fixed_*` backend can keep its linear scan
(wins at small N), mirroring `fixed_array_route_columns`.

**Parity constraints (do not change):** the SHA-256 *packet hash* itself
(reference-defined) and the dedup *window policy* (generation capacity / what
counts as "seen") — change only the structure that stores it.

## #2 — Framing + serve loop (~10% cluster, partly shaveable)

- `rns_serial_framing::encode` **2.28%** and `RnsSerialDecoder::feed` ~1.44% are
  byte-by-byte HDLC octet-stuffing → candidate for a bulk/SIMD escape-scan
  (memchr-style "find next FLAG/ESC").
- `framed_stream::serve` closure **5.82%** self is the per-frame serve
  orchestration — needs a call-graph drill to separate shaveable framing logic
  from necessary async-read orchestration. **TODO: `perf report -g` on the serve
  closure + the `__memmove` (1.09%) to pin the copy.**
- Worth asking whether a relay must fully decode→re-encode, or can forward the
  framed bytes more directly (zero-copy forward).

## #3 — Per-packet bookkeeping (~1%, recomputation)

`WindowRing::total` **0.52%** is re-summed every packet — keep an incremental
running total. `ThroughputLedger::record_tx/rx/rates` + `AirtimeLedger::record_tx`
~0.45% — cheap to make incremental, and a pure relay may not need full accounting.

## The floor we don't own (~16%)

tokio async (~6%: wakers / semaphores / mpsc push-pop) and kernel TCP/skb (~10%).
Not micro-edits — the lever is **architectural**: batch multiple frames per wake
(fewer channel ops / wakeups per packet), and UDP/io_uring shift the syscall
floor. Track separately from the micro-wins above.

## Regression gate

Add a deterministic **iai forwarding micro-bench** (ingest a data packet on a
transport node, measure route + dedup + re-emit instruction count) alongside the
SINGLE-stage benches in `engine_cycle_iai`. Instruction counts are bit-exact, so
the dedup/framing wins become tracked numbers that can't silently regress.

## Status

All fixes land in `personal-rns` (dedup / framing / reactor). Apply once the
in-flight engine-storage work syncs to `main`, each paired with the iai gate.
