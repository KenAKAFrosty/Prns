use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use crate::interfaces::{InterfaceId, PacketPhyStats};
use crate::manifold::grant::{GrantConsumer, GrantProducer};
use crate::manifold::interface_seam::{InterfaceSeam, OutboundDisposition};

use super::super::leaked_grant_lane;
use super::EmbassyInterfaceSeam;

#[test]
fn packet_phy_crosses_the_embassy_ingress_seam_with_its_frame() {
    const FRAME: usize = 64;

    let interface = InterfaceId::new([0xA1; 8]);
    let (inbound, mut manifold_inbound) = leaked_grant_lane::<FRAME>(1);
    let (_manifold_outbound, outbound) = leaked_grant_lane::<FRAME>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let packet_phy = PacketPhyStats {
        rssi: Some(crate::interfaces::RssiDbm::new(-87)),
        snr: Some(crate::interfaces::SnrQuarterDb::new(-9)),
        quality: crate::interfaces::SignalQualityTenthsPercent::new(875),
    };
    let mut seam = EmbassyInterfaceSeam::new(
        interface,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );

    block_on(seam.next_inbound_with_phy(b"observed", packet_phy));

    let retained = manifold_inbound
        .try_peek()
        .expect("the committed frame reaches the manifold lane");
    assert_eq!(
        (retained.frame(), retained.packet_phy),
        (b"observed".as_slice(), packet_phy)
    );
    assert_eq!(notify.receiver().try_receive(), Ok(interface));

    manifold_inbound.release();
    block_on(seam.next_inbound(b"plain"));

    let retained = manifold_inbound
        .try_peek()
        .expect("the next committed frame reaches the manifold lane");
    assert_eq!(
        (retained.frame(), retained.packet_phy),
        (b"plain".as_slice(), PacketPhyStats::default())
    );
}

#[test]
fn accepted_outbound_custody_releases_lane_storage_before_completion() {
    const FRAME: usize = 64;

    let interface = InterfaceId::new([0xA2; 8]);
    let (inbound, _manifold_inbound) = leaked_grant_lane::<FRAME>(1);
    let (mut manifold_outbound, outbound) = leaked_grant_lane::<FRAME>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let mut seam = EmbassyInterfaceSeam::new(
        interface,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );

    manifold_outbound
        .try_grant()
        .expect("the first outbound slot is free")
        .fill_for(interface, b"first");
    manifold_outbound.commit();
    assert_eq!(block_on(seam.next_outbound()), b"first");

    seam.accept_outbound_custody();
    manifold_outbound
        .try_grant()
        .expect("accepted custody frees the slot before radio completion")
        .fill_for(interface, b"second");
    manifold_outbound.commit();
    assert_eq!(block_on(seam.next_outbound()), b"second");

    seam.complete_outbound(OutboundDisposition::Sent);
    assert!(
        manifold_outbound.try_grant().is_some(),
        "final completion releases whichever frame is currently borrowed"
    );
}

#[test]
fn published_channel_rejects_old_lane_frames_and_stamps_new_ingress() {
    use core::{
        future::Future,
        task::{Context, Poll},
    };
    let old = InterfaceId::new(*b"oldlora0");
    let new = InterfaceId::new(*b"newlora0");
    let (inbound, mut received) = leaked_grant_lane::<64>(1);
    let (mut sender, outbound) = leaked_grant_lane::<64>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let mut seam = EmbassyInterfaceSeam::new(
        old,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );
    sender.try_grant().unwrap().fill_for(old, b"old channel");
    sender.commit();
    seam.set_channel_id(new);
    let mut pending = std::boxed::Box::pin(seam.next_outbound());
    let mut cx = Context::from_waker(std::task::Waker::noop());
    assert_eq!(pending.as_mut().poll(&mut cx), Poll::Pending);
    sender.try_grant().unwrap().fill_for(new, b"new channel");
    sender.commit();
    assert_eq!(
        pending.as_mut().poll(&mut cx),
        Poll::Ready(b"new channel".as_slice())
    );
    drop(pending);
    assert_eq!(seam.take_channel_change_drops(), 1);
    assert_eq!(seam.take_channel_change_drops(), 0);
    seam.complete_outbound(OutboundDisposition::Sent);
    block_on(seam.next_inbound(b"new ingress"));
    assert_eq!(
        received.try_peek().unwrap().target,
        crate::manifold::grant::FrameTarget::Direct(new)
    );
    assert_eq!(notify.receiver().try_receive(), Ok(new));
}

