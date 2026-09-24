use super::*;
use prns_simulation::{SimulationDurationInTicks, TransmissionAction};

pub fn lifecycle(lab: &mut Lab<'_>) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    lab.app(PRIMARY, primary, b"before-retirement");
    let old = lab.nodes[PRIMARY].radio;
    lab.arm(
        TARGET,
        FaultDestination::Peer(mac(PRIMARY)),
        TransmissionAction::Delay {
            by: SimulationDurationInTicks::from_ticks(50),
        },
    );
    let pending = lab.request(
        PRIMARY,
        primary,
        personal_rns::remote_control::RemoteControlRequest::DescribeBuild,
    );
    assert!(lab.settle().is_empty());
    assert_eq!(lab.medium.snapshot().pending, 1);
    lab.replace_adapter(PRIMARY);
    assert_ne!(old, lab.nodes[PRIMARY].radio);
    lab.topology(Topology::Shared);
    let Event::Response(result) = lab.wait(pending, 1000) else {
        panic!("old request completion");
    };
    assert_eq!(
        result,
        Err(personal_rns::runtime::SendError::Failed(
            personal_rns::engine::SendRequestFailure::Timeout
        )),
        "old adapter cannot settle as success from a stale delivery"
    );
    assert!(lab.medium.snapshot().events.iter().any(|event| matches!(event, HaLowEvent::Delivery { to, outcome: DeliveryOutcome::Detached, .. } if *to == old)));
    lab.app(HEALTHY, healthy, b"neighbor-during-replacement");
    lab.rediscover(PRIMARY);
    let replacement = lab.link(PRIMARY);
    lab.app(PRIMARY, replacement, b"replacement");
    let retired = lab.nodes[PRIMARY].radio;
    lab.medium
        .set_receive_behavior(retired, ReceiveBehavior::Failed);
    assert!(lab.settle().is_empty());
    assert!(
        lab.nodes[PRIMARY].handle.interfaces().is_empty(),
        "fatal receive detaches supervisor and children"
    );
    assert_eq!(
        lab.medium.snapshot().radios,
        NODE_COUNT - 1,
        "no automatic rebinding claim"
    );
    lab.app(HEALTHY, healthy, b"neighbor-during-failure");
    lab.replace_adapter(PRIMARY);
    lab.topology(Topology::Shared);
    lab.rediscover(PRIMARY);
    let fresh = lab.link(PRIMARY);
    lab.app(PRIMARY, fresh, b"explicit-recovery");
    lab.restart(PRIMARY);
    lab.topology(Topology::Shared);
    lab.rediscover(PRIMARY);
    let rebooted = lab.link(PRIMARY);
    lab.app(PRIMARY, rebooted, b"fresh-node");
    wire_contract(lab);
}
#[test]
fn adapter_retirement_receive_failure_and_restart_recover_without_stale_delivery() {
    for seed in [0, 42] {
        let run = || {
            let medium = medium();
            with_lab(medium.clone(), seed, Topology::Shared, lifecycle);
            medium.snapshot()
        };
        assert_eq!(run(), run());
    }
}
