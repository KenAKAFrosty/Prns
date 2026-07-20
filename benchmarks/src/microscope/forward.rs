use super::*;

pub struct Forward {
    upstream: EngineState<GrowableHeap>,
    relay: EngineState<GrowableHeap>,
    initiator: EngineState<GrowableHeap>,
    upstream_entropy: Splitmix,
    relay_entropy: Splitmix,
    initiator_entropy: Splitmix,
    up_view: Vec<InterfaceDescriptor>,
    relay_interfaces: Vec<InterfaceDescriptor>,
    down_interfaces: Vec<InterfaceDescriptor>,
    destination: DestinationHash,
    payload: [u8; PAYLOAD_LEN],
    next_id: u64,
    single: Vec<u8>,
    scratch: Vec<u8>,
}

impl Default for Forward {
    fn default() -> Self {
        Self::new()
    }
}

impl Forward {
    pub fn new() -> Self {
        let mut upstream =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x41; IDENTITY_SECRET_KEY_LEN]));
        let upstream_identity = upstream.held_identity_hashes()[0];
        let destination = upstream
            .register_single_destination(
                &upstream_identity,
                "bench",
                &["forward"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the forward destination");
        let relay =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x52; IDENTITY_SECRET_KEY_LEN]));
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x63; IDENTITY_SECRET_KEY_LEN]));

        let mut forward = Self {
            upstream,
            relay,
            initiator,
            upstream_entropy: Splitmix(11),
            relay_entropy: Splitmix(22),
            initiator_entropy: Splitmix(33),
            up_view: vec![tcp::descriptor(
                IF_UP,
                tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
            )],
            relay_interfaces: vec![
                tcp::descriptor(
                    IF_UP,
                    tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
                ),
                tcp::descriptor(
                    IF_DOWN,
                    tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
                ),
            ],
            down_interfaces: vec![tcp::descriptor(
                IF_DOWN,
                tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
            )],
            destination,
            payload: [0xCD; PAYLOAD_LEN],
            next_id: 1,
            single: Vec::with_capacity(1024),
            scratch: vec![0u8; MAX_WIRE_FRAME_LEN],
        };

        forward.learn_routes();
        forward
    }

    fn learn_routes(&mut self) {
        let mut announce = Vec::with_capacity(1024);
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination: self.destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        {
            let Self {
                upstream,
                upstream_entropy,
                up_view,
                ..
            } = self;
            upstream.ingest_command_into(
                issued,
                AttachedInterfaces::new(up_view),
                SETUP_NOW,
                &mut |bytes| upstream_entropy.fill(bytes),
                &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                        announce.extend_from_slice(bytes);
                    }
                },
            );
        }
        assert!(!announce.is_empty(), "upstream emitted its announce");

        {
            let Self {
                relay,
                relay_entropy,
                relay_interfaces,
                ..
            } = self;
            let mut heard = false;
            relay.ingest_packet_into(
                InboundPacket {
                    arrived_at: SETUP_NOW,
                    source_interface: IF_UP,
                    bytes: &mut announce,
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(relay_interfaces),
                    now: SETUP_NOW,
                    fill_entropy: &mut |bytes| relay_entropy.fill(bytes),
                    should_prove: &mut |_| true,
                    should_accept_resource: &mut |_| false,
                    sink: &mut |reaction| {
                        if matches!(
                            reaction,
                            EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                        ) {
                            heard = true;
                        }
                    },
                },
            );
            assert!(heard, "relay heard the upstream announce");
        }
        assert_eq!(self.relay.route_count(), 1, "relay learned the route");

        let mut rebroadcast = Vec::with_capacity(1024);
        {
            let Self {
                relay,
                relay_interfaces,
                ..
            } = self;
            relay.fire_due_scheduled_announces(
                REBROADCAST_NOW,
                AttachedInterfaces::new(relay_interfaces),
                &mut |reaction| {
                    if let EngineReaction::Directive(Directive::SendAnnounce {
                        bytes,
                        target,
                        ..
                    }) = reaction
                    {
                        if target == IF_DOWN {
                            rebroadcast.extend_from_slice(bytes);
                        }
                    }
                },
            );
        }
        assert!(
            !rebroadcast.is_empty(),
            "relay rebroadcast the announce downstream"
        );

        {
            let Self {
                initiator,
                initiator_entropy,
                down_interfaces,
                ..
            } = self;
            let mut heard = false;
            initiator.ingest_packet_into(
                InboundPacket {
                    arrived_at: REBROADCAST_NOW,
                    source_interface: IF_DOWN,
                    bytes: &mut rebroadcast,
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(down_interfaces),
                    now: REBROADCAST_NOW,
                    fill_entropy: &mut |bytes| initiator_entropy.fill(bytes),
                    should_prove: &mut |_| true,
                    should_accept_resource: &mut |_| false,
                    sink: &mut |reaction| {
                        if matches!(
                            reaction,
                            EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                        ) {
                            heard = true;
                        }
                    },
                },
            );
            assert!(heard, "initiator heard the relayed announce");
        }
    }

    pub fn seal_single(&mut self) {
        let issued = IssuedCommand {
            id: CommandId(self.next_id),
            command: EngineCommand::SendSinglePacket(SendSinglePacket {
                destination: self.destination,
                payload: SendSinglePacketPayload::from_slice(&self.payload).expect("payload fits"),
            }),
        };
        self.next_id += 1;
        let Self {
            initiator,
            initiator_entropy,
            down_interfaces,
            single,
            ..
        } = self;
        single.clear();
        initiator.ingest_command_into(
            issued,
            AttachedInterfaces::new(down_interfaces),
            FORWARD_NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    single.extend_from_slice(bytes);
                }
            },
        );
        assert!(
            !self.single.is_empty(),
            "initiator sealed a single via the relay"
        );
    }

    pub fn seal_many(&mut self, count: usize) -> Vec<Vec<u8>> {
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            self.seal_single();
            frames.push(self.single.clone());
        }
        frames
    }

    pub fn forward(&mut self) -> bool {
        let mut single = core::mem::take(&mut self.single);
        let forwarded = self.forward_frame(&mut single);
        self.single = single;
        forwarded
    }

    pub fn forward_frame(&mut self, frame: &mut [u8]) -> bool {
        let mut forwarded = false;
        let Self {
            relay,
            relay_entropy,
            relay_interfaces,
            scratch,
            ..
        } = self;
        relay.ingest_packet_into(
            InboundPacket {
                arrived_at: FORWARD_NOW,
                source_interface: IF_DOWN,
                bytes: frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(relay_interfaces),
                now: FORWARD_NOW,
                fill_entropy: &mut |bytes| relay_entropy.fill(bytes),
                should_prove: &mut |_| true,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::EmitFrame {
                        target, fill, ..
                    }) = reaction
                    {
                        if target == IF_UP && fill(&mut scratch[..]).is_some() {
                            forwarded = true;
                        }
                    }
                },
            },
        );
        forwarded
    }
}
