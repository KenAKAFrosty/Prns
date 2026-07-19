use super::*;

pub struct Cycle {
    initiator: EngineState<GrowableHeap>,
    responder: EngineState<GrowableHeap>,
    initiator_entropy: Splitmix,
    responder_entropy: Splitmix,
    interfaces: Vec<InterfaceDescriptor>,
    destination: DestinationHash,
    payload: [u8; PAYLOAD_LEN],
    next_id: u64,
    sealed: Vec<u8>,
    pub proof: Vec<u8>,
}

impl Default for Cycle {
    fn default() -> Self {
        Self::new()
    }
}

impl Cycle {
    pub fn new() -> Self {
        let mut responder =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]));
        let responder_identity = responder.held_identity_hashes()[0];
        let destination = responder
            .register_single_destination(
                &responder_identity,
                "bench",
                &["cycle"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the bench destination");
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x22; IDENTITY_SECRET_KEY_LEN]));
        let interfaces = vec![tcp_core::descriptor(
            WIRE,
            tcp_core::policy_for_bitrate(tcp_core::TCP_BITRATE_ESTIMATE),
        )];

        let mut cycle = Self {
            initiator,
            responder,
            initiator_entropy: Splitmix(1),
            responder_entropy: Splitmix(2),
            interfaces,
            destination,
            payload: [0xAB; PAYLOAD_LEN],
            next_id: 1,
            sealed: Vec::with_capacity(1024),
            proof: Vec::with_capacity(1024),
        };

        let mut announce = Vec::with_capacity(1024);
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        cycle.responder.ingest_command_into(
            issued,
            AttachedInterfaces::new(&cycle.interfaces),
            NOW,
            &mut |bytes| cycle.responder_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    announce.extend_from_slice(bytes);
                }
            },
        );
        assert!(!announce.is_empty(), "responder emitted its announce");

        let mut heard = false;
        cycle.initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: &mut announce,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&cycle.interfaces),
                now: NOW,
                fill_entropy: &mut |bytes| cycle.initiator_entropy.fill(bytes),
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
        assert!(heard, "initiator learned the destination");
        cycle
    }

    pub fn seal(&mut self) {
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
            interfaces,
            sealed,
            ..
        } = self;
        sealed.clear();
        initiator.ingest_command_into(
            issued,
            AttachedInterfaces::new(interfaces),
            NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    sealed.extend_from_slice(bytes);
                }
            },
        );
        assert!(!self.sealed.is_empty(), "send sealed a frame");
    }

    pub fn deliver_prove(&mut self) {
        let mut delivered = false;
        let Self {
            responder,
            responder_entropy,
            interfaces,
            sealed,
            proof,
            ..
        } = self;
        proof.clear();
        responder.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: sealed,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(interfaces),
                now: NOW,
                fill_entropy: &mut |bytes| responder_entropy.fill(bytes),
                should_prove: &mut |_| true,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Journaled(Journaled::Delivered(Delivery::Single(_))) => {
                        delivered = true;
                    }
                    EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                        proof.extend_from_slice(bytes);
                    }
                    _ => {}
                },
            },
        );
        assert!(delivered, "responder delivered the single");
        assert!(!self.proof.is_empty(), "responder proved the single");
    }

    pub fn settle(&mut self) {
        let mut proof = core::mem::take(&mut self.proof);
        self.settle_frame(&mut proof);
        self.proof = proof;
    }

    pub fn settle_frame(&mut self, proof: &mut [u8]) {
        let mut settled = false;
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            ..
        } = self;
        initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: proof,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(interfaces),
                now: NOW,
                fill_entropy: &mut |bytes| initiator_entropy.fill(bytes),
                should_prove: &mut |_| true,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::CommandSettled {
                        settlement: Settlement::SendSinglePacket(Ok(_)),
                        ..
                    }) = reaction
                    {
                        settled = true;
                    }
                },
            },
        );
        assert!(settled, "proof verified and the receipt settled");
    }
}
