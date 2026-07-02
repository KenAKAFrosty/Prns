use core::fmt::Write as _;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use personal_hopspot_core::{card_label, CardKind, CardLabel};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::core::{
    AndroidHost, BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
};
use prns_interfaces_tokio::ble::tokio::BluetoothAuto;
use prns_interfaces_tokio::ble::tokio::BluetoothAutoStatus;
use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
use prns_interfaces_tokio::shared_instance::rpc_compat::SharedInstanceRpcCompat;
use prns_interfaces_tokio::shared_instance::server::LocalServer;
use personal_rns::interfaces::wifi_auto::{AutoWifi, AutoWifiStatus};
use prns_interfaces_tokio::usb::UsbAutoHost;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    PreConfiguredDestination, Prns, PrnsRecipe, RuntimeHealth, TokioPrnsHandle,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, TransportId};
use personal_rns::{interfaces, routes};

use crate::ble::{AndroidBleBackend, AndroidBleBridge};
use crate::bridge::{AndroidUsbBridge, BridgeStream};
use crate::mdns::AndroidMdnsBridge;

/// Stable id for the USB-auto host over the JNI bridge (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
/// The single conduit name the host's scan reports while a USB device is attached.
const ANDROID_PORT: &str = "android-usb";
const LOCAL_RNS_PORT: u16 = 37428;
const RPC_PORT: u16 = LOCAL_RNS_PORT + 1;

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

/// What the engine thread hands back once the node is assembled: the USB interface's status, the
/// WiFi supervisor's aggregate status (whose `members()` yield the per-peer cards), and the USB
/// bridge the JNI layer feeds. The node owns the runtime and drives it forever.
struct Engine {
    started_at: Instant,
    usb_status: TokioInterfaceStatus,
    wifi_status: AutoWifiStatus,
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    bridge: AndroidUsbBridge,
    ble: AndroidBleBridge,
    mdns: AndroidMdnsBridge,
    handle: TokioPrnsHandle,
    destination: DestinationHash,
    rpc_key: [u8; 32],
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(spawn_engine)
}

pub(crate) fn wifi_status() -> AutoWifiStatus {
    engine().wifi_status.clone()
}

pub(crate) fn interface_snapshots() -> std::vec::Vec<InterfaceSnapshot> {
    engine().handle.interfaces()
}

pub(crate) fn runtime_health() -> RuntimeHealth {
    let engine = engine();
    RuntimeHealth::from_snapshots(engine.started_at.elapsed(), &engine.handle.interfaces())
}

pub(crate) fn rpc_key_hex() -> String {
    let mut out = String::with_capacity(64);
    for byte in engine().rpc_key {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn toggle_interface(id: InterfaceId) {
    let engine = engine();
    if id == USB_INTERFACE_ID {
        engine
            .usb_status
            .set_enabled(!engine.usb_status.is_enabled());
    } else if id == engine.wifi_status.id() {
        engine
            .wifi_status
            .set_enabled(!engine.wifi_status.is_enabled());
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        if let Ok(slot) = engine.ble_status.lock() {
            if let Some(status) = slot.as_ref() {
                status.set_enabled(!status.is_enabled());
            }
        }
    }
}

pub(crate) fn sleep_interfaces() {
    let engine = engine();
    engine.usb_status.set_enabled(false);
    engine.wifi_status.set_enabled(false);
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.set_enabled(false);
        }
    }
}

pub(crate) fn wake_interfaces() {
    let engine = engine();
    engine.usb_status.set_enabled(true);
    engine.wifi_status.set_enabled(true);
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.set_enabled(true);
        }
    }
}

/// Fire an immediate `lxmf.delivery` announce across every interface, driven by the Hopspot's
/// global "Announce" menu item.
pub(crate) fn announce() {
    let engine = engine();
    log::info!("hopspot: manual announce -> lxmf.delivery on every interface");
    let _ = engine.handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
        destination: engine.destination,
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    }));
}

pub(crate) fn usb_bridge() -> AndroidUsbBridge {
    engine().bridge.clone()
}

pub(crate) fn ble_bridge() -> AndroidBleBridge {
    engine().ble.clone()
}

pub(crate) fn mdns_bridge() -> AndroidMdnsBridge {
    engine().mdns.clone()
}

/// Trigger the engine to spawn (idempotent), without taking a handle. The face calls this at
/// startup so the node is running before the first frame asks for its status.
pub(crate) fn ensure_started() {
    let _ = engine();
}

