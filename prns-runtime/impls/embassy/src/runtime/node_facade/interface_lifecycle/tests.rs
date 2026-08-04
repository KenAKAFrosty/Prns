use super::{Fleet, FleetWire, InboundDeliveryError};
use crate::engine::FanTarget;
use crate::interfaces::InterfaceId;
use crate::manifold::driver::{leaked_grant_lane, InterfaceLifecycle};
use crate::manifold::grant::{
    FrameTarget, LaneWriteOutcome, ManifoldLaneReader, ManifoldLaneWriter,
};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration};

type Mtx = CriticalSectionRawMutex;
const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;

fn leak<T>(value: T) -> &'static T {
    std::boxed::Box::leak(std::boxed::Box::new(value))
}

#[test]
fn next_outbound_releases_the_copied_grant_so_the_depth_one_lane_refills() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<FRAME>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<FRAME>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, FRAME, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );

    assert_eq!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"one"),
        LaneWriteOutcome::Written
    );
    let frame = block_on(fleet.next_outbound());
    assert_eq!(frame.target(), FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.bytes(), b"one");

    assert_eq!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"two"),
        LaneWriteOutcome::Written,
        "the depth-1 lane must accept the next frame the instant next_outbound copied the last"
    );
    let frame = block_on(fleet.next_outbound());
    assert_eq!(frame.target(), FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.bytes(), b"two");
}

#[test]
fn an_outbound_commit_wakes_the_supervisor_and_try_next_outbound_drains() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<FRAME>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<FRAME>(1);
    let wake: &'static Signal<Mtx, ()> = leak(Signal::new());
    outbound_tx.set_outbound_wake(wake);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, FRAME, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: wake,
        },
        lifecycle.sender(),
    );

    assert!(
        fleet.try_next_outbound().is_none(),
        "an empty lane drains to nothing"
    );

    assert_eq!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"hi"),
        LaneWriteOutcome::Written
    );
    block_on(with_timeout(
        Duration::from_millis(50),
        fleet.outbound_ready(),
    ))
    .expect("the commit must signal the outbound wake");

    let frame = fleet
        .try_next_outbound()
        .expect("the committed frame drains after the wake");
    assert_eq!(frame.target(), FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.bytes(), b"hi");
    assert!(
        fleet.try_next_outbound().is_none(),
        "the depth-1 lane is empty once drained"
    );
}

#[test]
fn one_coalesced_wake_covers_every_committed_outbound_frame() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<8>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<8>(4);
    let wake: &'static Signal<Mtx, ()> = leak(Signal::new());
    outbound_tx.set_outbound_wake(wake);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, 8, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: wake,
        },
        lifecycle.sender(),
    );

    for frame in [b"one".as_slice(), b"two", b"three", b"four"] {
        assert_eq!(
            outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), frame),
            LaneWriteOutcome::Written
        );
    }

    block_on(with_timeout(
        Duration::from_millis(50),
        fleet.outbound_ready(),
    ))
    .expect("the burst must publish a wake");

    let mut drained = std::vec::Vec::new();
    while let Some(frame) = fleet.try_next_outbound() {
        drained.push(frame.bytes().to_vec());
    }
    assert_eq!(
        drained,
        [b"one".as_slice(), b"two", b"three", b"four"]
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect::<std::vec::Vec<_>>()
    );
    assert!(
        block_on(with_timeout(
            Duration::from_millis(10),
            fleet.outbound_ready(),
        ))
        .is_err(),
        "the four commits intentionally coalesce to one wake"
    );
}

#[test]
fn deregistration_waits_for_lifecycle_lane_capacity() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<FRAME>(1);
    let (_outbound_tx, outbound) = leaked_grant_lane::<FRAME>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let fleet: Fleet<Mtx, FRAME, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );
    let first = InterfaceId::new([1; 8]);
    let second = InterfaceId::new([2; 8]);
    assert!(lifecycle
        .sender()
        .try_send(InterfaceLifecycle::Remove { id: first })
        .is_ok());

    block_on(join(fleet.deregister_member(second), async {
        assert!(matches!(
            lifecycle.receiver().receive().await,
            InterfaceLifecycle::Remove { id } if id == first
        ));
        assert!(matches!(
            lifecycle.receiver().receive().await,
            InterfaceLifecycle::Remove { id } if id == second
        ));
    }));
}

#[test]
fn inbound_delivery_distinguishes_oversized_frames_from_a_full_lane() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<8>(1);
    let (_outbound_tx, outbound) = leaked_grant_lane::<8>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, 8, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );
    let member = InterfaceId::new([3; 8]);

    assert_eq!(
        fleet.try_deliver_inbound(member, &[0; 9]),
        Err(InboundDeliveryError::FrameTooLarge {
            len: 9,
            capacity: 8,
        })
    );
    assert_eq!(fleet.try_deliver_inbound(member, b"fits"), Ok(()));
    assert_eq!(
        fleet.try_deliver_inbound(member, b"blocked"),
        Err(InboundDeliveryError::LaneFull)
    );
}

#[test]
fn reliable_inbound_delivery_waits_for_depth_one_lane_capacity() {
    let (inbound, mut inbound_rx) = leaked_grant_lane::<8>(1);
    let (_outbound_tx, outbound) = leaked_grant_lane::<8>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, 8, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );
    let member = InterfaceId::new([4; 8]);

    block_on(fleet.deliver_inbound(member, b"first")).unwrap();
    block_on(join(fleet.deliver_inbound(member, b"second"), async {
        let (target, _, frame) = inbound_rx.try_read().expect("first frame is retained");
        assert_eq!(target, FrameTarget::Direct(member));
        assert_eq!(frame, b"first");
        inbound_rx.release();
        assert_eq!(notify.receiver().receive().await, member);

        assert_eq!(notify.receiver().receive().await, member);
        let (target, _, frame) = inbound_rx
            .try_read()
            .expect("second frame follows capacity");
        assert_eq!(target, FrameTarget::Direct(member));
        assert_eq!(frame, b"second");
        inbound_rx.release();
    }))
    .0
    .unwrap();
}

#[test]
fn outbound_capacity_is_enforced_before_a_frame_reaches_the_fleet() {
    let (inbound, _inbound_rx) = leaked_grant_lane::<8>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<8>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, 8, 1, 1> = Fleet::new(
        FleetWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );

    assert_eq!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"too large"),
        LaneWriteOutcome::FrameTooLarge {
            frame_len: 9,
            capacity: 8,
        }
    );
    assert!(fleet.try_next_outbound().is_none());
    assert_eq!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"fits"),
        LaneWriteOutcome::Written
    );
    assert_eq!(fleet.try_next_outbound().unwrap().bytes(), b"fits");
}
