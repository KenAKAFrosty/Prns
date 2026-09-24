use super::fixture::*;
use personal_rns::wifi_halow::Destination;
use prns_simulation::halow::*;
mod boot;
mod broadcast;
mod cancellation;
mod control;
mod faults;
mod lifecycle;
mod pressure;
mod receive;
pub mod recovery;
mod resource;
pub use broadcast::broadcast_faults;
pub use cancellation::cancel_reply;
pub use control::overlap;
pub use faults::{faulted_exchange, FaultEffect, FaultLeg};
pub use lifecycle::lifecycle;
pub use pressure::pressure;
pub use receive::receive_pressure;
pub use resource::{resource_fault, ResourceFault};

pub fn baseline(lab: &mut Lab<'_>, topology: Topology) {
    if topology == Topology::Asymmetric {
        lab.topology(Topology::Shared);
    }
    for index in [TARGET, PRIMARY, HEALTHY] {
        lab.announce(index);
    }
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    if topology == Topology::Asymmetric {
        lab.topology(topology);
        let task = lab.request(
            PRIMARY,
            primary,
            personal_rns::remote_control::RemoteControlRequest::DescribeBuild,
        );
        let Event::Response(reply) = lab.wait(task, 1000) else {
            panic!("asymmetric timeout");
        };
        assert_eq!(
            reply,
            Err(personal_rns::runtime::SendError::Failed(
                personal_rns::engine::SendRequestFailure::Timeout
            ))
        );
        lab.app(HEALTHY, healthy, b"asymmetric-healthy");
        lab.topology(Topology::Shared);
    }
    lab.app(PRIMARY, primary, b"first-unicast");
    lab.app(HEALTHY, healthy, b"healthy");
    let task = lab.request_bytes(
        PRIMARY,
        primary,
        crate::traffic::RESOURCE_PATH,
        Vec::new(),
        10_000,
    );
    let Event::Response(reply) = lab.wait(task, 10_000) else {
        panic!("resource reply");
    };
    assert_eq!(reply.expect("complete resource"), crate::traffic::PAYLOAD);
    match topology {
        Topology::Chain => assert_eq!(
            lab.peer_ids(PRIMARY),
            [scope(PRIMARY).peer_id(mac(HEALTHY))]
        ),
        Topology::Shared | Topology::Asymmetric => assert!(lab
            .peer_ids(PRIMARY)
            .contains(&scope(PRIMARY).peer_id(mac(TARGET)))),
    }
    wire_contract(lab);
}
pub fn wire_contract(lab: &Lab<'_>) {
    let mut groups = 0;
    let mut directed = 0;
    for event in &lab.medium.snapshot().events {
        if let HaLowEvent::Transmitted {
            destination, bytes, ..
        } = event
        {
            let frame =
                personal_rns::interfaces::wifi_halow::decode(bytes).expect("production envelope");
            let (header, _) =
                personal_rns::wire::WirePacketHeader::parse(frame).expect("wire header");
            match destination {
                Destination::Broadcast => {
                    assert_eq!(header.packet_type, personal_rns::wire::PacketType::Announce);
                    groups += 1;
                }
                Destination::Peer(_) => directed += 1,
            }
        }
    }
    assert!(groups > 0 && directed > 0);
}

#[test]
fn real_nodes_replay_source_mac_broadcast_and_two_hop_resources() {
    for seed in [0, 1, 42, 0x5eed] {
        for topology in [Topology::Shared, Topology::Chain, Topology::Asymmetric] {
            let run = || {
                let medium = medium();
                with_lab(medium.clone(), seed, topology, |lab| {
                    baseline(lab, topology)
                });
                medium.snapshot()
            };
            assert_eq!(run(), run(), "seed {seed}, topology {topology:?}");
        }
    }
}