#[test]
fn publication_drains_stale_prefix_immediately_and_preserves_new_channel_work() {
    let old = InterfaceId::new(*b"oldlora0");
    let new = InterfaceId::new(*b"newlora0");
    let (inbound, _received) = leaked_grant_lane::<64>(1);
    let (mut sender, outbound) = leaked_grant_lane::<64>(3);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let mut seam = EmbassyInterfaceSeam::new(
        old,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );
    for target in [old, old, new] {
        sender
            .try_grant()
            .unwrap()
            .fill_for(target, target.as_bytes());
        sender.commit();
    }
    seam.set_channel_id(new);
    assert_eq!(seam.take_channel_change_drops(), 2);
    seam.set_channel_id(new);
    assert_eq!(block_on(seam.next_outbound()), new.as_bytes());
    seam.accept_outbound_custody();
    sender.try_grant().unwrap().fill_for(new, b"same channel");
    sender.commit();
    seam.set_channel_id(new);
    assert_eq!(block_on(seam.next_outbound()), b"same channel");
    seam.complete_outbound(OutboundDisposition::Sent);
    sender
        .try_grant()
        .unwrap()
        .fill_for(new, b"must not survive B-A-B");
    sender.commit();
    seam.set_channel_id(old);
    seam.set_channel_id(new);
    assert_eq!(seam.take_channel_change_drops(), 1);
    assert!(sender.try_grant().is_some());
}

#[test]
fn phy_ingress_stamps_current_identity_and_rejects_empty_or_oversized_frames() {
    use crate::manifold::grant::FrameTarget;
    let old = InterfaceId::new(*b"oldlora0");
    let new = InterfaceId::new(*b"newlora0");
    let (inbound, mut received) = leaked_grant_lane::<8>(1);
    let (_, outbound) = leaked_grant_lane::<8>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let mut seam = EmbassyInterfaceSeam::new(
        old,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );
    block_on(seam.next_inbound_with_phy(b"first", PacketPhyStats::default()));
    assert_eq!(
        received.try_peek().unwrap().target,
        FrameTarget::Direct(old)
    );
    received.release();
    assert_eq!(notify.receiver().try_receive(), Ok(old));
    seam.set_channel_id(new);
    for bytes in [b"".as_slice(), b"too large".as_slice()] {
        block_on(seam.next_inbound_with_phy(bytes, PacketPhyStats::default()));
        assert!(received.try_peek().is_none());
        assert!(notify.receiver().try_receive().is_err());
    }
    block_on(seam.next_inbound_with_phy(b"second", PacketPhyStats::default()));
    assert_eq!(
        received.try_peek().unwrap().target,
        FrameTarget::Direct(new)
    );
    assert_eq!(notify.receiver().try_receive(), Ok(new));
}

#[test]
fn seam_randomness_advances_the_authoritative_runtime_stream() {
    let id = InterfaceId::new(*b"radio000");
    let (inbound, _) = leaked_grant_lane::<8>(1);
    let (_, outbound) = leaked_grant_lane::<8>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let mut seam = EmbassyInterfaceSeam::new(
        id,
        inbound,
        notify.sender(),
        outbound,
        crate::manifold::driver::test_support::entropy_handle(),
    );
    let oracle = crate::manifold::driver::test_support::entropy_handle();
    for _ in 0..3 {
        let mut expected = [0; 32];
        let mut actual = [0; 32];
        oracle.fill_random(&mut expected);
        seam.fill_random(&mut actual);
        assert_eq!(actual, expected);
    }
}

#[test]
fn same_identity_publication_filters_late_stale_frames_and_releases_retained_slots() {
    let current = InterfaceId::new(*b"radio000");
    let stale = InterfaceId::new(*b"radio111");
    for accept in [false, true] {
        let (inbound, _) = leaked_grant_lane::<16>(1);
        let (mut sender, outbound) = leaked_grant_lane::<16>(2);
        let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
        let mut seam = EmbassyInterfaceSeam::new(
            current,
            inbound,
            notify.sender(),
            outbound,
            crate::manifold::driver::test_support::entropy_handle(),
        );
        seam.set_channel_id(current);
        for target in [stale, current] {
            sender
                .try_grant()
                .unwrap()
                .fill_for(target, target.as_bytes());
            sender.commit();
        }
        assert_eq!(block_on(seam.next_outbound()), current.as_bytes());
        assert_eq!(seam.take_channel_change_drops(), 1);
        seam.complete_outbound(OutboundDisposition::Sent);
        sender.try_grant().unwrap().fill_for(stale, b"retained");
        sender.commit();
        seam.set_channel_id(stale);
        if accept {
            seam.accept_outbound_custody();
        } else {
            seam.complete_outbound(OutboundDisposition::Sent);
        }
        sender.try_grant().unwrap().fill_for(stale, b"next");
        sender.commit();
        assert_eq!(block_on(seam.next_outbound()), b"next");
        assert_eq!(seam.take_channel_change_drops(), 0);
    }
}
