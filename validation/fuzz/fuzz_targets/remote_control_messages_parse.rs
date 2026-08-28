#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::remote_control::{RemoteControlRequest, RemoteControlResponse};

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = RemoteControlRequest::parse(data) {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let written = request
            .write_into(&mut encoded)
            .expect("a parsed request must fit its maximum wire shape");
        let encoded = encoded
            .get(..written)
            .expect("request writer returned an out-of-bounds length");
        assert_eq!(RemoteControlRequest::parse(encoded), Ok(request));
    }

    if let Ok(response) = RemoteControlResponse::parse(data) {
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let written = response
            .write_into(&mut encoded)
            .expect("a parsed response must fit its maximum wire shape");
        let encoded = encoded
            .get(..written)
            .expect("response writer returned an out-of-bounds length");
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
});
