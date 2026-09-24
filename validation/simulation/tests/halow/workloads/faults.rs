use super::*;
use personal_rns::engine::SendRequestFailure;
use personal_rns::remote_control::*;
use personal_rns::runtime::SendError;
use prns_simulation::{SimulationDurationInTicks, TransmissionAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FaultLeg {
    Request,
    Response,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FaultEffect {
    Loss,
    Delay,
    Duplicate,
    Late,
}
impl FaultEffect {
    fn action(self) -> TransmissionAction {
        let ticks = SimulationDurationInTicks::from_ticks;
        match self {
            Self::Loss => TransmissionAction::Drop,
            Self::Delay => TransmissionAction::Delay { by: ticks(20) },
            Self::Duplicate => TransmissionAction::Duplicate {
                first_after: ticks(0),
                second_after: ticks(10),
            },
            Self::Late => TransmissionAction::Delay {
                by: ticks(REQUEST_TIMEOUT_MS + 20),
            },
        }
    }
}
pub fn faulted_exchange(lab: &mut Lab<'_>, leg: FaultLeg, effect: FaultEffect) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    let from = match leg {
        FaultLeg::Request => PRIMARY,
        FaultLeg::Response => TARGET,
    };
    let destination = match leg {
        FaultLeg::Request => mac(TARGET),
        FaultLeg::Response => mac(PRIMARY),
    };
    let before = lab.medium.snapshot().events.len();
    lab.arm(from, FaultDestination::Peer(destination), effect.action());
    let payload = RemoteControlAppMessage::from_slice(b"faulted").expect("message");
    let task = lab.request(
        PRIMARY,
        primary,
        RemoteControlRequest::AppMessage(payload.clone()),
    );
    let healthy_message = RemoteControlAppMessage::from_slice(b"unaffected").expect("message");
    let other = lab.request(
        HEALTHY,
        healthy,
        RemoteControlRequest::AppMessage(healthy_message.clone()),
    );
    for (found, event) in lab.wait_many(&[task, other], 1000) {
        let Event::Response(reply) = event else {
            panic!("request outcome");
        };
        if found == other {
            assert_eq!(
                RemoteControlResponse::parse(&reply.expect("healthy progress")),
                Ok(RemoteControlResponse::AppMessage(healthy_message.clone()))
            );
            continue;
        }
        match effect {
            FaultEffect::Loss | FaultEffect::Late => {
                assert_eq!(reply, Err(SendError::Failed(SendRequestFailure::Timeout)))
            }
            FaultEffect::Delay | FaultEffect::Duplicate => assert_eq!(
                RemoteControlResponse::parse(&reply.expect("fault tolerated")),
                Ok(RemoteControlResponse::AppMessage(payload.clone()))
            ),
        }
    }
    assert!(
        lab.advance(REQUEST_TIMEOUT_MS + 30).is_empty(),
        "late/duplicate copies cannot complete another waiter"
    );
    let calls = lab.calls.borrow();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.controller == controller(HEALTHY))
            .count(),
        1
    );
    let expected = match (leg, effect) {
        (FaultLeg::Request, FaultEffect::Loss) => 0,
        _ => 1,
    };
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.controller == controller(PRIMARY))
            .count(),
        expected,
        "request deduplication and late admission"
    );
    drop(calls);
    let snapshot = lab.medium.snapshot();
    let first = snapshot.events[before..]
        .iter()
        .find_map(|event| match event {
            HaLowEvent::Transmitted {
                from: sent,
                destination: Destination::Peer(peer),
                bytes,
                ..
            } if *sent == lab.nodes[from].radio && *peer == destination => Some(bytes),
            _ => None,
        })
        .expect("fault struck a transmission");
    let (header, _) = personal_rns::wire::WirePacketHeader::parse(
        personal_rns::interfaces::wifi_halow::decode(first).expect("envelope"),
    )
    .expect("header");
    assert_eq!(
        header.context,
        match leg {
            FaultLeg::Request => personal_rns::wire::WireContext::Request,
            FaultLeg::Response => personal_rns::wire::WireContext::Response,
        },
        "fault must strike the intended packet boundary"
    );
    assert_eq!(snapshot.armed_faults, 0);
    lab.app(PRIMARY, primary, b"recovered");
    wire_contract(lab);
}

#[test]
fn delivery_faults_isolate_waiters_and_preserve_verified_controller_ownership() {
    for seed in [0, 1, 42, 0x5eed] {
        for leg in [FaultLeg::Request, FaultLeg::Response] {
            for effect in [
                FaultEffect::Loss,
                FaultEffect::Delay,
                FaultEffect::Duplicate,
                FaultEffect::Late,
            ] {
                let run = || {
                    let medium = medium();
                    with_lab(medium.clone(), seed, Topology::Shared, |lab| {
                        faulted_exchange(lab, leg, effect)
                    });
                    medium.snapshot()
                };
                assert_eq!(run(), run(), "{seed} {leg:?} {effect:?}");
            }
        }
    }
}
