//! Spike for the async-reactor pivot: the whole shell shape in miniature, driving
//! today's `EngineState` unchanged. A single `select!` races the three inputs — a
//! command, an inbound packet, the next scheduled deadline — calls the one sync method
//! the winner names, and turns whatever comes back into I/O. Between inputs it is truly
//! dormant (no timer armed, the task parked). Throwaway: it proves the loop and the
//! dormancy before we re-cut the engine surface to the `EngineReaction` sink.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::engine::test_support::{hx, Cap, RAW_ANNOUNCE, TEST_TRANSPORT_ID};
use crate::engine::{
    AnnounceIngest, EngineState, IngestPacketOutcome, InstantMillis, IssuedCommand,
    NextScheduledEngineWork,
};
use crate::interfaces::{
    ConnectionState, EgressCapability, InboundPacket, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind, TransportCapability,
};
use crate::routing::announce::defaults::JitterSeed;
use crate::wire::{PacketType, WirePacketHeader, MTU};

const JITTER: JitterSeed = JitterSeed(0x5151_5151_5151_5151);

/// The reactor loop: park until one of the three inputs fires, run the sync method it
/// names, push whatever it owes outward. No batching — one packet, one command, one
/// due deadline per turn — and no work at all between them.
async fn drive(
    mut engine: EngineState<Cap>,
    view: [InterfaceDescriptor; 2],
    base: Instant,
    mut inbound: UnboundedReceiver<(InterfaceId, std::vec::Vec<u8>)>,
    mut commands: UnboundedReceiver<IssuedCommand>,
    sent: UnboundedSender<(InterfaceId, std::vec::Vec<u8>)>,
    journal: UnboundedSender<std::string::String>,
) {
    loop {
        let now = InstantMillis(base.elapsed().as_millis() as u64);
        let wake = engine.next_wakeup(now);
        tokio::select! {
            arrived = inbound.recv() => {
                let Some((id, mut bytes)) = arrived else { return };
                let packet = InboundPacket {
                    arrived_at: now,
                    source_interface: id,
                    bytes: &mut bytes,
                };
                if let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) =
                    engine.ingest_packet(packet, JITTER, &view)
                {
                    let _ = journal.send(std::format!("AnnounceHeard hops={}", accepted.hops));
                }
            }
            issued = commands.recv() => {
                let Some(issued) = issued else { return };
                let _ = engine.ingest_command(issued, &view);
                let _ = journal.send(std::string::String::from("CommandIngested"));
            }
            () = wait_for_deadline(base, wake) => {
                let tick_output = engine.tick(now, JITTER, &view);
                for directive in tick_output.egress_directives(&view) {
                    let mut buf = [0u8; MTU];
                    if let Ok(written) = directive.to_wire(&mut buf) {
                        let _ = sent.send((directive.target(), buf[..written].to_vec()));
                    }
                }
                tick_output.commit();
            }
        }
    }
}

/// The timer racer. `Idle` is the dormancy: a future that never resolves, so the
/// `select!` rests on the two input channels alone and the task parks.
async fn wait_for_deadline(base: Instant, wake: NextScheduledEngineWork) {
    match wake {
        NextScheduledEngineWork::Idle => std::future::pending::<()>().await,
        NextScheduledEngineWork::Immediate => {}
        NextScheduledEngineWork::At(at) => {
            let target = base + Duration::from_millis(at.0);
            tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
        }
    }
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

#[tokio::test]
async fn a_packet_wakes_the_reactor_to_rebroadcast_then_it_falls_dormant() {
    let source = InterfaceId::new([0xA1; 16]);
    let peer = InterfaceId::new([0xB2; 16]);
    let view = [descriptor(source), descriptor(peer)];

    let mut engine = EngineState::<Cap>::default();
    engine.set_transport_id(TEST_TRANSPORT_ID);

    let base = Instant::now();
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
    let (sent_tx, mut sent_rx) = mpsc::unbounded_channel();
    let (journal_tx, mut journal_rx) = mpsc::unbounded_channel();

    tokio::spawn(drive(
        engine, view, base, inbound_rx, command_rx, sent_tx, journal_tx,
    ));

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        journal_rx.try_recv().is_err(),
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

    let note = tokio::time::timeout(Duration::from_secs(2), journal_rx.recv())
        .await
        .expect("the packet edge journals within the window")
        .expect("the reactor task is alive");
    assert!(note.starts_with("AnnounceHeard"), "journaled: {note}");

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
