use crate::engine::test_support::{
    bytes_from_hex, transporting_interfaces, transporting_node, RNS_1_3_5_ANNOUNCE,
};
use crate::engine::{EngineReaction, IngestIo, Journaled};
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::routing::ingress::Ingress;
use crate::units::InstantMillis;

#[test]
fn an_accepted_announce_journals_the_identity_app_data_and_path_provenance() {
    let source_interface = InterfaceId::new([0xA7; 8]);
    let arrived_at = InstantMillis(7_000);
    let mut identity_wire = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let Ingress::Announce {
        identity_hash: expected_identity,
        ..
    } = Ingress::classify(InboundPacket {
        arrived_at,
        source_interface,
        bytes: &mut identity_wire,
    })
    else {
        panic!("reference announce should classify");
    };

    let mut engine = transporting_node();
    let interfaces = transporting_interfaces();
    let mut wire = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut heard = None;
    engine.ingest_packet_into(
        InboundPacket {
            arrived_at,
            source_interface,
            bytes: &mut wire,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&interfaces),
            now: arrived_at,
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::AnnounceHeard { observation, .. }) =
                    reaction
                {
                    heard = Some((
                        observation.announced_identity,
                        observation.hops,
                        observation.source_interface,
                        observation.arrived_at,
                        observation.app_data.to_vec(),
                        observation.is_path_response,
                    ));
                }
            },
        },
    );

    assert_eq!(
        heard,
        Some((
            expected_identity,
            crate::units::HopCount(1),
            source_interface,
            arrived_at,
            b"hello-personal".to_vec(),
            false,
        ))
    );
}
