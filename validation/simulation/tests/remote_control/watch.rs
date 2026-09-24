use super::fixture::*;
use personal_rns::remote_control::*;
use personal_rns::runtime::StreamId;
use prns_simulation::{FaultPlan, ManualTaskScheduling};

#[test]
fn interface_watch_replays_initial_resync_and_heartbeat_under_manual_time() {
    let run = || {
        with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
            let link = lab.link(CONTROLLER);
            let watch = lab.watch(link, StreamId::new(3).expect("watch ID"));
            let task = lab.read_watch(watch);
            let completed = lab.settle();
            let (watch, first) = watch_result(task, completed);
            assert_eq!(
                first.expect("initial frame"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
            let task = lab.read_watch(watch);
            assert!(lab.settle().is_empty());
            assert!(lab.advance(4_999).is_empty(), "no premature heartbeat");
            let completed = lab.advance(1);
            let (_, heartbeat) = watch_result(task, completed);
            assert_eq!(
                heartbeat.expect("heartbeat frame"),
                RemoteControlStreamEvent::Heartbeat { sequence: 2 }
            );
        })
    };
    let first = run();
    assert_eq!(first, run());
    assert_eq!(first, run());
}

#[test]
fn stable_interface_changes_invalidate_snapshots_and_link_close_ends_the_reader() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let watch = lab.watch(link, StreamId::new(3).expect("watch ID"));
        let task = lab.read_watch(watch);
        let completed = lab.settle();
        let (watch, initial) = watch_result(task, completed);
        assert_eq!(
            initial.expect("initial"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
        let attached = lab.nodes[TARGET]
            .handle
            .add_interface(inventory::IdleInterface {
                tag: b"watch-change".to_vec(),
            });
        let task = lab.read_watch(watch);
        assert!(lab.settle().is_empty());
        let completed = lab.advance(500);
        let (watch, changed) = watch_result(task, completed);
        assert_eq!(
            changed.expect("invalidation"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 2 }
        );
        attached.teardown();
        let task = lab.read_watch(watch);
        assert!(lab.settle().is_empty());
        let completed = lab.advance(500);
        let (watch, removed) = watch_result(task, completed);
        assert_eq!(
            removed.expect("removal"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 3 }
        );
        let task = lab.read_watch(watch);
        assert!(lab.nodes[CONTROLLER].handle.close_link(link));
        let completed = lab.settle();
        let (_, closed) = watch_result(task, completed);
        assert!(closed.is_err(), "link closure must end the reader");
    });
}

#[test]
fn watch_capacity_is_bounded_and_closing_the_link_releases_all_slots() {
    with_lab(ManualTaskScheduling::Cyclic, FaultPlan::none(), |lab| {
        let link = lab.link(CONTROLLER);
        let mut watches = Vec::new();
        for index in 0..8 {
            watches.push(lab.watch(link, StreamId::new(index).expect("watch ID")));
        }
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::WatchInterfaces {
                    stream_id: StreamId::new(8).expect("watch ID")
                }
            ),
            RemoteControlResponse::ProtocolError(RemoteControlProtocolError::Busy {
                request: RemoteControlRequestKind::WatchInterfaces
            })
        );
        assert_eq!(
            lab.exchange(
                CONTROLLER,
                link,
                RemoteControlRequest::WatchInterfaces {
                    stream_id: StreamId::new(0).expect("watch ID")
                }
            ),
            RemoteControlResponse::ProtocolError(RemoteControlProtocolError::Busy {
                request: RemoteControlRequestKind::WatchInterfaces
            })
        );
        assert!(lab.nodes[CONTROLLER].handle.close_link(link));
        assert!(lab.settle().is_empty());
        drop(watches);
        let fresh = lab.link(CONTROLLER);
        assert_ne!(fresh, link);
        let watch = lab.watch(fresh, StreamId::new(0).expect("watch ID"));
        let task = lab.read_watch(watch);
        let completed = lab.settle();
        let (_, initial) = watch_result(task, completed);
        assert_eq!(
            initial.expect("fresh initial"),
            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
        );
    });
}
