#include "../include/prns_host.h"

_Static_assert(PRNS_HOST_CONTRACT_ABI == 1, "unexpected ABI");
_Static_assert(PRNS_DESTINATION_HASH_LENGTH == 16, "unexpected destination hash");
_Static_assert(PRNS_APPLICATION_EVENT_KIND_SINGLE_DELIVERY == 100, "unexpected event kind");

int main(void) {
    PrnsContractInfo contract = {0};
    PrnsHostOptions options = {0};
    PrnsLifecycle lifecycle = {0};
    contract.struct_size = sizeof(contract);
    options.struct_size = sizeof(options);
    options.limits.struct_size = sizeof(options.limits);
    lifecycle.struct_size = sizeof(lifecycle);
    return contract.abi + options.required_abi + lifecycle.phase;
}
