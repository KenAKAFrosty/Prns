use super::*;
use prns_simulation::{SimulationDurationInTicks, TransmissionAction};
pub fn broadcast_faults(lab: &mut Lab<'_>, topology: Topology) {
    lab.arm(
        TARGET,
        FaultDestination::Broadcast,
        TransmissionAction::Duplicate {
            first_after: SimulationDurationInTicks::ZERO,
            second_after: SimulationDurationInTicks::from_ticks(20),
        },
    );
    lab.announce(TARGET);
    let retry_horizon =
        personal_rns::routing::announce::defaults::REBROADCAST_RETRANSMIT_INTERVAL_MS
            + 2 * personal_rns::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS
            + 1;
    assert!(lab.advance(retry_horizon).is_empty());
    let count = |lab: &Lab<'_>| {
        lab.medium
            .snapshot()
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    HaLowEvent::Transmitted {
                        destination: Destination::Broadcast,
                        ..
                    }
                )
            })
            .count()
    };
    let groups = count(lab);
    assert!(
        groups > 1
            && groups
                <= NODE_COUNT
                    * usize::from(personal_rns::routing::announce::defaults::MAX_OUR_EMISSIONS),
        "echoes and retries stay bounded"
    );
    assert!(lab.advance(10_000).is_empty());
    assert_eq!(
        count(lab),
        groups,
        "no ongoing announce loop or automatic announce policy"
    );
    lab.rediscover_via(
        PRIMARY,
        match topology {
            Topology::Chain => HEALTHY,
            Topology::Shared => TARGET,
            Topology::Asymmetric => panic!("unsupported broadcast topology"),
        },
    );
    let primary = lab.link(PRIMARY);
    lab.app(PRIMARY, primary, b"after-echoes");
    wire_contract(lab);
}
#[test]
fn broadcast_duplicates_and_onward_echoes_stop_without_adapter_deduplication() {
    for seed in [0, 1, 42, 0x5eed] {
        let run = || {
            let medium = medium();
            with_lab(medium.clone(), seed, Topology::Shared, |lab| {
                broadcast_faults(lab, Topology::Shared)
            });
            medium.snapshot()
        };
        assert_eq!(run(), run());
    }
}
#[test]
fn learned_routes_cannot_bypass_a_removed_relay_and_recover_after_restoration() {
    let medium = medium();
    with_lab(medium, 42, Topology::Chain, |lab| {
        lab.announce(TARGET);
        let primary = lab.link(PRIMARY);
        lab.app(PRIMARY, primary, b"requires-relay");
        for endpoint in [TARGET, PRIMARY, OUTSIDER] {
            lab.medium.set_path(
                lab.nodes[HEALTHY].radio,
                lab.nodes[endpoint].radio,
                PathState::Isolated,
            );
            lab.medium.set_path(
                lab.nodes[endpoint].radio,
                lab.nodes[HEALTHY].radio,
                PathState::Isolated,
            );
        }
        let task = lab.request(
            PRIMARY,
            primary,
            personal_rns::remote_control::RemoteControlRequest::DescribeBuild,
        );
        let Event::Response(reply) = lab.wait(task, 1000) else {
            panic!("partitioned request");
        };
        assert_eq!(
            reply,
            Err(personal_rns::runtime::SendError::Failed(
                personal_rns::engine::SendRequestFailure::Timeout
            ))
        );
        lab.topology(Topology::Chain);
        lab.app(PRIMARY, primary, b"restored-relay");
        assert_eq!(
            lab.peer_ids(PRIMARY),
            [scope(PRIMARY).peer_id(mac(HEALTHY))]
        );
    });
}
