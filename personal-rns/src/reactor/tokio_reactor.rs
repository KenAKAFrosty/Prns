use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use super::Host;
use crate::engine::{
    AnnounceIngest, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis, IssuedCommand,
    Journaled, NextScheduledEngineWork,
};
use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::routing::announce::defaults::JitterSeed;
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
    loop {
        let mut jitter_bytes = [0u8; core::mem::size_of::<u64>()];
        host.fill_entropy(&mut jitter_bytes);
        let jitter = JitterSeed(u64::from_le_bytes(jitter_bytes));
        let wake = engine.next_wakeup(host.now());
        tokio::select! {
            arrived = inbound.recv() => {
                let Some((id, mut bytes)) = arrived else { return };
                let packet = InboundPacket {
                    arrived_at: host.now(),
                    source_interface: id,
                    bytes: &mut bytes,
                };
                if let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) =
                    engine.ingest_packet(packet, jitter, &view)
                {
                    on_reaction(EngineReaction::Journaled(Journaled::AnnounceHeard {
                        destination: accepted.destination,
                        hops: accepted.hops,
                        source_interface: id,
                    }));
                }
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                let _ = engine.ingest_command(issued, &view);
            }
            () = wait_for_deadline(&host, wake) => {
                engine.drain_scheduled(host.now(), jitter, &view, &mut |directive| {
                    on_reaction(EngineReaction::Directive(directive));
                });
            }
        }
    }
}

async fn wait_for_deadline<H: Host>(host: &H, wake: NextScheduledEngineWork) {
    match wake {
        NextScheduledEngineWork::Idle => std::future::pending::<()>().await,
        NextScheduledEngineWork::Immediate => {}
        NextScheduledEngineWork::At(at) => host.sleep_until(at).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{hx, Cap, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
    use crate::engine::Directive;
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
}
