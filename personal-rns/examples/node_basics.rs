//! A complete, bounded two-node Reticulum exchange over an isolated localhost TCP link.

use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::interfaces::bluetooth_auto::{BleIdentity, BLE_IDENTITY_LEN};
use personal_rns::interfaces::{BitrateBps, InterfaceKind};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::routes;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Manual, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestHandlerRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::{
    fill_os_entropy, try_generate_identity_secret, AttachIntent, DefaultAutoInterfaces,
};

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

fn destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-guide",
        aspects: &["announce"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"hello from node A",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: RequestHandlerRegistration::None,
    })
}

fn auto_identity() -> Result<BleIdentity, Box<dyn Error>> {
    let mut bytes = [0_u8; BLE_IDENTITY_LEN];
    fill_os_entropy(&mut bytes)?;
    Ok(BleIdentity::new(bytes))
}

fn with_auto_from_args() -> Result<bool, Box<dyn Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(false),
        [argument] if argument == "--with-auto" => Ok(true),
        [argument] if argument == "--help" || argument == "-h" => {
            println!("usage: cargo tools guide rust [-- --with-auto]");
            println!("  --with-auto  also attach Wi-Fi, USB, and Bluetooth auto interfaces");
            std::process::exit(0);
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected no arguments or exactly --with-auto",
        )
        .into()),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn Error>> {
    let with_auto = with_auto_from_args()?;
    let destination_a = destination()?;
    let destination_a_hash = destination_a.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;
    let destination_b = destination()?;

    let server = TcpServer::bind("127.0.0.1:0", BITRATE).await?;
    let server_address = server.local_addr()?.to_string();
    println!("Node A: TCP server listening on {server_address}");

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination_a],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: Manual,
    });
    let node_a_handle = node_a.handle();
    let _server = node_a_handle.supervise(server);

    let client = TcpClientInterface::new(server_address, BITRATE, ReconnectPolicy::STANDARD);
    let optional_auto_identity = with_auto.then(auto_identity).transpose()?;
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination_b],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
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
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
            if let Some(identity) = optional_auto_identity {
                DefaultAutoInterfaces::new(identity).attach(node);
            }
        },
    });
    let node_b_handle = node_b.handle();

    if with_auto {
        println!("Node B: TCP plus explicitly enabled Wi-Fi, USB, and Bluetooth auto interfaces");
    } else {
        println!("Node B: TCP client only (safe default; no radio or USB discovery)");
    }

    let announcer = node_a_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
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

    let observed = tokio::select! {
        heard = tokio::time::timeout(DELIVERY_TIMEOUT, heard_rx.recv()) => {
            heard
                .map_err(|_| io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Node B did not observe Node A's announce over TCP within 10 seconds",
                ))?
                .ok_or_else(|| io::Error::other("Node B's event stream closed before delivery"))?
        }
        result = node_a.run() => {
            result?;
            return Err(io::Error::other("Node A stopped before delivery").into());
        }
        result = node_b.run() => {
            result?;
            return Err(io::Error::other("Node B stopped before delivery").into());
        }
    };

    if observed.0 != destination_a_hash {
        return Err(io::Error::other("Node B observed the wrong destination").into());
    }
    if observed.1.kind() != Some(InterfaceKind::TcpClient) {
        return Err(
            io::Error::other("Node B's announce did not arrive through its TCP client").into(),
        );
    }

    println!(
        "Success: Node B observed Node A's real Reticulum announce on {:?} ({:?}).",
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
    Ok(())
}
