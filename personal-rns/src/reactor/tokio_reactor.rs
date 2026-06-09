use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use super::driver::{advance, draw_jitter, fire_due_lane, wait_for_due_lane};
use super::Host;
use crate::engine::{EngineReaction, EngineState, InstantMillis, IssuedCommand};
use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::routing::storage::EngineStorage;

/// A [`Host`] backed by tokio's clock and the OS CSPRNG.
pub struct TokioHost {
    base: Instant,
}

impl TokioHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for TokioHost {
    fn now(&self) -> InstantMillis {
        InstantMillis(self.base.elapsed().as_millis() as u64)
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        tokio::time::sleep_until(self.base + Duration::from_millis(deadline.0)).await;
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide reactor entropy");
    }
}

/// Run the reactor loop until the input channels close. Each turn parks on the three
/// inputs, runs the one sync engine method the winner names, and pushes whatever it owes
/// as `EngineReaction`s to `on_reaction`. Between inputs it is dormant: `Idle` arms no
/// timer, so the select rests on the two channels alone and the task truly parks.
pub async fn run<S, H>(
    mut engine: EngineState<S>,
    view: std::vec::Vec<InterfaceDescriptor>,
    mut host: H,
    mut inbound: UnboundedReceiver<(InterfaceId, std::vec::Vec<u8>)>,
    mut commands: UnboundedReceiver<IssuedCommand>,
    mut on_reaction: impl FnMut(EngineReaction<'_>),
) where
    S: EngineStorage,
    H: Host,
{
    let mut outlook = engine.wake_outlook();
    loop {
        let wake = outlook.soonest(host.now());
        tokio::select! {
            arrived = inbound.recv() => {
                let Some((id, mut bytes)) = arrived else { return };
                let now = host.now();
                let jitter = draw_jitter(&mut host);
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: id,
                    bytes: &mut bytes,
                };
                let delta = engine.ingest_packet_into(
                    packet,
                    jitter,
                    &view,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut on_reaction,
                );
                advance(&mut outlook, delta, &engine);
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    &view,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut on_reaction,
                );
                advance(&mut outlook, delta, &engine);
            }
            lane = wait_for_due_lane(&host, wake) => {
                let now = host.now();
                let delta = fire_due_lane(&mut engine, lane, now, &view, &mut host, &mut on_reaction);
                advance(&mut outlook, delta, &engine);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{hx, Cap, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
    use crate::engine::{Directive, Journaled};
    use crate::interfaces::{
        ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceMode,
        MediumKind, TransportCapability,
    };
    use crate::wire::{PacketType, WirePacketHeader};
    use tokio::sync::mpsc;

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

    #[tokio::test]
    async fn a_packet_wakes_the_reactor_to_rebroadcast_then_it_falls_dormant() {
        let source = InterfaceId::new([0xA1; 16]);
        let peer = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(source), descriptor(peer)];

        let mut engine = EngineState::<Cap>::default();
        engine.set_transport_id(TEST_TRANSPORT_ID);

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();
        let (sent_tx, mut sent_rx) = mpsc::unbounded_channel::<(InterfaceId, std::vec::Vec<u8>)>();

        let on_reaction = move |reaction: EngineReaction<'_>| match reaction {
            EngineReaction::Journaled(Journaled::AnnounceHeard { .. }) => {
                let _ = heard_tx.send(());
            }
            EngineReaction::Journaled(
                Journaled::Delivered(_) | Journaled::CommandSettled { .. },
            ) => {}
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                let _ = sent_tx.send((target, bytes.to_vec()));
            }
        };

        tokio::spawn(run(
            engine,
            view,
            TokioHost::new(),
            inbound_rx,
            command_rx,
            on_reaction,
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            heard_rx.try_recv().is_err(),
            "an idle reactor journals nothing"
        );
        assert!(sent_rx.try_recv().is_err(), "an idle reactor emits nothing");

        let raw = hx(RAW_ANNOUNCE);
        let original_hops = WirePacketHeader::parse(&raw)
            .expect("valid announce wire")
            .0
            .hops;
        inbound_tx
            .send((source, raw))
            .expect("the reactor task holds the receiver");

        tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
            .await
            .expect("the packet edge journals within the window")
            .expect("the reactor task is alive");

        let (target, bytes) = tokio::time::timeout(Duration::from_secs(2), sent_rx.recv())
            .await
            .expect("the rebroadcast deadline fires within the jitter window")
            .expect("the reactor task is alive");
        assert_eq!(
            target, peer,
            "a rebroadcast fans to the peer, never back its source"
        );
        let (header, _) = WirePacketHeader::parse(&bytes).expect("valid rebroadcast wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.hops,
            original_hops + 1,
            "the rebroadcast bumps the hop count"
        );
    }

    #[tokio::test]
    async fn a_delivery_answers_with_a_proof_directive_on_the_arrival_lane() {
        use crate::crypto::X25519SecretKey;
        use crate::engine::proof::IMPLICIT_PROOF_WIRE_LEN;
        use crate::engine::RatchetPolicy;
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
        use crate::routing::dedup::PacketHash;
        use crate::routing::upstream_app_destinations::ProofStrategy;
        use crate::wire::{
            ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, MTU,
        };

        let mut secret = [0u8; 64];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        let secret = Zeroizing::new(secret);

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        let mut engine = EngineState::<Cap>::new(secret);
        let destination = engine
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the single destination");

        let remote = RemoteIdentity::from_public_keys(
            identity.encryption_public_key(),
            identity.signing_public_key(),
        );
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination,
            context: WireContext::None,
        };
        let mut wire = [0u8; MTU];
        let header_len = header.write(&mut wire).expect("writes the header");
        let sealed = remote
            .encrypt(
                &X25519SecretKey::new([0x77; 32]),
                &[0x88; 16],
                b"prove-through-the-stack",
                &mut wire[header_len..],
            )
            .expect("seals the payload");
        let raw = wire[..header_len + sealed].to_vec();
        let packet_hash = PacketHash::of_wire_packet(&raw).expect("hashes the wire packet");

        let mut expected_proof = std::vec::Vec::new();
        expected_proof.push(0x03);
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(packet_hash.proof_destination().as_bytes());
        expected_proof.push(0x00);
        expected_proof.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
        assert_eq!(expected_proof.len(), IMPLICIT_PROOF_WIRE_LEN);

        let source = InterfaceId::new([0xA1; 16]);
        let view = std::vec![descriptor(source)];

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (delivered_tx, mut delivered_rx) = mpsc::unbounded_channel::<()>();
        let (sent_tx, mut sent_rx) = mpsc::unbounded_channel::<(InterfaceId, std::vec::Vec<u8>)>();

        let on_reaction = move |reaction: EngineReaction<'_>| match reaction {
            EngineReaction::Journaled(Journaled::Delivered(_)) => {
                let _ = delivered_tx.send(());
            }
            EngineReaction::Journaled(
                Journaled::AnnounceHeard { .. } | Journaled::CommandSettled { .. },
            ) => {}
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                let _ = sent_tx.send((target, bytes.to_vec()));
            }
        };

        tokio::spawn(run(
            engine,
            view,
            TokioHost::new(),
            inbound_rx,
            command_rx,
            on_reaction,
        ));

        inbound_tx
            .send((source, raw))
            .expect("the reactor task holds the receiver");

        tokio::time::timeout(Duration::from_secs(2), delivered_rx.recv())
            .await
            .expect("the delivery journals within the window")
            .expect("the reactor task is alive");

        let (target, bytes) = tokio::time::timeout(Duration::from_secs(2), sent_rx.recv())
            .await
            .expect("the owed proof is emitted within the window")
            .expect("the reactor task is alive");
        assert_eq!(
            target, source,
            "the proof returns on the lane the packet arrived through"
        );
        assert_eq!(
            bytes, expected_proof,
            "the proof is byte-identical to the RNS 1.3.1 implicit proof"
        );
    }

    #[tokio::test]
    async fn a_commanded_announce_fans_to_every_interface_and_settles() {
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, RatchetPolicy,
            Settlement,
        };
        use crate::identity::Zeroizing;
        use crate::routing::upstream_app_destinations::ProofStrategy;

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
            .expect("registers the single destination");

        let first = InterfaceId::new([0xA1; 16]);
        let second = InterfaceId::new([0xB2; 16]);
        let view = std::vec![descriptor(first), descriptor(second)];

        let (_inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
        let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (sent_tx, mut sent_rx) = mpsc::unbounded_channel::<(InterfaceId, std::vec::Vec<u8>)>();

        let on_reaction = move |reaction: EngineReaction<'_>| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                let _ = settled_tx.send((id, settlement));
            }
            EngineReaction::Journaled(
                Journaled::AnnounceHeard { .. } | Journaled::Delivered(_),
            ) => {}
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                let _ = sent_tx.send((target, bytes.to_vec()));
            }
        };

        tokio::spawn(run(
            engine,
            view,
            TokioHost::new(),
            inbound_rx,
            command_rx,
            on_reaction,
        ));

        command_tx
            .send(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Scheduled,
                }),
            })
            .expect("the reactor task holds the receiver");

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
                .await
                .expect("the command settles within the window")
                .expect("the reactor task is alive");
        assert_eq!(settled_id, CommandId(7));
        assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));

        let mut sent_targets = std::vec::Vec::new();
        for _ in 0..2 {
            let (target, bytes) = tokio::time::timeout(Duration::from_secs(2), sent_rx.recv())
                .await
                .expect("an announce fires on each interface")
                .expect("the reactor task is alive");
            let (header, _) = WirePacketHeader::parse(&bytes).expect("valid announce wire");
            assert_eq!(header.packet_type, PacketType::Announce);
            assert_eq!(header.destination, destination);
            sent_targets.push(target);
        }
        assert!(
            sent_targets.contains(&first) && sent_targets.contains(&second),
            "the announce fans to every interface"
        );
    }
}
