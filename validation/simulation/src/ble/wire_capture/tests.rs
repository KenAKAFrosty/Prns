use super::*;

#[test]
fn eviction_retains_whole_values_and_snapshots_are_independent() {
    let capture = BleWireCapture::new(
        NonZeroUsize::new(2).unwrap_or_else(|| unreachable!("nonzero capacity")),
    );
    let first = BleAddress::new([1; 6]);
    let second = BleAddress::new([2; 6]);
    let link = capture
        .bind()
        .unwrap_or_else(|error| unreachable!("bind: {error:?}"));
    link.record(first, second, BleWireChannel::Control, &[3, 0]);
    let before = capture.snapshot();
    let shared = capture.clone();
    let replacement = shared
        .bind()
        .unwrap_or_else(|error| unreachable!("bind: {error:?}"));
    replacement.record(second, first, BleWireChannel::Data, &[1, 2, 3]);
    replacement.record(first, second, BleWireChannel::Control, &[]);
    assert_eq!(
        before,
        BleWireSnapshot {
            discarded_values: 0,
            values: vec![BleWireValue {
                connection: BleWireConnectionId(0),
                from: first,
                to: second,
                channel: BleWireChannel::Control,
                bytes: vec![3, 0]
            }],
        }
    );
    assert_eq!(
        capture.snapshot(),
        BleWireSnapshot {
            discarded_values: 1,
            values: vec![
                BleWireValue {
                    connection: BleWireConnectionId(1),
                    from: second,
                    to: first,
                    channel: BleWireChannel::Data,
                    bytes: vec![1, 2, 3]
                },
                BleWireValue {
                    connection: BleWireConnectionId(1),
                    from: first,
                    to: second,
                    channel: BleWireChannel::Control,
                    bytes: vec![]
                },
            ],
        }
    );
}

#[test]
fn exhausted_ids_never_wrap_or_alias_even_after_eviction() {
    let capture = BleWireCapture::new(NonZeroUsize::MIN);
    capture
        .buffer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .next_connection = Some(u64::MAX);
    let last = capture
        .bind()
        .unwrap_or_else(|error| unreachable!("last ID: {error:?}"));
    assert_eq!(last.id, BleWireConnectionId(u64::MAX));
    for _ in 0..2 {
        assert!(matches!(
            capture.clone().bind(),
            Err(ConnectionIdsExhausted)
        ));
    }
    for bytes in [vec![1], vec![2]] {
        last.record(
            BleAddress::new([1; 6]),
            BleAddress::new([2; 6]),
            BleWireChannel::Data,
            &bytes,
        );
    }
    assert_eq!(
        capture.snapshot(),
        BleWireSnapshot {
            discarded_values: 1,
            values: vec![BleWireValue {
                connection: BleWireConnectionId(u64::MAX),
                from: BleAddress::new([1; 6]),
                to: BleAddress::new([2; 6]),
                channel: BleWireChannel::Data,
                bytes: vec![2],
            }],
        }
    );
    assert!(matches!(capture.bind(), Err(ConnectionIdsExhausted)));
}
