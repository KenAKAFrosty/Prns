# personal-rns-capi

C ABI bindings for `personal-rns`.

This crate is the low-level native escape hatch for C, C++, Go, Zig, and
other hosts that can consume a C ABI directly. It intentionally does not expose
Rust layouts or UniFFI-generated symbols. Consumers use opaque handles, status
codes, and out-parameters.

```c
#include "personal_rns.h"

prns_runtime_t *runtime = NULL;
prns_status_t status = prns_runtime_new(&runtime);

uint64_t tick_count = 0;
if (status == PRNS_STATUS_OK) {
    status = prns_runtime_tick_count(runtime, &tick_count);
}

prns_runtime_free(runtime);
```

The committed header lives at
[`include/personal_rns.h`](./include/personal_rns.h). Keep it in sync with the
exported symbols whenever this crate's C surface changes.
