//! Diagnostics are observations: turning collection on must not change egress,
//! expiry, cached-evidence acceptance, or command settlement.
use crate::engine::test_support::{routable_descriptor, transporting_node};
use crate::engine::{
    CommandId, Directive, EngineProtocolPolicy, EngineReaction, IngestIo, InstantMillis, Journaled,
    PathDiscoveryPhase, PathRequestDisposition, PathRequestEgress, PathRequestId,
    RecursivePathRequestDefault, RequestPath, RequestPathFailure, Settlement,
};
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::routing::path_requests::write_path_request_wire_packet;
use crate::wire::{DestinationHash, BROADCAST_MTU};

#[test]
fn phase_trace_distinguishes_offline_egress_and_expires_each_obligation_once() {
    for online in [false, true] {
        let source = InterfaceId::new([0xA1; 8]);
        let onward = InterfaceId::new([0xB2; 8]);
        let descriptors = [routable_descriptor(source), routable_descriptor(onward)];
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut relay = transporting_node();
        relay.set_protocol_policy(EngineProtocolPolicy {
            recursive_path_request_default: RecursivePathRequestDefault::Enabled,
            ..Default::default()
        });
        let destination = DestinationHash::new([0x44; 16]);
        let mut wire = [0; BROADCAST_MTU];
        let size =
            write_path_request_wire_packet(destination, None, &[0x33; 16], &mut wire).unwrap();
        let mut events = std::vec::Vec::new();
        let mut attempts = 0;
        relay.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut wire[..size],
            },
            IngestIo {
                interfaces,
                now: InstantMillis(1_000),
                fill_random: &mut |_| {},
                should_prove: &mut |_| false,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::SendIfOnline {
                        on_send, target, ..
                    }) => {
                        assert_eq!(target, onward);
                        attempts += 1;
                        if online {
                            on_send();
                        }
                    }
                    EngineReaction::Journaled(Journaled::PathDiscovery(event)) => {
                        events.push(event)
                    }
                    _ => {}
                },
            },
        );
        assert_eq!(attempts, 1);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].phase,
            PathDiscoveryPhase::RequestIngress {
                disposition: PathRequestDisposition::RecursiveForward,
                recursive: true,
                ..
            }
        ));
        assert_eq!(
            events[1].phase,
            PathDiscoveryPhase::RequestEgress(if online {
                PathRequestEgress::Dispatched
            } else {
                PathRequestEgress::Offline
            })
        );
        assert!(events
            .iter()
            .all(|event| event.destination == Some(destination)
                && event.request_tag == Some([0x33; 16])));

        let command = CommandId(99);
        let _ = relay.write_commanded_path_request_with_interfaces(
            command,
            &RequestPath {
                destination,
                id: PathRequestId::new([0x55; 16]),
            },
            InstantMillis(1_000),
            interfaces,
            &mut wire,
        );
        let mut settlements = std::vec::Vec::new();
        for _ in 0..2 {
            relay.settle_timed_out_path_requests(InstantMillis(60_000), &mut |reaction| {
                match reaction {
                    EngineReaction::Journaled(Journaled::PathDiscovery(event)) => {
                        events.push(event)
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        settlements.push((id, settlement))
                    }
                    _ => {}
                }
            });
        }
        assert_eq!(
            settlements,
            [(
                command,
                Settlement::RequestPath(Err(RequestPathFailure::Timeout))
            )]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.phase,
                    PathDiscoveryPhase::LocalTimeout {
                        command: CommandId(99)
                    }
                ))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.phase, PathDiscoveryPhase::RecursiveTimeout))
                .count(),
            1
        );
        assert_eq!(events.last().unwrap().interface, Some(source));
        assert_eq!(events.last().unwrap().destination, Some(destination));
    }
}
