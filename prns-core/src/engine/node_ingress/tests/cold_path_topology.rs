//! Four independent engines and six directed interface ends. No startup announces,
//! shared route state, local-client shortcut, or direct phone-to-phone carrier.
use crate::engine::test_support::{
    fixed_secret_key, pin_transport_id, routable_descriptor, second_secret_key, TestStorageLayout,
};
use crate::engine::{
    CommandId, Directive, EngineProtocolPolicy, EngineReaction, EngineState, FanTarget, IngestIo,
    InstantMillis, Journaled, PathFound, PathRequestId, RatchetPolicy, RecursivePathRequestDefault,
    RequestPath, Settlement,
};
use crate::interfaces::{
    AttachedInterfaces, BitrateBps, InboundPacket, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use crate::routing::path_requests::request_path::PathRequestWriteOutcome;
use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use crate::routing::warmth::Departure;
use crate::units::HopCount;
use crate::wire::{DestinationHash, TransportId, BROADCAST_MTU};
use std::collections::VecDeque;

type Frame = (usize, InterfaceId, std::vec::Vec<u8>);

struct Topology {
    nodes: [EngineState<TestStorageLayout>; 4],
    interfaces: [std::vec::Vec<InterfaceDescriptor>; 4],
    endpoints: [DestinationHash; 2],
    frames: VecDeque<Frame>,
    settled: std::vec::Vec<(usize, CommandId, Settlement)>,
    now: u64,
    entropy: u8,
}

impl Topology {
    fn cold() -> Self {
        let mut nodes = [
            EngineState::new(fixed_secret_key()),
            EngineState::default(),
            EngineState::default(),
            EngineState::new(second_secret_key()),
        ];
        let endpoints = [0, 3].map(|index| {
            let identity = nodes[index].held_identity_hashes()[0];
            nodes[index]
                .register_single_destination(
                    &identity,
                    "cold",
                    &["endpoint"],
                    &[],
                    ProofStrategy::ProveNone,
                    LinkRequestPolicy::AcceptAll,
                    RatchetPolicy::NoRatchets,
                )
                .unwrap()
        });
        for (index, id) in [(1, 0xA1), (2, 0xB2)] {
            pin_transport_id(&mut nodes[index], TransportId::new([id; 16]));
            // Matches EMBEDDED_HOPSPOT_PROTOCOL_POLICY: Full interfaces inherit
            // the appliance's explicitly enabled recursive discovery policy.
            nodes[index].set_protocol_policy(EngineProtocolPolicy {
                recursive_path_request_default: RecursivePathRequestDefault::Enabled,
                ..EngineProtocolPolicy::default()
            });
        }
        let interfaces = [
            std::vec![descriptor(0x10, false)],
            std::vec![descriptor(0x11, false), descriptor(0x12, true)],
            std::vec![descriptor(0x21, true), descriptor(0x22, false)],
            std::vec![descriptor(0x30, false)],
        ];
        Self {
            nodes,
            interfaces,
            endpoints,
            frames: VecDeque::new(),
            settled: std::vec::Vec::new(),
            now: 1_000,
            entropy: 1,
        }
    }

    fn request(&mut self, phone: usize, tag: u8) {
        let destination = self.endpoints[usize::from(phone == 0)];
        let mut wire = [0; BROADCAST_MTU];
        let PathRequestWriteOutcome::Written { wire_bytes, .. } = self.nodes[phone]
            .write_commanded_path_request_with_interfaces(
                CommandId(u64::from(tag)),
                &RequestPath {
                    destination,
                    id: PathRequestId::new([tag; 16]),
                },
                InstantMillis(self.now),
                AttachedInterfaces::new(&self.interfaces[phone]),
                &mut wire,
            )
        else {
            panic!("bounded request fits");
        };
        self.frames.push_back((
            phone,
            self.interfaces[phone][0].id,
            wire[..wire_bytes].to_vec(),
        ));
    }

    fn peer(&self, node: usize, interface: InterfaceId) -> (usize, InterfaceId) {
        let side = self.interfaces[node]
            .iter()
            .position(|value| value.id == interface)
            .unwrap();
        match (node, side) {
            (0, 0) => (1, self.interfaces[1][0].id),
            (1, 0) => (0, self.interfaces[0][0].id),
            (1, 1) => (2, self.interfaces[2][0].id),
            (2, 0) => (1, self.interfaces[1][1].id),
            (2, 1) => (3, self.interfaces[3][0].id),
            (3, 0) => (2, self.interfaces[2][1].id),
            _ => panic!("isolated line only"),
        }
    }

    fn drive(&mut self) {
        for _ in 0..100 {
            if let Some((source, interface, mut bytes)) = self.frames.pop_front() {
                let (peer, arrived_on) = self.peer(source, interface);

                self.now += if (source == 1 && peer == 2) || (source == 2 && peer == 1) {
                    1_000
                } else {
                    10
                };
                let now = InstantMillis(self.now);
                self.entropy = self.entropy.wrapping_add(1);
                crate::engine::drive_packet_to_quiescence(
                    &mut self.nodes[peer],
                    InboundPacket {
                        arrived_at: now,
                        source_interface: arrived_on,
                        bytes: &mut bytes,
                    },
                    IngestIo {
                        interfaces: AttachedInterfaces::new(&self.interfaces[peer]),
                        now,
                        fill_random: &mut |bytes: &mut [u8]| bytes.fill(self.entropy),
                        should_prove: &mut |_| false,
                        should_accept_resource: &mut |_| false,
                        sink: &mut |reaction| {
                            capture(
                                peer,
                                reaction,
                                &self.interfaces[peer],
                                &mut self.frames,
                                &mut self.settled,
                            )
                        },
                    },
                );
            } else {
                for index in 0..4 {
                    self.nodes[index].fire_due_scheduled_announces(
                        InstantMillis(self.now + 500),
                        AttachedInterfaces::new(&self.interfaces[index]),
                        &mut |reaction| {
                            capture(
                                index,
                                reaction,
                                &self.interfaces[index],
                                &mut self.frames,
                                &mut self.settled,
                            )
                        },
                    );
                }
                self.now += 500;
                if self.frames.is_empty()
                    && self
                        .nodes
                        .iter()
                        .all(|node| node.scheduled_announce_count() == 0)
                {
                    return;
                }
            }
        }
        panic!("path exchange did not quiesce");
    }

    fn assert_discovered(&self, phone: usize, tag: u8) {
        assert!(
            self.settled.contains(&(
                phone,
                CommandId(u64::from(tag)),
                Settlement::RequestPath(Ok(PathFound { hops: HopCount(3) }))
            )),
            "settlements: {:?}",
            self.settled
        );
        let remote = self.endpoints[usize::from(phone == 0)];
        assert_eq!(
            self.nodes[phone].routing_table.hop_count_to(&remote),
            Some(3)
        );
    }

    fn replace_interface(&mut self, node: usize, side: usize, id: u8) {
        let old = self.interfaces[node][side].id;
        self.nodes[node].interface_departed(old, Departure::Forgotten, InstantMillis(self.now));
        self.interfaces[node][side].id = InterfaceId::from_channel_tag(old.kind().unwrap(), &[id]);
        self.nodes[node].cull_expired_routes(
            InstantMillis(self.now),
            AttachedInterfaces::new(&self.interfaces[node]),
            &mut |_| {},
        );
    }
}

fn descriptor(id: u8, radio: bool) -> InterfaceDescriptor {
    InterfaceDescriptor {
        bitrate: BitrateBps::guess(if radio { 696 } else { 1_000_000 }),
        ..routable_descriptor(InterfaceId::from_channel_tag(
            if radio {
                InterfaceKind::LoRa
            } else {
                InterfaceKind::BluetoothPeer
            },
            &[id],
        ))
    }
}

fn capture<Work>(
    node: usize,
    reaction: EngineReaction<'_, Work>,
    interfaces: &[InterfaceDescriptor],
    frames: &mut VecDeque<Frame>,
    settled: &mut std::vec::Vec<(usize, CommandId, Settlement)>,
) {
    match reaction {
        EngineReaction::Directive(
            Directive::Send { target, bytes } | Directive::SendAnnounce { target, bytes, .. },
        ) => frames.push_back((node, target, bytes.to_vec())),
        EngineReaction::Directive(Directive::SendIfOnline {
            target,
            bytes,
            on_send,
        }) => {
            on_send();
            frames.push_back((node, target, bytes.to_vec()));
        }
        EngineReaction::Directive(
            Directive::SendToFleet {
                supervisor,
                fan,
                bytes,
            }
            | Directive::SendAnnounceToFleet {
                supervisor,
                fan,
                bytes,
                ..
            },
        ) => {
            for descriptor in interfaces {
                if descriptor.id.kind().and_then(InterfaceKind::fanout_kind) == Some(supervisor)
                    && match fan {
                        FanTarget::All => true,
                        FanTarget::Only(target) => target == descriptor.id,
                        FanTarget::AllExcept(excluded) => excluded != descriptor.id,
                    }
                {
                    frames.push_back((node, descriptor.id, bytes.to_vec()));
                }
            }
        }
        EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
            settled.push((node, id, settlement))
        }
        _ => {}
    }
}

#[test]
fn cold_full_mode_appliance_relays_discover_both_phones_without_announcements() {
    let mut topology = Topology::cold();
    assert!(topology.nodes.iter().all(|node| node.route_count() == 0));
    topology.request(0, 1);
    topology.request(3, 2);
    topology.drive();
    topology.assert_discovered(0, 1);
    topology.assert_discovered(3, 2);
}

#[test]
fn cold_discovery_recovers_after_radio_and_phone_interface_replacement() {
    let mut topology = Topology::cold();
    topology.request(0, 1);
    topology.request(3, 2);
    topology.drive();
    topology.assert_discovered(0, 1);
    topology.assert_discovered(3, 2);
    for (node, side, id) in [(1, 1, 0x41), (2, 0, 0x42), (2, 1, 0x43), (3, 0, 0x44)] {
        topology.replace_interface(node, side, id);
    }
    topology.request(0, 3);
    topology.request(3, 4);
    topology.drive();
    topology.assert_discovered(0, 3);
    topology.assert_discovered(3, 4);
}
