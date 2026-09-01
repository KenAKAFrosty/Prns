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
