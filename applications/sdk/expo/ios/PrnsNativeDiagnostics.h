#ifndef PRNS_NATIVE_DIAGNOSTICS_H
#define PRNS_NATIVE_DIAGNOSTICS_H
#include <stdint.h>
#include <stddef.h>
// Debug-only process-owned telemetry callback; no application domain ABI.
void prns_app_ios_install_restoration_probe(void (*emit)(uint64_t, const uint8_t *, size_t));
#endif
