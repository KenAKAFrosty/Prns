#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::remote_control::{RemoteControlPairingRequest, RemoteControlPairingResponse};

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = RemoteControlPairingRequest::parse(data) {
        let mut encoded = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
        let written = request
            .write_into(&mut encoded)
            .expect("a parsed pairing request must fit its maximum wire shape");
        let encoded = encoded
            .get(..written)
            .expect("pairing request writer returned an out-of-bounds length");
        assert_eq!(RemoteControlPairingRequest::parse(encoded), Ok(request));
    }

    if let Ok(response) = RemoteControlPairingResponse::parse(data) {
        let mut encoded = [0u8; RemoteControlPairingResponse::MAX_ENCODED_LEN];
        let written = response
            .write_into(&mut encoded)
            .expect("a parsed pairing response must fit its maximum wire shape");
        let encoded = encoded
            .get(..written)
            .expect("pairing response writer returned an out-of-bounds length");
        assert_eq!(RemoteControlPairingResponse::parse(encoded), Ok(response));
    }
});
