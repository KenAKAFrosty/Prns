#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::interfaces::shared_instance::rns_rpc::RpcRequest;

fuzz_target!(|data: &[u8]| {
    let _ = RpcRequest::decode(data);
});
