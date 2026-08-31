# Browser full-engine benchmark

This opt-in Chromium benchmark compares the browser package's `DedicatedWorker` and `MainThread` executions through a complete two-engine journey. Each browser node connects to a native Prns WebSocket server, discovers its registered destination, establishes an encrypted link, sends link packets, receives delivery evidence, settles the public promises, and observes the resulting projections.

Run it from `prns-js` after building the local N-API and WASM artifacts:

```sh
npm run bench:browser-full-engine
```

The timed command path includes the public TypeScript API, Worker crossing when selected, WASM engine command execution, packet construction and encryption, browser WebSocket transport, native Prns ingress, delivery receipt generation, return transport, engine settlement, projection/event work, and the final JavaScript promise settlement. It therefore answers a different question from `browser-worker-wire`: the wire benchmark isolates scheduling and representation costs, while this benchmark measures their effect inside a working Prns system.

Payloads and command arrays are allocated before timed regions. Both executions warm before measurement, measurement order rotates across five repetitions, and summaries report medians. The sequential workload measures latency-sensitive use; the coalesced and bounded workloads exercise same-turn command batching below and at the configured admission limit. A separate rotated projection-demand workload runs the same 1,000-command path with all projections released and with interface, route, link, and diagnostic projections observed.

The 2026-08-29 Linux run on a 12th Gen Intel Core i7-1260P with headless Chrome 151 proved the distinction the isolated wire benchmark could not. At 4,096 commands in 256-command grains, the Worker submitted all commands in 0.3 ms and kept the maximum observed event-loop gap to 4.4 ms; main-thread execution spent 36.8 ms submitting and produced a 30.2 ms gap. Main-thread execution completed the network journey faster overall, 999.0 ms versus 1,428.6 ms, because this loopback workload is engine- and receipt-heavy and the Worker still pays its crossings. At 4,000 commands in 100-command grains, the corresponding totals were 2,133.2 ms main-thread and 2,633.9 ms Worker, while submission and maximum event-loop gap were 37.6/16.1 ms main-thread versus 1.1/4.6 ms Worker. Sequential end-to-end throughput was equal at roughly 40 deliveries per second, showing the network/receipt round trip fully dominates isolated command latency there.
