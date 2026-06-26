//! A headless native-Bluetooth (BlueR) node for live first-light testing of the LE auto-interface:
//! it opens the default Bluetooth adapter, supervises `BluetoothAuto` over the BlueR backend, and
//! rolls the interface table every few seconds so a peer link shows up as a `BluetoothPeer` the
//! moment a handshake settles. The node advertises the shared Reticulum BLE service and scans for
//! it, so two such nodes (or a node and an Android peer) discover each other on the radio.
//!
//! Run it, then confirm on the radio from another shell:
//!   busctl get-property org.bluez /org/bluez/hci0 org.bluez.Adapter1 Discovering
//!   busctl get-property org.bluez /org/bluez/hci0 org.bluez.LEAdvertisingManager1 ActiveInstances
//!
//!   NODE=11 NAME=A cargo run --example bluetooth_auto_node --features bluetooth-bluer
//!
//! Demo code: it `expect`s on adapter open.

#![cfg(all(feature = "bluetooth-bluer", target_os = "linux"))]
#![allow(clippy::expect_used)]

use core::time::Duration;
use std::string::String;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::core::{
    BleIdentity, BlueZHost, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
};
use personal_rns::interfaces::bluetooth_auto::impls::bluer::BluerBackend;
use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};

const CONTROL_PSM: u16 = 0x0083;

#[tokio::main]
async fn main() {
    env_logger::init();
    let node_byte = std::env::var("NODE")
        .ok()
        .and_then(|v| u8::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x11);
    let name = std::env::var("NAME").unwrap_or_else(|_| std::format!("{node_byte:02x}"));

    let psm = Psm::new(CONTROL_PSM).expect("control PSM is in the LE dynamic range");
    let backend = BluerBackend::open(psm)
        .await
        .expect("open default bluetooth adapter");
    let identity = BleIdentity::new([node_byte; 16]);
    let capabilities = LinkCapabilities {
        l2cap: Some(psm),
        link_mtu: BLE_HW_MTU as u16,
    };

    let me = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "hopspot",
        aspects: &["node"],
        identity: Zeroizing::new([node_byte; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"bluetooth-auto-node",
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
    let _bluetooth = handle.supervise(BluetoothAuto::<_, { BluerBackend::MAX_PEERS }>::new(
        backend,
        identity,
        Endpoint::BlueZ(BlueZHost::Linux),
        capabilities,
    ));
    std::println!(
        "[{name}] up — supervising native bluetooth (BlueR), control PSM {CONTROL_PSM:#x}"
    );

    let roll_call = handle.clone();
    let roll_name = name.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            let summary: std::vec::Vec<String> = roll_call
                .interfaces()
                .iter()
                .map(|snap| std::format!("{:?}/{:?}", snap.id.kind(), snap.connection))
                .collect();
            std::println!("[{roll_name}] interfaces: {summary:?}");
        }
    });

    node.run().await;
}
