use super::*;

#[derive(Debug, Clone)]
pub struct ResourceTransferProfile {
    pub payload_len: usize,
    pub sender_offer: Duration,
    pub receiver_accept: Duration,
    pub sender_serve: Duration,
    pub receiver_receive: Duration,
    pub initiator_settle: Duration,
    pub requests: u64,
    pub advertisements: u64,
    pub parts: u64,
    pub hashmap_updates: u64,
    pub proofs: u64,
    pub wire_bytes: u64,
}

impl ResourceTransferProfile {
    pub fn new(payload_len: usize) -> Self {
        Self {
            payload_len,
            sender_offer: Duration::ZERO,
            receiver_accept: Duration::ZERO,
            sender_serve: Duration::ZERO,
            receiver_receive: Duration::ZERO,
            initiator_settle: Duration::ZERO,
            requests: 0,
            advertisements: 0,
            parts: 0,
            hashmap_updates: 0,
            proofs: 0,
            wire_bytes: 0,
        }
    }

    pub fn add_assign(&mut self, other: &Self) {
        self.payload_len = other.payload_len;
        self.sender_offer += other.sender_offer;
        self.receiver_accept += other.receiver_accept;
        self.sender_serve += other.sender_serve;
        self.receiver_receive += other.receiver_receive;
        self.initiator_settle += other.initiator_settle;
        self.requests += other.requests;
        self.advertisements += other.advertisements;
        self.parts += other.parts;
        self.hashmap_updates += other.hashmap_updates;
        self.proofs += other.proofs;
        self.wire_bytes += other.wire_bytes;
    }

    pub fn stage_total(&self) -> Duration {
        self.sender_offer
            + self.receiver_accept
            + self.sender_serve
            + self.receiver_receive
            + self.initiator_settle
    }
}

pub struct ResourceCycle {
    initiator: EngineState<GrowableHeap>,
    responder: EngineState<GrowableHeap>,
    initiator_entropy: Splitmix,
    responder_entropy: Splitmix,
    interfaces: Vec<InterfaceDescriptor>,
    destination: DestinationHash,
    link_id: LinkId,
    payload: Vec<u8>,
    next_id: u64,
    now: u64,
    scratch: Vec<u8>,
}

