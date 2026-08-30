//! Two nodes with no addresses anywhere in the code find each other over Wi-Fi auto-discovery. See `docs/getting-started.md` for context.

#![expect(clippy::expect_used, clippy::panic)]

use core::time::Duration;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const LISTEN_WINDOW: Duration = Duration::from_secs(60);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let destination_a = example_preconfigured_destination();
    let destination_a_hash = destination_a
        .destination_hash()
        .expect("invalid example destination name");

    let destination_b = example_preconfigured_destination();

    println!("Node A and Node B: Wi-Fi auto-discovery only; this program contains no addresses");

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(
            try_generate_identity_secret().expect("identity generation failed"),
        ),
        pre_configured_destinations: [destination_a],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        on_event: |_event, _state| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(AutoWifi::default());
        },
        persistence: NoPersistence,
    });
    let node_a_handle = node_a.handle();

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(
            try_generate_identity_secret().expect("identity generation failed"),
        ),
        pre_configured_destinations: [destination_b],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                source_interface,
                ..
            }) = event
            {
                let _ignored = heard_tx.send((destination, source_interface));
            }
        },
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(AutoWifi::default());
        },
        persistence: NoPersistence,
    });
    let node_b_handle = node_b.handle();

    let announcer = node_a_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: destination_a_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let mut run_a = std::pin::pin!(node_a.run());
    let mut run_b = std::pin::pin!(node_b.run());
    let mut announced_destinations = Vec::new();
    let deadline = tokio::time::Instant::now() + DELIVERY_TIMEOUT;
    let observed = loop {
        let heard = tokio::select! {
            heard = tokio::time::timeout_at(deadline, heard_rx.recv()) => {
                heard
                    .expect("Node B did not observe Node A's announce within 10 seconds")
                    .expect("Node B's event stream closed before delivery")
            }
            result = &mut run_a => {
                result.expect("Node A failed");
                panic!("Node A stopped before delivery");
            }
            result = &mut run_b => {
                result.expect("Node B failed");
                panic!("Node B stopped before delivery");
            }
        };
        if heard.0 == destination_a_hash {
            break heard;
        }
        if !announced_destinations.contains(&heard.0) {
            println!(
                "Heard an announce for {:?} via {:?}",
                heard.0,
                heard.1.kind()
            );
            announced_destinations.push(heard.0);
        }
    };

    println!(
        "Success: Node B found Node A with no wiring; the announce arrived on {:?} ({:?}).",
        observed.1,
        observed.1.kind()
    );
    println!("Node B interface inventory:");
    for interface in node_b_handle.interfaces() {
        println!(
            "  {:?} connection={:?} rx={} tx={}",
            interface.id, interface.connection, interface.rx_bytes, interface.tx_bytes
        );
    }

    announced_destinations.push(destination_a_hash);
    println!(
        "Listening {} more seconds for announces from other machines on this network; run this same command there.",
        LISTEN_WINDOW.as_secs()
    );
    let window_end = tokio::time::Instant::now() + LISTEN_WINDOW;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(window_end) => break,
            heard = heard_rx.recv() => {
                let Some((destination, source_interface)) = heard else { break };
                if !announced_destinations.contains(&destination) {
                    println!("Heard an announce for {destination:?} via {:?}", source_interface.kind());
                    announced_destinations.push(destination);
                }
            }
            result = &mut run_a => {
                result.expect("Node A failed");
                panic!("Node A stopped during the listen window");
            }
            result = &mut run_b => {
                result.expect("Node B failed");
                panic!("Node B stopped during the listen window");
            }
        }
    }
    println!("Listen window closed.");
}

fn example_preconfigured_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "auto-discovery"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"hello from node A",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
