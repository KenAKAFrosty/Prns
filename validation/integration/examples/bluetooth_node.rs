#[cfg(target_os = "macos")]
use prns_core::interfaces::bluetooth_auto::default_group_tag;
#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    use core::time::Duration;
    use personal_rns::runtime::NoPersistence;
    use std::string::String;

    use personal_rns::engine::RatchetPolicy;
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::interfaces::bluetooth_auto::{
        AppleHost, BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use personal_rns::request_endpoints;
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode,
        PrnsNodeRecipe, ServeMyRequestEndpoints,
    };
    use personal_rns::storage::GrowableHeap;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;
    use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;

    let _ = env_logger::try_init();

    let node_byte: u8 = 0x33;

    let identity = BleIdentity::new([node_byte; 16]);
    let backend = match MacosBleBackend::new(identity, default_group_tag()).await {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("bluetooth did not power on: {error:?}");
            eprintln!("grant Bluetooth access in System Settings > Privacy & Security > Bluetooth");
            return;
        }
    };
    let psm = backend.psm();
    let endpoint = Endpoint::CoreBluetooth(AppleHost::MacOs);
    let capabilities = LinkCapabilities {
        l2cap: Some(psm),
        link_mtu: BLE_HW_MTU as u16,
    };

    let me = PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "hopspot",
        aspects: &["node"],
        identity: Zeroizing::new([node_byte; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"bluetooth-macos-node",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    };

    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [me],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                source_interface,
                hops,
                destination,
                app_data: _,
            }) = event
            {
                println!(
                    "[macos] HEARD dest={:02x}{:02x} via {:?} ({hops} hop) — a frame crossed the L2CAP data plane",
                    destination.as_bytes()[0],
                    destination.as_bytes()[1],
                    source_interface.kind(),
                );
            }
        },
    });
    let handle = node.handle();
    let _bluetooth = handle.supervise(BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::new(
        backend,
        identity,
        endpoint,
        capabilities,
        default_group_tag(),
    ));
    println!(
        "[macos] up — supervising native bluetooth (CoreBluetooth), L2CAP psm {:#06x}",
        psm.get()
    );

    let roll_call = handle.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            let summary: std::vec::Vec<String> = roll_call
                .interfaces()
                .iter()
                .map(|snap| std::format!("{:?}/{:?}", snap.id.kind(), snap.connection))
                .collect();
            println!("[macos] interfaces: {summary:?}");
        }
    });

    if let Err(error) = node.run().await {
        eprintln!("node stopped: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the `bluetooth_node` example is macOS-only (it drives the CoreBluetooth backend)");
}
