pub(crate) fn parse_u8(value: &str) -> Option<u8> {
    digits(value, 2).and_then(|digits| u8::from_str_radix(digits, 16).ok())
}

pub(crate) fn parse_u16(value: &str) -> Option<u16> {
    digits(value, 4).and_then(|digits| u16::from_str_radix(digits, 16).ok())
}

pub(crate) fn parse_u32(value: &str) -> Option<u32> {
    digits(value, 8).and_then(|digits| u32::from_str_radix(digits, 16).ok())
}

fn digits(value: &str, width: usize) -> Option<&str> {
    value.strip_prefix("0x").filter(|digits| {
        digits.len() == width
            && digits
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hex_requires_prefix_width_and_lowercase() {
        assert_eq!(parse_u8("0x01"), Some(1));
        assert_eq!(parse_u16("0x00ff"), Some(255));
        assert_eq!(parse_u32("0xada52840"), Some(0xada5_2840));
        assert_eq!(parse_u32("ada52840"), None);
        assert_eq!(parse_u32("0xAda52840"), None);
        assert_eq!(parse_u32("0xada5284"), None);
    }
}
