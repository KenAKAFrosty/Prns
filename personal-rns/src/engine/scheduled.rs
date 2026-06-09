use crate::engine::{
    Directive, DueSelfAnnounceWriteOutcome, EngineReaction, EngineState, InstantMillis, Journaled,
    RatchetEntropy, RequestPathFailure, SendSingleFailure, Settlement,
};
use crate::interfaces::{ConnectionState, InterfaceDescriptor};
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

impl<S: EngineStorage> EngineState<S> {
    /// Fire this node's due self-announce, if one is due: write it once and fan it to every
    /// interface that may transmit. Entropy is pulled only when an announce is actually due,
    /// so an idle timer wake costs nothing.
    pub fn fire_due_self_announces(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceDescriptor],
        fill_entropy: &mut impl FnMut(&mut [u8]),
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        if self.self_announces.due_announce(now).is_none() {
            return;
        }
        let mut self_announce_bytes = [0u8; SelfAnnounceEntropy::LEN];
        fill_entropy(&mut self_announce_bytes);
        let self_announce = SelfAnnounceEntropy::new(self_announce_bytes);
        let mut ratchet_bytes = [0u8; RatchetEntropy::LEN];
        fill_entropy(&mut ratchet_bytes);
        let ratchet = RatchetEntropy::new(ratchet_bytes);

        let mut buf = [0u8; MTU];
        if let DueSelfAnnounceWriteOutcome::Written { len, .. } =
            self.write_due_self_announce(now, self_announce, ratchet, &mut buf)
        {
            for descriptor in view {
                if matches!(
                    descriptor.state,
                    ConnectionState::Connected | ConnectionState::Degraded
                ) && descriptor.capabilities.allows_transmit()
                {
                    sink(EngineReaction::Directive(Directive::Send {
                        target: descriptor.id,
                        bytes: &buf[..len],
                    }));
                }
            }
        }
    }

    /// Settle every send-single whose proof deadline has passed: each gives up and closes
    /// `SendSingle(Timeout)`.
    pub fn settle_timed_out_send_singles(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        while let Some(expired) = self.pop_timed_out_send_single(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
            }));
        }
    }

    /// Settle every path request whose answer never arrived in time: each closes
    /// `RequestPath(Timeout)`.
    pub fn settle_timed_out_path_requests(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        while let Some(expired) = self.pop_timed_out_path_request(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::self_announce::AnnounceConfig;
    use crate::engine::test_support::Cap;
    use crate::engine::{
        CommandId, PathRequestId, PathRequestWriteOutcome, RatchetPolicy, ReannounceSchedule,
        RequestPath, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::identity::Zeroizing;
    use crate::interfaces::{
        EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceId, InterfaceMode,
        MediumKind, TransportCapability,
    };
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::{DestinationHash, PacketType, WirePacketHeader};

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            medium: MediumKind::Loopback,
            state: ConnectionState::Connected,
            announce_rate_limit: None,
        }
    }

    #[test]
    fn fire_due_self_announces_fans_the_announce_to_every_transmitting_interface() {
        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let mut engine = EngineState::<Cap>::new(Zeroizing::new(secret));
        let node = engine.held_identity_hashes()[0];
        let destination = engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        engine
            .schedule_announce(
                &destination,
                AnnounceConfig {
                    app_data: b"hello",
                    schedule: ReannounceSchedule::default(),
                },
            )
            .unwrap();

        let first = iface(0xA1);
        let second = iface(0xB2);
        let view = std::vec![descriptor(first), descriptor(second)];

        let mut sent: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)> = std::vec::Vec::new();
        engine.fire_due_self_announces(
            InstantMillis(10_000_000),
            &view,
            &mut |bytes| bytes.fill(0xCA),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                    sent.push((target, bytes.to_vec()));
                }
            },
        );

        assert_eq!(sent.len(), 2, "the self-announce fans to both interfaces");
        for (target, bytes) in &sent {
            let (header, _) = WirePacketHeader::parse(bytes).unwrap();
            assert_eq!(header.packet_type, PacketType::Announce);
            assert_eq!(header.destination, destination);
            assert!(*target == first || *target == second);
        }
    }

    #[test]
    fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
        let mut engine = EngineState::<Cap>::default();
        let issued_at = InstantMillis(1_000);
        let mut buf = [0u8; MTU];
        let outcome = engine.write_commanded_path_request(
            CommandId(9),
            &RequestPath {
                destination: DestinationHash::new([0x44; 16]),
                id: PathRequestId::new([0x55; 16]),
            },
            issued_at,
            &mut buf,
        );
        assert!(matches!(outcome, PathRequestWriteOutcome::Written { .. }));

        let mut settled: std::vec::Vec<(CommandId, Settlement)> = std::vec::Vec::new();

        engine.settle_timed_out_path_requests(issued_at, &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        });
        assert!(settled.is_empty(), "before the deadline, nothing settles");

        engine.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            )],
            "past the deadline the request settles Timeout, exactly once",
        );
    }
}
