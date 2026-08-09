# Prns vendor record

- Package: `esp-radio 0.18.0`
- Upstream: `https://github.com/esp-rs/esp-hal`
- Registry checksum: `23fbff98b06a96b6ce3791ecec5c668524052a068e23aacd23afe17ddba844ce`
- Upstream revision: `347003de8a48320bb7724f53045be3afa9204411`
- Radio blobs: `esp-wifi-sys 0.2.0`, revision `fee9770fc96fa3bb753b2ce4bd968daa4f068a04`, generated from ESP-IDF 5.5.3
- License: `MIT OR Apache-2.0`
- Local changes:
  - Return Wi-Fi transmit credit when an interface state change rejects a send.
  - Prefer external memory for ESP-IDF Wi-Fi allocations when static TX buffers are selected, while retaining internal-only allocator paths.
  - Align the ESP32-C3/S3 BLE controller OS adapter ABI with ESP-IDF 5.5.3.
  - Pair S3 Wi-Fi driver lifecycle with the ESP-IDF PHY receive-enable contract.
  - Add typed data-path diagnostics, bounded radio event tracing, stalled-credit recovery, and a transmit-submission circuit breaker.
  - Correct full Wi-Fi reinitialization teardown ordering, unregister receive callbacks, and drain admitted receive buffers before driver deinitialization.
  - Isolate the package as its own Cargo workspace for repository validation.
