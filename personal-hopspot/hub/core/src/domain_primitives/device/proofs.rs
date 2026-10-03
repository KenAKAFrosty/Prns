use super::{DeviceLabel, DeviceLabelError};

#[kani::proof]
#[kani::unwind(8)]
fn short_ascii_labels_preserve_bytes_or_reject_only_whitespace() {
    let bytes: [u8; 2] = kani::any();
    kani::assume(bytes.iter().all(u8::is_ascii));
    let Ok(text) = core::str::from_utf8(&bytes) else {
        assert!(false);
        return;
    };
    let blank = bytes
        .iter()
        .all(|byte| matches!(*byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c));
    match DeviceLabel::new(text) {
        Ok(label) => {
            assert!(!blank);
            assert_eq!(label.as_str(), text);
        }
        Err(DeviceLabelError::Blank) => assert!(blank),
        Err(DeviceLabelError::TooLong { .. }) => assert!(false),
    }
}
