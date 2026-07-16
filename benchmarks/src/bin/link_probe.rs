use std::time::Instant;

use personal_rns::engine::RatchetPolicy;
use personal_rns::routes;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, Manual, PreConfiguredDestination, Prns, PrnsEvent,
    PrnsRecipe,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, OnExisting, RnsLocalBlackholeFile, Role,
    SharedInstanceCredentials, SharedInstanceIntent,
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
    let usage = "usage: link_probe <bus-port> <dest-hex>";
    let port: u16 = args.next().expect(usage).parse().expect("bus-port");
    let target = parse_hex(&args.next().expect(usage));

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("scenario runtime")
        .block_on(run(port, target));
}

async fn run(port: u16, target: Vec<u8>) {
    let single = PreConfiguredDestination::Single {
        app_name: "linkprobe",
        aspects: &["probe"],
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let node = Prns::new(PrnsRecipe {
        transport_identity: None,
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
                credentials: SharedInstanceCredentials::from_identity_secret(
                    &[0xA5; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
                ),
                blackhole_file: RnsLocalBlackholeFile::new(
                    std::env::temp_dir().join(std::format!("prns-link-probe-{port}-blackhole")),
                ),
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
        println!("PROBE_READY bus={port} target={target:02x?}");
        loop {
            let destination = heard_rx
                .recv()
                .await
                .expect("hears an announce over the bus");
            if destination.as_bytes() == target.as_slice() {
                println!(
                    "PROBE_HEARD_TARGET destination={:02x?}",
                    destination.as_bytes()
                );
                let started = Instant::now();
                match commands.establish_link(destination).await {
                    Ok(link_id) => println!(
                        "PROBE_LINK_OK link_id={:02x?} elapsed_ms={}",
                        link_id.as_bytes(),
                        started.elapsed().as_millis(),
                    ),
                    Err(failure) => println!("PROBE_LINK_FAIL {failure:?}"),
                }
                return;
            }
        }
    };
    tokio::select! {
        () = node.run() => unreachable!("the probe's run loop returned"),
        () = driver => {}
    }
}
