#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::remote_control::{RemoteControlRequest, RemoteControlResponse};

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = RemoteControlRequest::parse(data) {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN + 1];
        let written = request
            .write_into(&mut encoded)
            .expect("a parsed request must fit its maximum wire shape");
        let canonical = encoded
            .get(..written)
            .expect("request writer returned an out-of-bounds length");
        assert_eq!(RemoteControlRequest::parse(canonical), Ok(request));

        encoded[written] = 0xA5;
        assert!(RemoteControlRequest::parse(&encoded[..written + 1]).is_err());
    }

    if let Ok(response) = RemoteControlResponse::parse(data) {
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN + 1];
        let written = response
            .write_into(&mut encoded)
            .expect("a parsed response must fit its maximum wire shape");
        let canonical = encoded
            .get(..written)
            .expect("response writer returned an out-of-bounds length");
        assert_eq!(RemoteControlResponse::parse(canonical), Ok(response));

        encoded[written] = 0xA5;
        assert!(RemoteControlResponse::parse(&encoded[..written + 1]).is_err());
    }
});
