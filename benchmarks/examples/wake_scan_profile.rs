use personal_rns::interfaces::AttachedInterfaces;
use std::time::Instant;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineReaction,
    EngineState, IngestIo, InstantMillis, IssuedCommand, Journaled, PrnsCommand, RatchetPolicy,
    SendSinglePacket, SendSinglePacketPayload, WakeSchedule,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{tcp, InboundPacket, InterfaceDescriptor, InterfaceId};
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;

const WIRE: InterfaceId = InterfaceId::new([0xC7; 8]);

struct Splitmix(u64);

impl Splitmix {
    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = self.0;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
    }
}

fn interfaces() -> Vec<InterfaceDescriptor> {
    vec![tcp::descriptor(
        WIRE,
        tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE),
    )]
}

fn announce_wire() -> (Vec<u8>, DestinationHash) {
    let mut responder =
        EngineState::<GrowableHeap>::new(Zeroizing::new([0x91; IDENTITY_SECRET_KEY_LEN]));
    let identity = responder.held_identity_hashes()[0];
    let destination = responder
        .register_single_destination(
            &identity,
            "bench",
            &["wakescan"],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the destination");
    let mut entropy = Splitmix(202);
    let mut wire = Vec::new();
    responder.ingest_command_into(
        IssuedCommand {
            id: CommandId(0),
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        },
        AttachedInterfaces::new(&interfaces()),
        InstantMillis(1_000),
        &mut |bytes| entropy.fill(bytes),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                wire.extend_from_slice(bytes);
            }
        },
    );
    assert!(!wire.is_empty(), "responder emitted its announce");
    (wire, destination)
}

fn initiator_with_route(announce: &[u8]) -> EngineState<GrowableHeap> {
    let mut initiator =
        EngineState::<GrowableHeap>::new(Zeroizing::new([0x92; IDENTITY_SECRET_KEY_LEN]));
    let mut entropy = Splitmix(101);
    let mut frame = announce.to_vec();
    let mut heard = false;
    initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: WIRE,
            bytes: &mut frame,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&interfaces()),
            now: InstantMillis(1_000),
            fill_random: &mut |bytes| entropy.fill(bytes),
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
    assert!(heard, "initiator learned the route");
    initiator
}

fn fill_outstanding(
    initiator: &mut EngineState<GrowableHeap>,
    destination: DestinationHash,
    n: usize,
) {
    let mut entropy = Splitmix(303);
    let payload = [0xAB_u8; 32];
    for i in 0..n {
        initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(i as u64 + 1),
                command: PrnsCommand::SendSinglePacket(SendSinglePacket {
                    destination,
                    payload: SendSinglePacketPayload::from_slice(&payload).expect("payload fits"),
                }),
            },
            AttachedInterfaces::new(&interfaces()),
            InstantMillis(1_000),
            &mut |bytes| entropy.fill(bytes),
            &mut |_| {},
        );
    }
}

fn schedule_deadline(schedule: WakeSchedule) -> u64 {
    match schedule {
        WakeSchedule::At(at) | WakeSchedule::AtMost(at) => at.0,
        _ => 0,
    }
}

fn main() {
    let (announce, destination) = announce_wire();

    println!("receipt_timeouts_wake() vs outstanding-receipt count (engine-direct, no I/O)");
    println!(
        "{:<10} {:<14} {:<16}",
        "receipts", "ns/scan", "throughput note"
    );
    let iterations = 2_000_000u64;
    for &n in &[1usize, 16, 64, 128, 256, 512, 1024] {
        let mut initiator = initiator_with_route(&announce);
        fill_outstanding(&mut initiator, destination, n);

        for _ in 0..10_000 {
            std::hint::black_box(schedule_deadline(initiator.receipt_timeouts_wake()));
        }
        let begun = Instant::now();
        let mut sink = 0u64;
        for _ in 0..iterations {
            sink = sink.wrapping_add(schedule_deadline(std::hint::black_box(
                initiator.receipt_timeouts_wake(),
            )));
        }
        let elapsed = begun.elapsed();
        std::hint::black_box(sink);
        let ns = elapsed.as_secs_f64() * 1e9 / iterations as f64;
        let at_send_rate = ns * 20_000.0 / 1e9 * 100.0;
        println!(
            "{:<10} {:<14.1} {:<16}",
            n,
            ns,
            format!("{at_send_rate:.2}% @20k/s")
        );
    }
}
