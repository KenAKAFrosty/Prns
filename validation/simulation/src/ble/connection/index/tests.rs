use std::sync::{Arc, Mutex};

use super::super::ConnectionEndpoint;
use super::{BleAddress, Connection, ConnectionIndex};

const FIRST: BleAddress = BleAddress::new([1; 6]);
const SECOND: BleAddress = BleAddress::new([2; 6]);
const THIRD: BleAddress = BleAddress::new([3; 6]);

fn edges(index: &ConnectionIndex) -> Vec<(BleAddress, Vec<[BleAddress; 2]>)> {
    index
        .by_radio
        .iter()
        .map(|(address, connections)| {
            (
                *address,
                connections
                    .iter()
                    .map(|connection| connection.addresses)
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn parallel_and_opposite_direction_links_are_indexed_at_both_endpoints() {
    let mut index = ConnectionIndex::default();
    for (first, second) in [(FIRST, SECOND), (SECOND, FIRST), (FIRST, THIRD)] {
        index.insert(Arc::new(Connection::new(first, second)));
    }
    assert_eq!(
        edges(&index),
        vec![
            (
                FIRST,
                vec![[FIRST, SECOND], [SECOND, FIRST], [FIRST, THIRD]]
            ),
            (SECOND, vec![[FIRST, SECOND], [SECOND, FIRST]]),
            (THIRD, vec![[FIRST, THIRD]]),
        ]
    );
    assert_eq!(index.active_count(), 3);
    assert_eq!(index.disconnect_between(SECOND, FIRST), 2);
    assert_eq!(
        edges(&index),
        vec![(FIRST, vec![[FIRST, THIRD]]), (THIRD, vec![[FIRST, THIRD]])]
    );
    assert_eq!(index.active_count(), 1);
    assert_eq!(index.disconnect_between(SECOND, FIRST), 0);
    assert_eq!(index.disconnect_radio(SECOND), 0);
    assert_eq!(index.disconnect_radio(THIRD), 1);
    assert_eq!(index.active_count(), 0);
    assert_eq!(index.count_for(FIRST), 0);
    assert!(index.by_radio.is_empty());
}

#[test]
fn endpoint_loss_is_reclaimed_before_replacement_without_touching_other_radios() {
    let mut index = ConnectionIndex::default();
    let unrelated = Arc::new(Connection::new(SECOND, THIRD));
    index.insert(unrelated.clone());
    let old = Arc::new(Connection::new(FIRST, SECOND));
    index.insert(old.clone());
    let endpoint = ConnectionEndpoint {
        connection: old.clone(),
        side: super::super::ConnectionSide::Dialer,
    };
    drop(endpoint);
    assert_eq!(index.active_count(), 1);
    assert_eq!(index.count_for(FIRST), 0);
    assert_eq!(index.count_for(SECOND), 1);
    let replacement = Arc::new(Connection::new(FIRST, SECOND));
    index.insert(replacement.clone());
    assert_eq!(
        edges(&index),
        vec![
            (FIRST, vec![[FIRST, SECOND]]),
            (SECOND, vec![[SECOND, THIRD], [FIRST, SECOND]]),
            (THIRD, vec![[SECOND, THIRD]]),
        ]
    );
    assert!(!old.close());
    assert!(!replacement.is_closed());
    assert!(!unrelated.is_closed());
    assert_eq!(index.active_count(), 2);
}

#[test]
fn activity_snapshots_retire_closed_connections_and_do_not_alias_replacements() {
    use super::super::{BleConnectionDataSnapshot, BleDataCounters, ConnectionSide, DataEvent};
    let mut index = ConnectionIndex::default();
    let old = Arc::new(Connection::new(FIRST, SECOND));
    index.insert(old.clone());
    let first = ConnectionEndpoint {
        connection: old.clone(),
        side: ConnectionSide::Dialer,
    };
    let second = ConnectionEndpoint {
        connection: old.clone(),
        side: ConnectionSide::Listener,
    };
    first.outgoing(DataEvent::FragmentQueued);
    second.incoming(DataEvent::ValueConsumed);
    second.outgoing(DataEvent::SendStarted);
    let expected = BleConnectionDataSnapshot {
        dialer_last_send: None,
        listener_last_send: None,
        dialer: FIRST,
        listener: SECOND,
        dialer_to_listener: BleDataCounters {
            fragments_queued: 1,
            values_consumed: 1,
            ..BleDataCounters::default()
        },
        listener_to_dialer: BleDataCounters {
            sends_started: 1,
            ..BleDataCounters::default()
        },
    };
    assert_eq!(index.data_snapshots(), [expected]);
    assert!(old.close());
    assert!(index.data_snapshots().is_empty());
    let replacement = Arc::new(Connection::new(FIRST, SECOND));
    index.insert(replacement);
    first.start_send(b"old closed endpoint");
    assert_eq!(
        index.data_snapshots(),
        [BleConnectionDataSnapshot {
            dialer_last_send: None,
            listener_last_send: None,
            dialer: FIRST,
            listener: SECOND,
            dialer_to_listener: BleDataCounters::default(),
            listener_to_dialer: BleDataCounters::default(),
        }]
    );
}

#[test]
fn data_counter_overflow_is_explicit_and_does_not_wrap() {
    use super::super::{BleDataCounters, DataEvent};
    let mut counts = BleDataCounters {
        fragments_queued: u64::MAX,
        ..BleDataCounters::default()
    };
    counts.record(DataEvent::FragmentQueued);
    counts.record(DataEvent::SendStarted);
    assert_eq!(
        counts,
        BleDataCounters {
            fragments_queued: u64::MAX,
            sends_started: 1,
            saturated: true,
            ..BleDataCounters::default()
        }
    );
}

#[test]
fn dormant_counterpart_entries_do_not_accumulate_across_churn() {
    let mut index = ConnectionIndex::default();
    for _ in 0..1_024 {
        index.insert(Arc::new(Connection::new(FIRST, SECOND)));
        assert_eq!(index.disconnect_radio(FIRST), 1);
        assert_eq!(edges(&index), vec![(SECOND, vec![[FIRST, SECOND]])]);
        assert_eq!(index.active_count(), 0);
    }
    assert_eq!(index.disconnect_radio(SECOND), 0);
    assert!(index.by_radio.is_empty());
}

#[test]
fn endpoint_teardown_during_disconnect_does_not_relock_the_registry() {
    let (done, completion) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let connection = Arc::new(Connection::new(FIRST, SECOND));
        let endpoint = Mutex::new(Some(ConnectionEndpoint {
            connection: connection.clone(),
            side: super::super::ConnectionSide::Dialer,
        }));
        let mut index = ConnectionIndex::default();
        index.insert(connection);
        let registry = Mutex::new(index);
        let mut index = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let closed = index.close_matching(FIRST, |_| {
            // Force teardown while the registry lock and traversal reference are held.
            drop(
                endpoint
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take(),
            );
            false
        });
        let _ = done.send((closed, index.active_count()));
    });
    assert_eq!(
        completion.recv_timeout(std::time::Duration::from_secs(1)),
        Ok((0, 0))
    );
}
