# PRNS esp-alloc patch

This is `esp-alloc` 0.10.0 with its fixed heap-region table enlarged from three entries to four.
ESP32-S3 boards register PSRAM plus three disjoint internal-memory windows: reclaimed boot RAM, a
small static radio reserve, and the unused D-cache address window. Allocation behavior and region
ordering are otherwise unchanged.
