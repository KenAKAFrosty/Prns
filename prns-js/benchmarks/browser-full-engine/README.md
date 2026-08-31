# Browser full-engine benchmark

This opt-in Chromium benchmark compares `PortableWasm`, `WebCrypto`, and
`ParallelWorkers` crypto execution through a complete two-engine browser
journey. Each browser node runs in a DedicatedWorker, connects through the
loopback WebSocket relay, discovers the other node's registered destination,
establishes an encrypted link, sends link packets and Resources, receives
delivery evidence, settles the public promises, and observes the resulting
projections.

Run it from `prns-js` after building the local N-API and WASM artifacts:

```sh
npm run bench:browser-full-engine
```

The timed command path includes the public TypeScript API, engine-Worker
crossings, WASM engine command execution, packet construction and encryption,
browser WebSocket transport, peer Prns ingress, delivery receipt generation,
return transport, engine settlement, projection/event work, and the final
JavaScript promise settlement. It therefore answers a different question from
`browser-worker-wire`: the wire benchmark isolates scheduling and
representation costs, while this benchmark measures their effect inside a
working Prns system.

Payloads and command arrays are allocated before timed regions. Every execution
warms before measurement, order rotates across three repetitions, and summaries
report medians. The sequential workload measures latency-sensitive use; the
coalesced and bounded workloads exercise same-turn command batching. A separate
contention workload overlaps link delivery with 4,096 snapshot requests.
Resource runs cover 1, 2, and 4 MiB payloads and report browser availability,
assembly, settlement, relay, verification, and event-loop timing.

The 2026-08-30 Linux run on a 12th Gen Intel Core i7-1260P with headless
Chrome 151 measured startup at 72.7 ms for `PortableWasm`, 85.5 ms for
`WebCrypto`, and 163.8 ms for `ParallelWorkers`. Path discovery remained about
260 ms and link establishment about 4 ms in all three modes. Established-link
command workloads were within a few percent because they contain little
protocol verification work. For 2 MiB Resources, `ParallelWorkers` settled at
31.7 MiB/s versus 27.9 MiB/s for `WebCrypto`; at 4 MiB they were effectively
equal at 31.5 and 31.8 MiB/s. Those results establish end-to-end correctness and
the expected startup trade, while leaving throughput claims scoped to this host
and workload.

A same-host follow-up removed per-link browser seal serialization and fused the
common seal-plus-digest and open-plus-digest Worker jobs. The two-segment 1 MiB
case remained effectively neutral at 21.7 MiB/s versus 21.6 MiB/s before. The
three-segment 2 MiB case measured 24.4–27.2 MiB/s across two post-change runs,
up from 18.2 MiB/s, and the five-segment 4 MiB case measured 33.1–34.4 MiB/s,
up from 30.3 MiB/s. Platform crypto throughput varied materially between runs,
so these are scoped end-to-end findings rather than primitive-speed claims.
