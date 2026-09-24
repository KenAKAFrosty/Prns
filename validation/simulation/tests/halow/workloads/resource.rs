use super::*;
use prns_simulation::{SimulationDurationInTicks, TransmissionAction};
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResourceFault {
    Loss,
    Delay,
    Duplicate,
    PartLoss,
    ReorderedParts,
}
pub fn resource_fault(lab: &mut Lab<'_>, effect: ResourceFault) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    let action = match effect {
        ResourceFault::Loss => TransmissionAction::Drop,
        ResourceFault::Delay => TransmissionAction::Delay {
            by: SimulationDurationInTicks::from_ticks(20),
        },
        ResourceFault::Duplicate => TransmissionAction::Duplicate {
            first_after: SimulationDurationInTicks::ZERO,
            second_after: SimulationDurationInTicks::from_ticks(10),
        },
        ResourceFault::PartLoss | ResourceFault::ReorderedParts => TransmissionAction::Delay {
            by: SimulationDurationInTicks::ZERO,
        },
    };
    let before = lab.medium.snapshot().events.len();
    lab.arm(TARGET, FaultDestination::Peer(mac(PRIMARY)), action);
    match effect {
        ResourceFault::PartLoss => lab.arm(
            TARGET,
            FaultDestination::Peer(mac(PRIMARY)),
            TransmissionAction::Drop,
        ),
        ResourceFault::ReorderedParts => {
            lab.arm(
                TARGET,
                FaultDestination::Peer(mac(PRIMARY)),
                TransmissionAction::Delay {
                    by: SimulationDurationInTicks::from_ticks(20),
                },
            );
            lab.arm(
                TARGET,
                FaultDestination::Peer(mac(PRIMARY)),
                TransmissionAction::Duplicate {
                    first_after: SimulationDurationInTicks::ZERO,
                    second_after: SimulationDurationInTicks::from_ticks(10),
                },
            );
        }
        ResourceFault::Loss | ResourceFault::Delay | ResourceFault::Duplicate => {}
    }
    let resource = lab.request_bytes(
        PRIMARY,
        primary,
        crate::traffic::RESOURCE_PATH,
        Vec::new(),
        30_000,
    );
    let message =
        personal_rns::remote_control::RemoteControlAppMessage::from_slice(b"resource-neighbor")
            .expect("message");
    let other = lab.request(
        HEALTHY,
        healthy,
        personal_rns::remote_control::RemoteControlRequest::AppMessage(message.clone()),
    );
    for (task, event) in lab.wait_many(&[resource, other], 30_000) {
        let Event::Response(reply) = event else {
            panic!("resource/fairness reply");
        };
        let bytes = reply.expect("real Resource recovery");
        if task == resource {
            assert_eq!(bytes, crate::traffic::PAYLOAD);
        } else {
            assert_eq!(
                personal_rns::remote_control::RemoteControlResponse::parse(&bytes),
                Ok(
                    personal_rns::remote_control::RemoteControlResponse::AppMessage(
                        message.clone()
                    )
                )
            );
        }
    }
    assert!(lab.advance(30).is_empty());
    let snapshot = lab.medium.snapshot();
    let bytes = snapshot.events[before..]
        .iter()
        .find_map(|event| match event {
            HaLowEvent::Transmitted {
                from,
                destination: Destination::Peer(peer),
                bytes,
                ..
            } if *from == lab.nodes[TARGET].radio && *peer == mac(PRIMARY) => Some(bytes),
            _ => None,
        })
        .expect("resource boundary");
    let (header, _) = personal_rns::wire::WirePacketHeader::parse(
        personal_rns::interfaces::wifi_halow::decode(bytes).expect("envelope"),
    )
    .expect("header");
    assert_eq!(
        header.context,
        personal_rns::wire::WireContext::ResourceAdvertisement
    );
    if matches!(
        effect,
        ResourceFault::PartLoss | ResourceFault::ReorderedParts
    ) {
        let parts: Vec<_> = snapshot.events[before..]
            .iter()
            .filter_map(|event| match event {
                HaLowEvent::Transmitted {
                    ordinal,
                    from,
                    destination: Destination::Peer(peer),
                    bytes,
                    ..
                } if *from == lab.nodes[TARGET].radio && *peer == mac(PRIMARY) => {
                    personal_rns::interfaces::wifi_halow::decode(bytes)
                        .ok()
                        .and_then(|frame| personal_rns::wire::WirePacketHeader::parse(frame).ok())
                        .and_then(|(header, _)| {
                            (header.context == personal_rns::wire::WireContext::Resource)
                                .then_some(*ordinal)
                        })
                }
                _ => None,
            })
            .collect();
        assert!(
            parts.len() >= 3,
            "the payload must cross multiple actual Resource parts"
        );
        let deliveries: Vec<_> = snapshot
            .events
            .iter()
            .filter_map(|event| match event {
                HaLowEvent::Delivery {
                    ordinal,
                    to,
                    copy,
                    outcome: DeliveryOutcome::Queued,
                    ..
                } if *to == lab.nodes[PRIMARY].radio && parts.contains(ordinal) => {
                    Some((*ordinal, *copy))
                }
                _ => None,
            })
            .collect();
        match effect {
            ResourceFault::PartLoss => assert!(
                !deliveries.contains(&(parts[0], 0)),
                "fault dropped an actual Resource part"
            ),
            ResourceFault::ReorderedParts => {
                assert!(
                    deliveries
                        .iter()
                        .position(|(id, _)| *id == parts[1])
                        .expect("second part")
                        < deliveries
                            .iter()
                            .position(|(id, _)| *id == parts[0])
                            .expect("first part"),
                    "later data overtakes delayed first part"
                );
                assert!(
                    deliveries.contains(&(parts[1], 1)),
                    "actual duplicated data delivered"
                );
            }
            _ => unreachable!("part-fault owner"),
        }
    }
    assert_eq!(snapshot.armed_faults, 0);
    lab.app(PRIMARY, primary, b"resource-link-reusable");
    wire_contract(lab);
}
#[test]
fn resource_advertisement_faults_recover_exact_bytes_while_another_controller_progresses() {
    for seed in [0, 42] {
        for effect in [
            ResourceFault::Loss,
            ResourceFault::Delay,
            ResourceFault::Duplicate,
            ResourceFault::PartLoss,
            ResourceFault::ReorderedParts,
        ] {
            let run = || {
                let medium = medium();
                with_lab(medium.clone(), seed, Topology::Shared, |lab| {
                    resource_fault(lab, effect)
                });
                medium.snapshot()
            };
            assert_eq!(run(), run());
        }
    }
}
