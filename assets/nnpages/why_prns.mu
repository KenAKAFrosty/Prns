>>`!Why Prns?`!

`F6eb•`f `F6eb`!Efficiency`!`f
>>>
Benchmarks you can rerun yourself compare Prns with stock interpreted RNS 1.5.4. Single-packet throughput on an Apple M4 reaches 110.22×. Across 30 published host/scenario comparisons, the middle half is roughly 5×–25× (median 9.62×), with a full range of 1.02×–110.22×.

In separate Linux workloads, Prns uses about 97% less peak memory than stock RNS for the sender of a matched-policy 64-segment stream, and about 98% less processor energy per request for request/response. These are loopback results, not guaranteed radio speedups or battery-life gains. See benchmarks/README.md and benchmarks/RESULTS.md in the source for the measurement scope and full tables.

>>
`F6eb•`f `F6eb`!Drop-in`!`f
>>>
Reads your existing ~/.reticulum config, and stock apps attach to the Prns daemon as their shared instance, unchanged.

>>
`F6eb•`f `F6eb`!Goes where Python can't`!`f
>>>
The same engine runs on bare-metal microcontrollers, directly inside Android and iOS apps, and in the browser. SDKs and bindings cover Rust, TypeScript and JavaScript, Python, .NET and C#, Go, Swift, Kotlin and Java, Julia, and C and C++.

>>
`F6eb•`f `F6eb`!Ready-to-flash embedded nodes`!`f
>>>
The Personal Hopspot firmware ships for a growing catalog of affordable boards: a complete node with on-device controls and a status screen when the hardware supports it.

>>
`F6eb•`f `F6eb`!New interfaces for the same network`!`f
>>>
Bluetooth LE Auto-interface, Auto USB, improved Wi-Fi Auto-interface, ESP-NOW, Wi-Fi Aware/NAN, and WebSocket server/client, alongside the interfaces you already run.

>>
`F6eb•`f `F6eb`!Built for operators`!`f
>>>
The Prns daemon, prnsd, offers a managed lifecycle, an interface editor CLI, live interface changes without a restart, and built-in metrics.

>>
`F6eb•`f `F6eb`!Built for builders`!`f
>>>
Library-first, with one deliberate API for building Reticulum into apps, tools, services, and games.

>>
`F6eb•`f `F6eb`!Thoroughly tested`!`f
>>>
Byte-for-byte wire parity with the reference implementation, a live interop suite against stock RNS, formal proofs, fuzzing, mutation testing, and sanitizers.

>>
