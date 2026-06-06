# microReticulum — announce-256 driver

Measures [microReticulum](https://github.com/attermann/microReticulum) (C++) on the shared
`announce-256` corpus: `Packet::unpack` + `Identity::validate_announce` (the Ed25519 verify
+ store), best-of-50 min wall time. Conformance is the count that validate
(`validate_announce == true`) — the same `resolved` metric as the RNS reference, not the
library's LRU-capped `known_destinations`.

microReticulum is **🟡 partial / MCU-targeted** on the upstream maturity list, included as a
"portable C++ crypto" data point. Its Ed25519 is rweather's portable implementation, built
for microcontrollers — so on a desktop it's the slowest of the bunch by design.

## Run

```sh
./run.sh
```

Needs CMake (≥ 3.15) + a C++17 compiler. Clones the pinned upstream into `.upstream/`
(gitignored), CMake-builds our harness against it (FetchContent pulls its Crypto/microStore
deps), runs it, and writes `../../results/<host>/announce-256/microreticulum.jsonl`.

- **Upstream:** https://github.com/attermann/microReticulum @ `79b8524`
- **License:** Apache-2.0 — we vendor only `main.cpp` + `CMakeLists.txt` (our code) + the numbers.
- **Crypto backend:** rweather Crypto (portable C++, MCU-oriented).
