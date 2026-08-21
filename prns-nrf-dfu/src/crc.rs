#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareCrc(u16);

impl FirmwareCrc {
    pub const fn get(self) -> u16 {
        self.0
    }
}

pub fn firmware_crc(bytes: &[u8]) -> FirmwareCrc {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc = crc.rotate_right(8);
        crc ^= u16::from(*byte);
        crc ^= (crc & 0x00ff) >> 4;
        crc ^= (crc << 8) << 4;
        crc ^= ((crc & 0x00ff) << 4) << 1;
    }
    FirmwareCrc(crc)
}

#[cfg(test)]
mod tests {
    use super::{firmware_crc, FirmwareCrc};

    #[test]
    fn matches_crc_ccitt_false_reference_value() {
        assert_eq!(firmware_crc(b"123456789"), FirmwareCrc(0x29b1));
    }
}
