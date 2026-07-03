use std::time::Instant;

use personal_rns::engine::RatchetPolicy;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, OnExisting, Role, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap as NodeStorage;
use personal_rns::wire::DestinationHash;
use personal_rns::{interfaces, routes};
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
    let usage = "usage: ble_resource <bus-port> <dest-hex> [kib-per-transfer] [iterations]";
    let port: u16 = args.next().expect(usage).parse().expect("bus-port");
    let target = parse_hex(&args.next().expect(usage));
    let kib: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(64);
    let iterations: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("resource runtime")
        .block_on(run(port, target, kib * 1024, iterations));
}

async fn run(port: u16, target: Vec<u8>, total_bytes: usize, iterations: usize) {
    let single = PreConfiguredDestination::Single {
        app_name: "resfire",
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
        interfaces: interfaces![],
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
            "RESOURCE_READY bus={port} kib={} iters={iterations}",
            total_bytes / 1024
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
            "RESOURCE_HEARD_TARGET destination={:02x?}",
            destination.as_bytes()
        );

        let link_id = match commands.establish_link(destination).await {
            Ok(link_id) => {
                println!("RESOURCE_LINK_OK link_id={:02x?}", link_id.as_bytes());
                link_id
            }
            Err(failure) => {
                println!("RESOURCE_LINK_FAIL {failure:?}");
                return;
            }
        };

        for iteration in 1..=iterations {
            let mut payload = vec![0u8; total_bytes];
            getrandom::getrandom(&mut payload).expect("OS CSPRNG fills the payload");
            let started = Instant::now();
            match commands
                .send_resource(link_id, total_bytes as u64, std::io::Cursor::new(payload))
                .await
            {
                Ok(()) => {
                    let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
                    let kbps = (total_bytes as u64) * 1000 / elapsed_ms;
                    println!(
                        "RESOURCE_RESULT iter={iteration} bytes={total_bytes} elapsed_ms={elapsed_ms} \
                         goodput_kBps={:.1} goodput_kbps={:.1}",
                        kbps as f64 / 1000.0,
                        kbps as f64 * 8.0 / 1000.0,
                    );
                }
                Err(failure) => {
                    println!("RESOURCE_SEND_FAIL iter={iteration} {failure:?}");
                    break;
                }
            }
        }
        println!("RESOURCE_DONE");
    };
    tokio::select! {
        () = node.run() => unreachable!("the resource node's run loop returned"),
        () = driver => {}
    }
}
