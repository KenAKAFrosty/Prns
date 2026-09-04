use super::inline_work::{feed_packet_inline, issue_command_inline};
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
    capture: FeedCapture,
    scratch: Vec<u8>,
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
        let interfaces = vec![tcp::descriptor(
            WIRE,
            tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
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
            capture: FeedCapture::default(),
            scratch: Vec::new(),
        };

        let issued = IssuedCommand {
            id: CommandId(0),
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        let Self {
            initiator,
            responder,
            initiator_entropy,
            responder_entropy,
            interfaces,
            capture,
            scratch,
            ..
        } = &mut cycle;
        issue_command_inline(
            responder,
            issued,
            AttachedInterfaces::new(interfaces),
            NOW,
            responder_entropy,
            capture,
            scratch,
        );
        let mut announce = capture.only_frame("announce");
        assert!(!announce.is_empty(), "responder emitted its announce");

        capture.reset();
        feed_packet_inline(
            initiator,
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: &mut announce,
            },
            AttachedInterfaces::new(interfaces),
            NOW,
            initiator_entropy,
            capture,
            scratch,
        );
        assert!(capture.announce_heard, "initiator learned the destination");
        cycle
    }

    pub fn seal(&mut self) {
        let issued = IssuedCommand {
            id: CommandId(self.next_id),
            command: PrnsCommand::SendSinglePacket(SendSinglePacket {
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
            capture,
            scratch,
            ..
        } = self;
        sealed.clear();
        capture.reset();
        issue_command_inline(
            initiator,
            issued,
            AttachedInterfaces::new(interfaces),
            NOW,
            initiator_entropy,
            capture,
            scratch,
        );
        capture.take_only_frame_into("single", sealed);
        assert!(!self.sealed.is_empty(), "send sealed a frame");
    }

    pub fn deliver_prove(&mut self) {
        let Self {
            responder,
            responder_entropy,
            interfaces,
            sealed,
            proof,
            capture,
            scratch,
            ..
        } = self;
        proof.clear();
        capture.reset();
        feed_packet_inline(
            responder,
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: sealed,
            },
            AttachedInterfaces::new(interfaces),
            NOW,
            responder_entropy,
            capture,
            scratch,
        );
        let delivered = capture.delivered_single;
        capture.take_only_frame_into("proof", proof);
        assert!(delivered, "responder delivered the single");
        assert!(!self.proof.is_empty(), "responder proved the single");
    }

    pub fn settle(&mut self) {
        let mut proof = core::mem::take(&mut self.proof);
        self.settle_frame(&mut proof);
        self.proof = proof;
    }

    pub fn settle_frame(&mut self, proof: &mut [u8]) {
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            capture,
            scratch,
            ..
        } = self;
        capture.reset();
        feed_packet_inline(
            initiator,
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: proof,
            },
            AttachedInterfaces::new(interfaces),
            NOW,
            initiator_entropy,
            capture,
            scratch,
        );
        let settled = capture
            .settlements
            .iter()
            .any(|(_, settlement)| matches!(settlement, Settlement::SendSinglePacket(Ok(_))));
        assert!(settled, "proof verified and the receipt settled");
    }
}

#[cfg(test)]
mod tests {
    use super::Cycle;

    #[test]
    fn construction_learns_the_destination_from_the_announce_directive() {
        let _ = Cycle::new();
    }

    #[test]
    fn roundtrip_drives_each_typed_continuation_to_settlement() {
        let mut cycle = Cycle::new();
        cycle.seal();
        cycle.deliver_prove();
        cycle.settle();
    }
}
