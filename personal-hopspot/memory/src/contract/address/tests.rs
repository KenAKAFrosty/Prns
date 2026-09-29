use super::*;

#[test]
fn range_size_rejects_empty_and_overflowing_ranges() {
    assert_eq!(
        AddressRange::from_start_and_size(7, 0),
        Err(AddressRangeError::Empty)
    );
    assert_eq!(
        AddressRange::from_start_and_size(u64::MAX - 3, 4),
        Err(AddressRangeError::Overflow)
    );
    assert_eq!(
        AddressRange::from_start_and_size(0x1000, 0x20),
        Ok(AddressRange::new(0x1000, 0x1020))
    );
}

#[test]
fn alignment_rejects_zero_and_non_power_of_two_values() {
    assert_eq!(Alignment::try_new(0), Err(AlignmentError::Zero));
    assert_eq!(
        Alignment::try_new(24),
        Err(AlignmentError::NotPowerOfTwo { bytes: 24 })
    );
    assert_eq!(Alignment::try_new(4096).map(Alignment::bytes), Ok(4096));
}
