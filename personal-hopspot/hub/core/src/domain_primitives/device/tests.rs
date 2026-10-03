#![allow(clippy::unwrap_used)]
extern crate alloc;
use super::*;

#[test]
fn labels_preserve_text_and_bound_utf8_bytes() {
    for blank in ["", " \t\n", "\u{2003}"] {
        assert_eq!(DeviceLabel::new(blank), Err(DeviceLabelError::Blank));
    }
    assert_eq!(
        DeviceLabel::new("  Roof 🛰  ").unwrap().as_str(),
        "  Roof 🛰  "
    );
    assert!(DeviceLabel::new(&"é".repeat(64)).is_ok());
    assert_eq!(
        DeviceLabel::new(&"é".repeat(65)),
        Err(DeviceLabelError::TooLong {
            bytes: 130,
            maximum: MAX_DEVICE_LABEL_BYTES,
        })
    );
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_unicode_labels_match_the_validation_contract(text in any::<alloc::string::String>()) {
        let result = DeviceLabel::new(&text);
        if text.chars().all(char::is_whitespace) {
            prop_assert_eq!(result, Err(DeviceLabelError::Blank));
        } else if text.len() > MAX_DEVICE_LABEL_BYTES {
            prop_assert_eq!(result, Err(DeviceLabelError::TooLong { bytes: text.len(), maximum: MAX_DEVICE_LABEL_BYTES }));
        } else {
            let label = result.unwrap();
            prop_assert_eq!(label.as_str(), &text);
        }
    }
}
