use super::*;
use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::SendError;
use personal_rns::units::DurationMillis;
use personal_rns::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

const RESPONSE_TIMEOUT_MS: u64 = 50;

pub(super) fn expire(
    tasks: &mut EmbassyTasks<'_>,
    nodes: &[node::Node],
    links: &[LinkId],
) -> Vec<u64> {
    for (index, link) in links.iter().enumerate() {
        nodes[index ^ 1].wire.lose_after(
            WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: DestinationType::Link,
                packet_type: PacketType::Data,
                hops: 0,
                transport_id: None,
                address: link.to_address(),
                context: WireContext::Response,
            },
            0,
            NonZeroUsize::MIN,
        );
    }
    let deadline = tasks.snapshot().tick.get() + RESPONSE_TIMEOUT_MS;
    let requests = nodes
        .iter()
        .zip(links)
        .map(|(node, &link)| {
            let handle = node.handle;
            async move {
                assert_eq!(
                    handle
                        .request_with_response_timeout(
                            link,
                            RequestPathHash::of(echo::QUERY_PATH),
                            &[42],
                            RequestResponseTimeout::Exact(DurationMillis(RESPONSE_TIMEOUT_MS))
                        )
                        .await
                        .map(|_| ()),
                    Err(SendError::Failed(SendRequestFailure::Timeout))
                );
                embassy_time::Instant::now().as_millis()
            }
        })
        .collect();
    let observed = tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(deadline),
            polls_per_tick: NonZeroUsize::new(nodes.len() * 128).unwrap(),
        },
        together(requests),
    );
    assert_eq!(observed, vec![deadline; nodes.len()]);
    assert_eq!(tasks.snapshot().tick, tick(deadline));
    for node in nodes {
        assert_eq!(node.wire.stop_loss(), 1);
    }
    observed
}

#[test]
fn fleet_request_deadlines_replay_and_release_capacity_for_fresh_traffic() {
    for scheduling in [
        ManualTaskScheduling::Cyclic,
        ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(7),
        },
    ] {
        let expected = run_workload::<8>(scheduling, 42, Workload::ExpiredReplies);
        assert_eq!(
            run_workload::<8>(scheduling, 42, Workload::ExpiredReplies),
            expected
        );
        let larger = run_workload::<16>(scheduling, 42, Workload::ExpiredReplies);
        assert_eq!(
            run_workload::<16>(scheduling, 42, Workload::ExpiredReplies),
            larger
        );
        assert!(larger.timers.peak >= 16);
    }
}
