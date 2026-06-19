#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    use core::time::Duration;
    use std::string::String;

    use personal_rns::engine::RatchetPolicy;
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::interfaces::bluetooth_auto::core::{
        BleIdentity, LinkCapabilities, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::routing::ProofStrategy;
    use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe};
    use personal_rns::storage::GrowableHeap;
    use personal_rns::{interfaces, routes};
    use personal_rns_ffi::ble::macos::MacosBleBackend;

    let node_byte: u8 = 0x33;

    let backend = match MacosBleBackend::new().await {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("bluetooth did not power on: {error:?}");
            eprintln!("grant Bluetooth access in System Settings > Privacy & Security > Bluetooth");
            return;
        }
    };
    let identity = BleIdentity::new([node_byte; 16]);
    let capabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: BLE_HW_MTU as u16,
    };

    let me = PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "hopspot",
        aspects: &["node"],
        identity: Zeroizing::new([node_byte; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"bluetooth-macos-node",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };

    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [me],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: move |_event, _state| {},
    });
    let handle = node.handle();
    let _bluetooth = handle.supervise(BluetoothAuto::new(backend, identity, capabilities));
    println!("[macos] up — supervising native bluetooth (CoreBluetooth), GATT-only handshake");

    let roll_call = handle.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            let summary: std::vec::Vec<String> = roll_call
                .interface_snapshots()
                .iter()
                .map(|snap| std::format!("{:?}/{:?}", snap.id.kind(), snap.connection))
                .collect();
            println!("[macos] interfaces: {summary:?}");
        }
    });

    node.run().await;
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the `bluetooth_node` example is macOS-only (it drives the CoreBluetooth backend)");
}
