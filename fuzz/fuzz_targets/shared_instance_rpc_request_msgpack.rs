#![no_main]

#[allow(dead_code)]
#[path = "../../prns-interfaces/impls/tokio/src/shared_instance/rpc_compat/request.rs"]
mod request;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = request::decode(data);
});