impl ResourceCycle {
    pub fn new(payload_len: usize) -> Self {
        let mut responder =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x91; IDENTITY_SECRET_KEY_LEN]));
        let responder_identity = responder.held_identity_hashes()[0];
        let destination = responder
            .register_single_destination(
                &responder_identity,
                "bench",
                &["resource"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the resource destination");
        assert!(responder.set_default_resource_strategy(
            &destination,
            ResourceStrategy::Accept {
                // Admits a bulk transfer's total: every segment advertises the original total
                // (RNS 1.4.0 parity), so the ceiling is the whole resource, not one segment.
                max_uncompressed_bytes: 256 * 1024 * 1024,
                accept_compressed: false,
            },
        ));
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x92; IDENTITY_SECRET_KEY_LEN]));
        let mut cycle = Self {
            initiator,
            responder,
            initiator_entropy: Splitmix(101),
            responder_entropy: Splitmix(202),
            interfaces: vec![tcp::descriptor(
                WIRE,
                tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
            )],
            destination,
            link_id: LinkId::new([0; 16]),
            payload: deterministic_payload(payload_len),
            next_id: 2,
            now: 1_000,
            scratch: vec![0u8; MAX_WIRE_FRAME_LEN],
        };

        let announce = cycle.announce_destination();
        let heard = cycle.feed_initiator(announce).announce_heard;
        assert!(heard, "initiator heard resource destination");

        let request = cycle.issue_link_request();
        let proof = cycle.feed_responder(request).only_frame("link proof");
        let proof_response = cycle.feed_initiator(proof);
        let link_id = proof_response
            .settlements
            .iter()
            .find_map(|(_, settlement)| match settlement {
                Settlement::EstablishLink(Ok(established)) => Some(established.link_id),
                _ => None,
            })
            .expect("initiator settles the link");
        let rtt = proof_response.only_frame("link rtt");
        let responder_up = cycle.feed_responder(rtt);
        assert!(
            responder_up.link_established.is_some(),
            "responder activates on the rtt"
        );
        cycle.link_id = link_id;
        cycle
    }

    fn tick(&mut self) -> InstantMillis {
        self.now += 1;
        InstantMillis(self.now)
    }

    fn announce_destination(&mut self) -> Vec<u8> {
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination: self.destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        let now = self.tick();
        let Self {
            responder,
            responder_entropy,
            interfaces,
            scratch,
            ..
        } = self;
        let mut capture = FeedCapture::default();
        responder.ingest_command_into(
            issued,
            AttachedInterfaces::new(interfaces),
            now,
            &mut |bytes| responder_entropy.fill(bytes),
            &mut |reaction| capture.absorb(reaction, scratch),
        );
        capture.only_frame("announce")
    }

    fn issue_link_request(&mut self) -> Vec<u8> {
        let issued = IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::EstablishLink(EstablishLink {
                destination: self.destination,
            }),
        };
        let now = self.tick();
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            scratch,
            ..
        } = self;
        let mut capture = FeedCapture::default();
        initiator.ingest_command_into(
            issued,
            AttachedInterfaces::new(interfaces),
            now,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| capture.absorb(reaction, scratch),
        );
        capture.only_frame("link request")
    }

    fn feed_initiator(&mut self, mut frame: Vec<u8>) -> FeedCapture {
        let now = self.tick();
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            scratch,
            ..
        } = self;
        let mut capture = FeedCapture::default();
        initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: now,
                source_interface: WIRE,
                bytes: &mut frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(interfaces),
                now,
                fill_entropy: &mut |bytes| initiator_entropy.fill(bytes),
                should_prove: &mut |_| true,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| capture.absorb(reaction, scratch),
            },
        );
        capture
    }

    fn feed_responder(&mut self, mut frame: Vec<u8>) -> FeedCapture {
        let now = self.tick();
        let Self {
            responder,
            responder_entropy,
            interfaces,
            scratch,
            ..
        } = self;
        let mut capture = FeedCapture::default();
        responder.ingest_packet_into(
            InboundPacket {
                arrived_at: now,
                source_interface: WIRE,
                bytes: &mut frame,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(interfaces),
                now,
                fill_entropy: &mut |bytes| responder_entropy.fill(bytes),
                should_prove: &mut |_| true,
                should_accept_resource: &mut |_| false,
                sink: &mut |reaction| capture.absorb(reaction, scratch),
            },
        );
        capture
    }

    pub fn transfer_profile(&mut self) -> ResourceTransferProfile {
        let mut profile = ResourceTransferProfile::new(self.payload.len());
        let id = CommandId(self.next_id);
        self.next_id += 1;
        let len = self.payload.len();
        self.transfer_one_segment(id, 1, 1, len, len as u64, &mut profile);
        profile
    }

    pub fn transfer_profile_multi(&mut self, total_len: usize) -> ResourceTransferProfile {
        let mut profile = ResourceTransferProfile::new(total_len);
        let total_segments = total_len.div_ceil(MAX_EFFICIENT_SIZE).max(1) as u64;
        let mut remaining = total_len;
        for segment_index in 1..=total_segments {
            let this = remaining.min(MAX_EFFICIENT_SIZE);
            remaining -= this;
            let id = CommandId(self.next_id);
            self.next_id += 1;
            self.transfer_one_segment(
                id,
                segment_index,
                total_segments,
                this,
                total_len as u64,
                &mut profile,
            );
        }
        profile
    }

    fn transfer_one_segment(
        &mut self,
        id: CommandId,
        segment_index: u64,
        total_segments: u64,
        len: usize,
        total_data_bytes: u64,
        profile: &mut ResourceTransferProfile,
    ) {
        let begun = Instant::now();
        let offer =
            self.send_resource_offer(id, segment_index, total_segments, len, total_data_bytes);
        profile.sender_offer += begun.elapsed();
        profile.advertisements += 1;
        profile.wire_bytes += offer.len() as u64;

        let begun = Instant::now();
        let accept = self.feed_responder(offer);
        profile.receiver_accept += begun.elapsed();
        let mut requests = accept.frames;
        assert_eq!(requests.len(), 1, "advertisement earns the first pull");

        let mut proof = None;
        while proof.is_none() {
            assert!(!requests.is_empty(), "receiver keeps the resource moving");
            let mut next_requests = Vec::new();
            for request in requests.drain(..) {
                profile.requests += 1;
                profile.wire_bytes += request.len() as u64;

                let begun = Instant::now();
                let served = self.feed_initiator(request);
                profile.sender_serve += begun.elapsed();

                for frame in served.frames {
                    profile.wire_bytes += frame.len() as u64;
                    match frame_context(&frame) {
                        Some(WireContext::Resource) => profile.parts += 1,
                        Some(WireContext::ResourceHashUpdate) => profile.hashmap_updates += 1,
                        _ => {}
                    }

                    let begun = Instant::now();
                    let received = self.feed_responder(frame);
                    profile.receiver_receive += begun.elapsed();
                    for response in received.frames {
                        profile.wire_bytes += response.len() as u64;
                        match frame_context(&response) {
                            Some(WireContext::ResourceRequest) => {
                                next_requests.push(response);
                            }
                            Some(WireContext::ResourceProof) => {
                                profile.proofs += 1;
                                proof = Some(response);
                            }
                            _ => {}
                        }
                    }
                }
            }
            requests = next_requests;
        }

        let begun = Instant::now();
        let settled = self.feed_initiator(proof.expect("proof"));
        profile.initiator_settle += begun.elapsed();
        assert!(
            settled.settlements.iter().any(|(settled_id, settlement)| {
                *settled_id == id && matches!(settlement, Settlement::SendResource(Ok(())))
            }),
            "proof settles the resource segment send",
        );
    }

    fn send_resource_offer(
        &mut self,
        id: CommandId,
        segment_index: u64,
        total_segments: u64,
        len: usize,
        total_data_bytes: u64,
    ) -> Vec<u8> {
        let now = self.tick();
        let Self {
            initiator,
            initiator_entropy,
            scratch,
            payload,
            link_id,
            ..
        } = self;
        let mut capture = FeedCapture::default();
        initiator.ingest_send_resource_segment_into(
            &personal_rns::routing::links::resources::ResourceSend {
                id,
                link_id: *link_id,
                body: personal_rns::routing::links::resources::ResourceBody {
                    data: &payload[..len],
                    compressed_candidate: None,
                    metadata: personal_rns::routing::links::resources::ResourceMetadata::None,
                },
                correlation:
                    personal_rns::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            personal_rns::routing::links::resources::ResourceSegment {
                index: segment_index,
                total_segments,
                total_data_bytes,
            },
            now,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| capture.absorb(reaction, scratch),
        );
        capture.only_frame("resource advertisement")
    }
}

impl FeedCapture {
    fn only_frame(mut self, label: &str) -> Vec<u8> {
        assert_eq!(self.frames.len(), 1, "{label} emits exactly one frame");
        self.frames.remove(0)
    }
}
fn deterministic_payload(len: usize) -> Vec<u8> {
    let mut state = 0xD00D_F00D_CAFE_BABEu64;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for byte in state.to_le_bytes() {
            if out.len() < len {
                out.push(byte);
            }
        }
    }
    out
}
