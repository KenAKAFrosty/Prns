use super::*;

#[test]
fn attachment_order_continues_past_the_old_sixteen_bit_ceiling() {
    let mut ids = RadioIdSequence::new();
    for ordinal in 0..=u64::from(u16::MAX) + 2 {
        assert_eq!(ids.issue().map(BleRadioId::get), Ok(ordinal));
    }
}

#[test]
fn the_last_identifier_is_issued_once_and_exhaustion_never_wraps() {
    let mut ids = RadioIdSequence::Available(BleRadioId(u64::MAX - 1));
    assert_eq!(ids.issue(), Ok(BleRadioId(u64::MAX - 1)));
    assert_eq!(ids.issue(), Ok(BleRadioId(u64::MAX)));
    for _ in 0..3 {
        assert_eq!(ids.issue(), Err(BleSimulationError::RadioIdsExhausted));
    }
}
