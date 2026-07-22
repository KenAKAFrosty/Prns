use super::{Fleet, FleetWire, InboundDeliveryError};
use crate::engine::FanTarget;
use crate::interfaces::InterfaceId;
use crate::reactor::driver::{leaked_grant_lane, InterfaceLifecycle};
use crate::reactor::grant::{FrameTarget, ReactorLaneWriter};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
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

    assert!(outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"one"));
    let frame = block_on(fleet.next_outbound());
    assert_eq!(frame.target(), FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.bytes(), b"one");

    assert!(
        outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"two"),
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

    assert!(outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"hi"));
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

    assert!(!outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"too large"));
    assert!(fleet.try_next_outbound().is_none());
    assert!(outbound_tx.try_write(FrameTarget::Fan(FanTarget::All), b"fits"));
    assert_eq!(fleet.try_next_outbound().unwrap().bytes(), b"fits");
}
