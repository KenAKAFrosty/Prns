use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use personal_rns::engine::{RatchetPolicy, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN};
use personal_rns::routes;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, Manual, PreConfiguredDestination, Prns, PrnsEvent,
    PrnsRecipe, TokioPrnsHandle,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, OnExisting, Role, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap as NodeStorage;
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc;

fn parse_hex(raw: &str) -> Vec<u8> {
    let clean: String = raw.chars().filter(char::is_ascii_hexdigit).collect();
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).expect("hex byte"))
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let usage =
        "usage: ble_firehose <bus-port> <dest-hex> [phase-secs] [payload-bytes] [windows-csv]";
    let port: u16 = args.next().expect(usage).parse().expect("bus-port");
    let target = parse_hex(&args.next().expect(usage));
    let phase = Duration::from_secs(args.next().and_then(|s| s.parse().ok()).unwrap_or(12));
    let payload_len: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN)
        .min(MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN);
    let windows: Vec<usize> = args
        .next()
        .map(|csv| {
            csv.split(',')
                .filter_map(|w| w.trim().parse().ok())
                .collect()
        })
        .unwrap_or_else(|| vec![8, 16, 32, 64, 128]);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(6)
        .enable_all()
        .build()
        .expect("firehose runtime")
        .block_on(run(port, target, phase, payload_len, windows));
}

async fn run(port: u16, target: Vec<u8>, phase: Duration, payload_len: usize, windows: Vec<usize>) {
    let single = PreConfiguredDestination::Single {
        app_name: "firehose",
        aspects: &["probe"],
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single],
        app_state: (),
        storage: NodeStorage::default(),
        routes: routes![],
        on_event: move |event, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: Manual,
    });
    let commands = node.handle();
    let driver = async {
        let role = join_shared_instance(
            &commands,
            SharedInstanceIntent {
                identity_dir: std::env::temp_dir(),
                ports: InstancePorts {
                    bus: port,
                    control: port + 1,
                },
                on_existing: OnExisting::JoinAsClient,
            },
        )
        .await
        .expect("join the shared-instance bus");
        assert!(
            matches!(role, Role::JoinedAsClient { .. }),
            "expected to join a running host as a client, got {role:?}"
        );
        println!(
            "FIREHOSE_READY bus={port} payload={payload_len} phase_s={} windows={windows:?}",
            phase.as_secs(),
        );

        let destination = loop {
            let heard = heard_rx
                .recv()
                .await
                .expect("hears an announce over the bus");
            if heard.as_bytes() == target.as_slice() {
                break heard;
            }
        };
        println!(
            "FIREHOSE_HEARD_TARGET destination={:02x?}",
            destination.as_bytes()
        );

        for window in windows {
            let result = pump(&commands, destination, window, payload_len, phase).await;
            let (delivered, failed, rtt_us_sum, elapsed) = result;
            let elapsed_ms = elapsed.as_millis().max(1) as u64;
            let pkts_per_sec = delivered * 1000 / elapsed_ms;
            let goodput_bps = delivered * payload_len as u64 * 1000 / elapsed_ms;
            let avg_rtt_ms = if delivered > 0 {
                rtt_us_sum as f64 / delivered as f64 / 1000.0
            } else {
                0.0
            };
            println!(
                "FIREHOSE_RESULT window={window} delivered={delivered} failed={failed} \
                 elapsed_ms={elapsed_ms} pkts_per_sec={pkts_per_sec} \
                 goodput_kBps={:.1} goodput_kbps={:.1} avg_rtt_ms={avg_rtt_ms:.1} payload={payload_len}",
                goodput_bps as f64 / 1000.0,
                goodput_bps as f64 * 8.0 / 1000.0,
            );
        }
        println!("FIREHOSE_DONE");
    };
    tokio::select! {
        () = node.run() => unreachable!("the firehose's run loop returned"),
        () = driver => {}
    }
}

async fn pump(
    commands: &TokioPrnsHandle,
    destination: DestinationHash,
    window: usize,
    payload_len: usize,
    phase: Duration,
) -> (u64, u64, u64, Duration) {
    let delivered = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let rtt_us_sum = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let payload = vec![0xABu8; payload_len];

    let started = Instant::now();
    let deadline = started + phase;
    let mut workers = Vec::with_capacity(window);
    for _ in 0..window {
        let commands = commands.clone();
        let payload = payload.clone();
        let delivered = delivered.clone();
        let failed = failed.clone();
        let rtt_us_sum = rtt_us_sum.clone();
        let stop = stop.clone();
        workers.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let at = Instant::now();
                match commands.send_single_packet(destination, &payload).await {
                    Ok(_) => {
                        delivered.fetch_add(1, Ordering::Relaxed);
                        rtt_us_sum.fetch_add(at.elapsed().as_micros() as u64, Ordering::Relaxed);
                    }
                    Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for worker in workers {
        let _ = worker.await;
    }
    stop.store(true, Ordering::Relaxed);
    (
        delivered.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        rtt_us_sum.load(Ordering::Relaxed),
        started.elapsed(),
    )
}
