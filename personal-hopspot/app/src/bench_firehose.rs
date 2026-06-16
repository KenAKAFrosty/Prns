//! On-device self-firehose: two engines on the real S3 silicon, wired by hand-carrying
//! each `Directive::Send` frame from one into the other's `ingest_packet_into` — no
//! interface, no link, no reactor. The initiator pumps `SendSingle`s at the responder's
//! bench destination; the responder proves every one; the initiator counts the settlements.
//! It measures pure engine + PSRAM-storage throughput on-chip and prints `DEVICE_FIREHOSE`
//! over the serial-JTAG log. Built only under the `device-firehose` feature, replacing the
//! normal Hopspot firmware.

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

esp_app_desc!();

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
    SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::usb_auto::core::device_descriptor;
use personal_rns::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::routing::announce::defaults::JitterSeed;
use personal_rns::routing::ProofStrategy;

use crate::engine_storage::EngineStorageType;

const BENCH_IFACE: InterfaceId = InterfaceId::new(*b"prns-bench-fireh");
const DURATION_MS: u64 = 5_000;
const PAYLOAD_LEN: usize = 128;
const YIELD_EVERY: u64 = 128;

type FrameBuf = heapless::Vec<u8, EMBEDDED_MAX_WIRE_FRAME_LEN>;
type FrameList = heapless::Vec<FrameBuf, 4>;

fn secret(seed: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([seed; IDENTITY_SECRET_KEY_LEN])
}

fn now_ms() -> InstantMillis {
    InstantMillis(Instant::now().as_millis())
}

fn push_frame(frames: &mut FrameList, bytes: &[u8]) {
    let mut buf = FrameBuf::new();
    if buf.extend_from_slice(bytes).is_ok() {
        let _ = frames.push(buf);
    }
}

/// Fold one engine reaction: outbound frames are captured for hand-off to the peer engine,
/// a settled `SendSingle(Ok)` flips `delivered`.
fn capture(reaction: EngineReaction<'_>, frames: &mut FrameList, delivered: &mut bool) {
    match reaction {
        EngineReaction::Directive(Directive::Send { bytes, .. }) => push_frame(frames, bytes),
        EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) => {
            push_frame(frames, bytes)
        }
        EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
            let mut buf = FrameBuf::new();
            let _ = buf.resize(EMBEDDED_MAX_WIRE_FRAME_LEN, 0u8);
            if let Some(len) = fill(buf.as_mut_slice()) {
                buf.truncate(len);
                let _ = frames.push(buf);
            }
        }
        EngineReaction::Journaled(Journaled::CommandSettled {
            settlement: Settlement::SendSingle(Ok(_)),
            ..
        }) => {
            *delivered = true;
        }
        _ => {}
    }
}

/// Feed every frame into `engine` as an inbound packet on the bench interface, returning what
/// the engine emits back (proofs, settlements). `prove` decides the proof strategy at ingest.
fn deliver_into(
    engine: &mut EngineState<EngineStorageType>,
    view: &[InterfaceConfig],
    frames: &mut FrameList,
    jitter: u64,
    prove: bool,
    out: &mut FrameList,
    delivered: &mut bool,
    entropy: &mut impl FnMut(&mut [u8]),
) {
    for frame in frames.iter_mut() {
        let now = now_ms();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: now,
                source_interface: BENCH_IFACE,
                bytes: frame.as_mut_slice(),
            },
            JitterSeed(jitter),
            view,
            now,
            entropy,
            &mut |_request| prove,
            &mut |reaction| capture(reaction, out, delivered),
        );
    }
}

pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    spawner.spawn(bench_task().expect("bench task fits the pool"));
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

/// The bench runs in its own task so the two `EngineState`s live in the task's static pool
/// (each ~28 KB SRAM at N=64), not on the executor stack.
#[embassy_executor::task]
async fn bench_task() {
    Timer::after(Duration::from_millis(2000)).await;
    println!("DEVICE_FIREHOSE boot — building two engines on PSRAM storage");

    let mut entropy = |bytes: &mut [u8]| {
        Rng::new().read(bytes);
    };
    let view = [device_descriptor(BENCH_IFACE)];

    let mut initiator = EngineState::<EngineStorageType>::new(secret(0xA1));
    let mut responder = EngineState::<EngineStorageType>::new(secret(0xB2));
    let responder_node = responder.held_identity_hashes()[0];
    let destination = responder
        .register_single_destination(
            &responder_node,
            "bench",
            &["firehose"],
            b"",
            ProofStrategy::ProveAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the bench destination");

    // Bootstrap: the responder announces (carrying its keys + a 0-hop route); the initiator
    // ingests it so a SendSingle resolves and encrypts directly.
    {
        let now = now_ms();
        let mut announce = FrameList::new();
        let mut ignore = false;
        responder.ingest_command_into(
            IssuedCommand {
                id: CommandId(0),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            },
            &view,
            now,
            &mut entropy,
            &mut |reaction| capture(reaction, &mut announce, &mut ignore),
        );
        let mut learned = FrameList::new();
        deliver_into(
            &mut initiator,
            &view,
            &mut announce,
            0,
            false,
            &mut learned,
            &mut ignore,
            &mut entropy,
        );
    }

    let payload = [0x5Au8; PAYLOAD_LEN];
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut command_id = 1u64;
    let mut cycle = 0u64;
    let start = Instant::now();

    while start.elapsed().as_millis() < DURATION_MS {
        // Initiator: seal one SINGLE at the bench destination.
        let now = now_ms();
        let mut single = FrameList::new();
        let mut ignore = false;
        initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(command_id),
                command: EngineCommand::SendSingle(SendSingle {
                    destination,
                    payload: SendSinglePayload::from_slice(&payload).expect("payload fits"),
                }),
            },
            &view,
            now,
            &mut entropy,
            &mut |reaction| capture(reaction, &mut single, &mut ignore),
        );
        sent += 1;
        command_id += 1;

        // Responder: ingest the SINGLE and prove it.
        let mut proof = FrameList::new();
        let mut ignore2 = false;
        deliver_into(
            &mut responder,
            &view,
            &mut single,
            cycle,
            true,
            &mut proof,
            &mut ignore2,
            &mut entropy,
        );

        // Initiator: ingest the proof and settle the receipt.
        let mut nothing = FrameList::new();
        let mut settled = false;
        deliver_into(
            &mut initiator,
            &view,
            &mut proof,
            cycle,
            false,
            &mut nothing,
            &mut settled,
            &mut entropy,
        );
        if settled {
            delivered += 1;
        }

        cycle += 1;
        if cycle % YIELD_EVERY == 0 {
            embassy_futures::yield_now().await;
        }
    }

    let elapsed_ms = start.elapsed().as_millis().max(1);
    let per_sec = delivered.saturating_mul(1000) / elapsed_ms;
    println!(
        "DEVICE_FIREHOSE sent={sent} delivered={delivered} elapsed_ms={elapsed_ms} \
         msg_per_sec={per_sec} payload_len={PAYLOAD_LEN}"
    );

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
