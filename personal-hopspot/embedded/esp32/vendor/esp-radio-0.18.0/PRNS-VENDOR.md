# Prns vendor record

- Package: `esp-radio 0.18.0`
- Upstream: `https://github.com/esp-rs/esp-hal`
- Registry checksum: `23fbff98b06a96b6ce3791ecec5c668524052a068e23aacd23afe17ddba844ce`
- Upstream revision: `347003de8a48320bb7724f53045be3afa9204411`
- Radio blobs: `esp-wifi-sys 0.2.0`, revision `fee9770fc96fa3bb753b2ce4bd968daa4f068a04`, generated from ESP-IDF 5.5.3
- License: `MIT OR Apache-2.0`
- Local changes:
  - Match ESP-IDF 5.5.3's task-versus-ISR context reporting instead of reporting every Wi-Fi
    adapter call as interrupt context.
  - Align the ESP32-C3/S3 BLE coexistence adapter with ESP-IDF 5.5.3: leave the private dynamic
    priority callback null and make both low-power wake-request callbacks no-ops while controller
    sleep is disabled.
  - Backport upstream esp-rs/esp-hal#5550's corrected BLE half-microsecond-to-low-power-clock
    conversion used by the controller's radio timing.
  - Pair S3 Wi-Fi driver lifecycle with the ESP-IDF PHY receive-enable contract.
  - Treat `esp_wifi_internal_tx`'s synchronous result as transmit admission authority. Network TX
    and RX-token availability no longer depend on best-effort TX-completion callbacks.
  - Add typed data-path diagnostics, bounded radio event tracing, and a transmit-submission circuit
    breaker.
  - Correct Wi-Fi teardown ordering, unregister receive callbacks, drain admitted receive buffers
    before stopping the driver, and use mode restart rather than reallocating the shared driver.
  - Isolate the package as its own Cargo workspace for repository validation.
