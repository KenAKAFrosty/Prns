#ifndef PERSONAL_RNS_H
#define PERSONAL_RNS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PRNS_ABI_VERSION ((uint32_t)1u)

typedef uint32_t prns_status_t;

#define PRNS_STATUS_OK ((prns_status_t)0u)
#define PRNS_STATUS_NULL_POINTER ((prns_status_t)1u)
#define PRNS_STATUS_ENTROPY_UNAVAILABLE ((prns_status_t)2u)
#define PRNS_STATUS_RUNTIME_POISONED ((prns_status_t)3u)
#define PRNS_STATUS_PANIC ((prns_status_t)4u)

typedef struct prns_runtime prns_runtime_t;

uint32_t prns_abi_version(void);

/* Returned strings are process-static and must not be freed by callers. */
const char *prns_version(void);
const char *prns_status_message(prns_status_t status);

/*
 * Pointer arguments must be valid for the duration of each call. Output
 * pointers must be writable. `prns_runtime_free` must receive either NULL or a
 * handle returned by `prns_runtime_new` that has not already been freed.
 */
prns_status_t prns_runtime_new(prns_runtime_t **out_runtime);
void prns_runtime_free(prns_runtime_t *runtime);

prns_status_t prns_runtime_tick(
    prns_runtime_t *runtime,
    uint64_t *out_emitted
);

prns_status_t prns_runtime_tick_count(
    prns_runtime_t *runtime,
    uint64_t *out_tick_count
);

#ifdef __cplusplus
}
#endif

#endif
