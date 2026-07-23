use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use crate::interfaces::{InterfaceId, PacketPhyStats};
use crate::manifold::grant::GrantConsumer;
use crate::manifold::interface_seam::InterfaceSeam;

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
    let mut seam =
        EmbassyInterfaceSeam::new(interface, inbound, notify.sender(), outbound, |bytes| {
            bytes.fill(0)
        });

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
