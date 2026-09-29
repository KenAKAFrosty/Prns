#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::interfaces::bluetooth_auto::{Control, CONTROL_MAX_LEN};

fuzz_target!(|data: &[u8]| {
    if let Ok(control) = Control::try_decode(data) {
        let mut encoded = [0u8; CONTROL_MAX_LEN + 1];
        let written = control
            .encode(&mut encoded)
            .expect("a parsed control value must fit the maximum control length");
        let canonical = &encoded[..written];
        assert_eq!(Control::try_decode(canonical), Ok(control));

        encoded[written] = 0xA5;
        assert!(Control::try_decode(&encoded[..written + 1]).is_err());
    }
});
