#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::remote_control::{
    RemoteControlRequest, RemoteControlResponse, REMOTE_CONTROL_APP_MESSAGE_CAP,
};

const APPENDED_BYTE: u8 = 0xA5;

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = RemoteControlRequest::parse(data) {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN + 1];
        let written = request
            .write_into(&mut encoded)
            .expect("a parsed request must fit its maximum wire shape");
        let canonical = encoded
            .get(..written)
            .expect("request writer returned an out-of-bounds length");
        assert_eq!(
            RemoteControlRequest::parse(canonical).as_ref(),
            Ok(&request)
        );

        encoded[written] = APPENDED_BYTE;
        let extended = RemoteControlRequest::parse(&encoded[..written + 1]);
        if let RemoteControlRequest::AppMessage(payload) = request {
            match extended {
                Ok(RemoteControlRequest::AppMessage(extended_payload)) => {
                    assert_eq!(
                        extended_payload.as_slice().split_last(),
                        Some((&APPENDED_BYTE, payload.as_slice()))
                    );
                }
                Err(_) => assert_eq!(payload.len(), REMOTE_CONTROL_APP_MESSAGE_CAP),
                Ok(_) => panic!("extending an app payload must preserve its request kind"),
            }
        } else {
            assert!(extended.is_err());
        }
    }

    if let Ok(response) = RemoteControlResponse::parse(data) {
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN + 1];
        let written = response
            .write_into(&mut encoded)
            .expect("a parsed response must fit its maximum wire shape");
        let canonical = encoded
            .get(..written)
            .expect("response writer returned an out-of-bounds length");
        assert_eq!(
            RemoteControlResponse::parse(canonical).as_ref(),
            Ok(&response)
        );

        encoded[written] = APPENDED_BYTE;
        let extended = RemoteControlResponse::parse(&encoded[..written + 1]);
        if let RemoteControlResponse::AppMessage(payload) = response {
            match extended {
                Ok(RemoteControlResponse::AppMessage(extended_payload)) => {
                    assert_eq!(
                        extended_payload.as_slice().split_last(),
                        Some((&APPENDED_BYTE, payload.as_slice()))
                    );
                }
                Err(_) => assert_eq!(payload.len(), REMOTE_CONTROL_APP_MESSAGE_CAP),
                Ok(_) => panic!("extending an app payload must preserve its response kind"),
            }
        } else {
            assert!(extended.is_err());
        }
    }
});
