use super::*;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

fn exercise(mut queue: impl ScheduledAnnounceQueue) {
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"controller");
    let absent_child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"absent");
    let keep = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"keep");
    let dest = |byte| DestinationHash::new([byte; 16]);
    // Both a parked flood and a directed answer can refer to an absent child.
    let _ = queue.schedule(dest(1), InstantMillis(100), absent_child, 2);
    let _ = queue.schedule_directed(dest(1), InstantMillis(50), keep, 2);
    let _ = queue.schedule(dest(2), InstantMillis(200), keep, 2);
    let _ = queue.schedule_directed(dest(2), InstantMillis(60), absent_child, 2);
    let _ = queue.schedule(dest(3), InstantMillis(300), supervisor, 2);
    assert_eq!(queue.cancel_interface(supervisor), 3);
    assert!(queue.iter().all(|entry| entry.source_interface == keep));
    assert_eq!(queue.earliest_due_at(), Some(InstantMillis(50)));
    // Removing the child's directed answer lets the unrelated parked flood return.
    // The retired parked flood must never return when its surviving answer completes.
    queue.advance_due_retransmits(InstantMillis(500), 50, 2);
    assert!(queue.iter().all(|entry| entry.source_interface == keep));
    assert!(queue
        .iter()
        .any(|entry| entry.destination == dest(2) && entry.directed_to.is_none()));
    queue.advance_due_retransmits(InstantMillis(600), 50, 2);
    assert!(!queue
        .iter()
        .any(|entry| entry.destination == dest(1) || entry.destination == dest(3)));
    assert_eq!(queue.cancel_interface(supervisor), 0);
    assert_eq!(queue.scheduled_count(), 0);
    // A sole surviving parked row must be restored and armed immediately; no other timer
    // will call advance_due_retransmits when the active queue was emptied by retirement.
    let _ = queue.schedule(dest(4), InstantMillis(900), keep, 2);
    let _ = queue.schedule_directed(dest(4), InstantMillis(800), absent_child, 2);
    assert_eq!(queue.cancel_interface(supervisor), 1);
    assert_eq!(queue.scheduled_count(), 1);
    assert_eq!(queue.earliest_due_at(), Some(InstantMillis(900)));
    let surviving = queue.iter().next().unwrap();
    assert_eq!(surviving.source_interface, keep);
    assert_eq!(surviving.directed_to, None);
    assert_eq!(queue.cancel_interface(keep), 1);
    assert_eq!(queue.scheduled_count(), 0);
    assert_eq!(queue.earliest_due_at(), None);
}

#[test]
fn administrative_retirement_fixed_active_and_parked() {
    exercise(FixedScheduledAnnounceQueue::<8>::new());
}

#[cfg(feature = "alloc")]
#[test]
fn administrative_retirement_heap_active_and_parked() {
    exercise(HeapScheduledAnnounceQueue::default());
}

#[cfg(feature = "external-alloc")]
#[test]
fn administrative_retirement_fixed_heap_active_and_parked() {
    exercise(FixedHeapScheduledAnnounceQueue::<8>::new());
}
