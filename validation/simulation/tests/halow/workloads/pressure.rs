use super::*;
pub fn pressure(lab: &mut Lab<'_>) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Stalled);
    let pending = lab.request(
        PRIMARY,
        primary,
        personal_rns::remote_control::RemoteControlRequest::DescribeBuild,
    );
    assert!(lab.settle().is_empty());
    lab.app(HEALTHY, healthy, b"progress-during-stall");
    let Event::Response(reply) = lab.wait(pending, 1000) else {
        panic!("stalled request");
    };
    assert_eq!(
        reply,
        Err(personal_rns::runtime::SendError::Failed(
            personal_rns::engine::SendRequestFailure::Timeout
        ))
    );
    assert!(
        lab.advance(2001).is_empty(),
        "production send timeout releases the canceled operation"
    );
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Ready);
    assert!(lab.settle().is_empty());
    lab.app(PRIMARY, primary, b"after-send-timeout");
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Failed);
    let failed = lab.request(
        PRIMARY,
        primary,
        personal_rns::remote_control::RemoteControlRequest::DescribeBuild,
    );
    let Event::Response(reply) = lab.wait(failed, 1000) else {
        panic!("failed send");
    };
    assert_eq!(
        reply,
        Err(personal_rns::runtime::SendError::Failed(
            personal_rns::engine::SendRequestFailure::Timeout
        ))
    );
    lab.app(HEALTHY, healthy, b"progress-during-send-failure");
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Ready);
    assert!(lab.settle().is_empty());
    lab.app(PRIMARY, primary, b"after-send-failure");
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Failed);
    for from in [TARGET, HEALTHY, OUTSIDER] {
        lab.medium.set_path(
            lab.nodes[from].radio,
            lab.nodes[PRIMARY].radio,
            PathState::Isolated,
        );
    }
    assert!(lab.advance(15_001).is_empty());
    assert!(lab.peer_ids(PRIMARY).is_empty(), "idle peers reclaimed");
    lab.medium
        .set_send_behavior(lab.nodes[PRIMARY].radio, SendBehavior::Ready);
    lab.topology(Topology::Shared);
    lab.rediscover(PRIMARY);
    assert!(
        lab.peer_ids(PRIMARY)
            .contains(&scope(PRIMARY).peer_id(mac(TARGET))),
        "same MAC re-admitted on a valid first frame"
    );
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"after-expiry");
    wire_contract(lab);
}
#[test]
fn stalled_and_failed_sends_do_not_block_other_neighbors_and_expiry_is_reusable() {
    for seed in [0, 42] {
        let run = || {
            let medium = medium();
            with_lab(medium.clone(), seed, Topology::Shared, pressure);
            medium.snapshot()
        };
        assert_eq!(run(), run());
    }
}