/// The card icon and label for an interface id: the fixed USB host, the WiFi supervisor's
/// aggregate, or one of its peers — the peer carries a short hex tag of its medium-derived id.
pub(crate) fn classify(id: InterfaceId, wifi_id: InterfaceId) -> Option<(CardKind, CardLabel)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, card_label("USB")))
    } else if id == wifi_id {
        Some((CardKind::Wifi, card_label("WiFi/LAN")))
    } else if id.kind() == Some(InterfaceKind::LocalServer) {
        Some((CardKind::Tcp, card_label("Local")))
    } else if id.kind() == Some(InterfaceKind::LocalClient) {
        Some((CardKind::Peer, card_label("App")))
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        Some((CardKind::Ble, card_label("BLE")))
    } else {
        let bytes = id.as_bytes();
        let (kind, tag) = match id.kind() {
            Some(InterfaceKind::BluetoothPeer) => (CardKind::Ble, "BLE"),
            _ => (CardKind::Peer, "Peer"),
        };
        let mut label = CardLabel::new();
        let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
        Some((kind, label))
    }
}

struct Ready {
    usb_status: TokioInterfaceStatus,
    wifi_status: AutoWifiStatus,
    handle: TokioPrnsHandle,
    destination: DestinationHash,
    rpc_key: [u8; 32],
}

fn spawn_engine() -> Engine {
    let bridge = AndroidUsbBridge::new();
    let ble = AndroidBleBridge::new();
    let ble_status = Arc::new(Mutex::new(None));
    let mdns = AndroidMdnsBridge::new();
    let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
    let worker_bridge = bridge.clone();
    let worker_ble = ble.clone();
    let worker_ble_status = Arc::clone(&ble_status);
    let worker_mdns = mdns.clone();
    let _ = thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || {
            run_engine(
                ready_tx,
                worker_bridge,
                worker_ble,
                worker_ble_status,
                worker_mdns,
            )
        });
    let ready = ready_rx
        .recv()
        .expect("the engine hands its status handles out before run() starts");
    Engine {
        started_at: Instant::now(),
        usb_status: ready.usb_status,
        wifi_status: ready.wifi_status,
        ble_status,
        bridge,
        ble,
        mdns,
        handle: ready.handle,
        destination: ready.destination,
        rpc_key: ready.rpc_key,
    }
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine(
    ready_tx: Sender<Ready>,
    bridge: AndroidUsbBridge,
    ble: AndroidBleBridge,
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    mdns: AndroidMdnsBridge,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the engine thread builds its tokio runtime");

    runtime.block_on(async move {
        let identity = load_identity_secret_key();
        let rpc_key = personal_rns::crypto::sha256(&*identity);
        let identity_hash = {
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&identity);
            *signer.identity_hash().as_bytes()
        };
        let transport_id = TransportId::new(identity_hash);
        let ble_identity = BleIdentity::new(identity_hash);
        ble.set_local_identity(ble_identity);

        let announce_destination = PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: ANNOUNCE_APP_NAME,
            aspects: ANNOUNCE_ASPECTS,
            identity,
            announce_app_data: ANNOUNCE_APP_DATA,
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::Ratcheted,
        };
        let destination = announce_destination
            .destination_hash()
            .expect("the lxmf.delivery name is valid");

        let node = Prns::new(PrnsRecipe {
            transport: Some(transport_id),
            pre_configured_destinations: [announce_destination],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            interfaces: interfaces![],
            on_event: |_event, _state: &()| {},
        });
        let handle = node.handle();

        // USB over the JNI bridge: report the one conduit present while a device is attached, and
        // open a fresh stream over it on each (re)connection.
        let scan = {
            let bridge = bridge.clone();
            move || {
                if bridge.is_connected() {
                    std::vec![ANDROID_PORT.to_string()]
                } else {
                    std::vec::Vec::new()
                }
            }
        };
        let open = {
            let bridge = bridge.clone();
            move |_name: String| {
                let bridge = bridge.clone();
                async move { Ok::<BridgeStream, io::Error>(bridge.open_stream()) }
            }
        };
        let usb = UsbAutoHost::new(USB_INTERFACE_ID, scan, open, bridge.rescan());
        let usb_status = usb.status();
        handle.add_interface(usb);

        handle.supervise(LocalServer::with_port(LOCAL_RNS_PORT));
        let fleet = handle.clone();
        tokio::spawn(
            SharedInstanceRpcCompat::tcp(rpc_key, RPC_PORT, handle.clone())
                .with_interfaces(move || fleet.interface_vitals())
                .run(),
        );

        let wifi = match mdns.take_receiver() {
            Some(rx) => AutoWifi::new().with_mdns(rx),
            None => AutoWifi::new(),
        };
        let wifi_status = wifi.status();
        handle.supervise(wifi);

        {
            let handle = handle.clone();
            tokio::spawn(async move {
                let bluetooth = BluetoothAuto::<_, { AndroidBleBackend::MAX_PEERS }>::new(
                    AndroidBleBackend::new(ble),
                    ble_identity,
                    Endpoint::Android(AndroidHost::Android),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                let status = bluetooth.status();
                if let Ok(mut slot) = ble_status.lock() {
                    *slot = Some(status);
                }
                handle.supervise(bluetooth);
            });
        }

        let _ = ready_tx.send(Ready {
            usb_status,
            wifi_status,
            handle: handle.clone(),
            destination,
            rpc_key,
        });

        node.run().await;
    });
}
