# Prns vendor record

- Package: `esp-rtos 0.3.0`
- Upstream: `https://github.com/esp-rs/esp-hal`
- Registry checksum: `551f90766e1527edaa0c91e8d559e9e2a60397b545e93357ac61fb31845e5712`
- Fix revisions:
  - `998e4faeaf0afc92b494ece4edc75e80df5624f2` (`esp-rs/esp-hal#6027`)
  - `39abaae0cca19da67ef1ac9f474bc194a69a29ec` (`esp-rs/esp-hal#6032`)
- License: `MIT OR Apache-2.0`
- Local changes:
  - Backport the upstream main-stack sizing, idle-stack overflow-check, and Xtensa interrupt-entry
    fixes.
  - Backport the upstream task-deletion stack-ownership guard. A task that deletes itself must be
    switched away before its stack is returned to the internal allocator; otherwise a radio task
    can keep executing on memory that another core immediately reuses.
  - Propagate `xHigherPriorityTaskWoken` when an interrupt-side radio queue or semaphore wakes a
    task. ESP-IDF's Wi-Fi, Bluetooth, and coexistence adapters rely on this flag to request the
    scheduler pass that runs the unblocked radio worker.
  - Do not request a cross-core yield until the destination scheduler is initialized, and clear a
    stale software-interrupt request before installing the second core's handler. This prevents an
    early radio task from entering core 1's scheduler before its main task and context exist.
  - Add `Task::new_with_stack` / `spawn_task_with_stack` so long-lived workers (Hopspot `run-core`)
    can use a caller-owned stack (e.g. PSRAM) instead of carving the Wi-Fi/BLE `InternalMemory`
    freelist.
