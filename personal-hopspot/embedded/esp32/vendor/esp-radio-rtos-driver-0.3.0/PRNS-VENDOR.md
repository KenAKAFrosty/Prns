# PRNS esp-radio-rtos-driver patch

This is `esp-radio-rtos-driver` 0.3.0 with its shared radio callback-timer task pinned to core 0,
matching ESP-IDF's default `esp_timer` affinity and the core used to initialize the Wi-Fi and BLE
libraries. Leaving that task unpinned allowed callbacks to migrate to the application core during
Wi-Fi/BLE coexistence; on ESP32-S3 this could strand the Wi-Fi block-ack reorder window and retain
the complete dynamic RX-buffer pool indefinitely.
