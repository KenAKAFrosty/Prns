use super::*;

#[kani::proof]
#[kani::unwind(8)]
fn pairing_request_parse_terminates_for_every_bounded_wire_shape() {
    let bytes: [u8; RemoteControlPairingRequest::MAX_ENCODED_LEN + 1] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let _result = RemoteControlPairingRequest::parse(&bytes[..len]);
}

#[kani::proof]
#[kani::unwind(8)]
fn pairing_response_parse_terminates_for_every_bounded_wire_shape() {
    let bytes: [u8; RemoteControlPairingResponse::MAX_ENCODED_LEN + 1] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let _result = RemoteControlPairingResponse::parse(&bytes[..len]);
}
