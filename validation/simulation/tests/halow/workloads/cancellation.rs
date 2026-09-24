use super::*;
use personal_rns::remote_control::*;
use prns_simulation::{ManualTaskCancellation, SimulationDurationInTicks, TransmissionAction};
pub fn cancel_reply(lab: &mut Lab<'_>) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    lab.arm(
        TARGET,
        FaultDestination::Peer(mac(PRIMARY)),
        TransmissionAction::Delay {
            by: SimulationDurationInTicks::from_ticks(40),
        },
    );
    let message = RemoteControlAppMessage::from_slice(b"admitted-before-cancel").expect("message");
    let waiter = lab.request(PRIMARY, primary, RemoteControlRequest::AppMessage(message));
    assert!(lab.settle().is_empty());
    assert_eq!(
        lab.runner.cancel(waiter).expect("cancel local waiter"),
        ManualTaskCancellation::Cancelled
    );
    assert!(
        lab.calls.borrow().contains(&AppInvocation {
            controller: controller(PRIMARY),
            payload: b"admitted-before-cancel".to_vec()
        }),
        "admitted app work is not rolled back"
    );
    lab.app(PRIMARY, primary, b"fresh-waiter");
    lab.app(HEALTHY, healthy, b"other-controller");
    assert!(
        lab.advance(50).is_empty(),
        "orphan response cannot settle a replacement actor"
    );
    lab.app(PRIMARY, primary, b"after-orphan");
}
#[test]
fn canceled_local_waiter_does_not_retract_admission_or_capture_another_response() {
    let medium = medium();
    with_lab(medium, 42, Topology::Shared, cancel_reply);
}
