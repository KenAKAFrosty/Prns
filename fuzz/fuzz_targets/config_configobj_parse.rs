#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_config::configobj;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = configobj::parse(text);
    }
});
