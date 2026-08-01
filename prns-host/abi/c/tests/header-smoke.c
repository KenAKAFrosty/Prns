#include "../include/prns_host.h"

#ifdef __cplusplus
#define PRNS_STATIC_ASSERT static_assert
#define PRNS_ZERO {}
#else
#define PRNS_STATIC_ASSERT _Static_assert
#define PRNS_ZERO {0}
#endif

PRNS_STATIC_ASSERT(PRNS_HOST_CONTRACT_ABI == 1, "unexpected ABI");
PRNS_STATIC_ASSERT(PRNS_DESTINATION_HASH_LENGTH == 16, "unexpected destination hash");
PRNS_STATIC_ASSERT(PRNS_APPLICATION_EVENT_KIND_SINGLE_DELIVERY == 100, "unexpected event kind");

int main(void) {
    PrnsContractInfo contract = PRNS_ZERO;
    PrnsHostOptions options = PRNS_ZERO;
    PrnsInterfaceConfig interface_config = PRNS_ZERO;
    PrnsLifecycle lifecycle = PRNS_ZERO;
    contract.struct_size = sizeof(contract);
    options.struct_size = sizeof(options);
    options.limits.struct_size = sizeof(options.limits);
    interface_config.struct_size = sizeof(interface_config);
    lifecycle.struct_size = sizeof(lifecycle);
    return contract.abi + options.required_abi + interface_config.kind + lifecycle.phase;
}
