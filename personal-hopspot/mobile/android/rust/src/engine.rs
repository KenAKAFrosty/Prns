use core::fmt::Write as _;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use personal_hopspot_core::{card_label, CardKind, CardLabel};
use personal_rns::bluetooth_auto::{BluetoothAuto, BluetoothAutoStatus};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::{
    AndroidHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::reactor::tokio::TokioInterfaceStatus;
use personal_rns::routes;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    ephemeral_ble_identity, Manual, PreConfiguredDestination, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestHandlerRegistration, RuntimeHealth,
};
use personal_rns::shared_instance::rns_rpc::{SharedInstanceCredentials, SharedInstanceRpcServer};
use personal_rns::shared_instance::SharedInstanceServer;
use personal_rns::storage::GrowableHeap;
use personal_rns::usb_auto::UsbAutoHost;
use personal_rns::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::wifi_aware::{WifiAwareAuto, WifiAwareStatus};
use personal_rns::wifi_direct::WifiDirectStatus;
use personal_rns::wire::DestinationHash;

use crate::bluetooth_auto::{AndroidBleBackend, AndroidBleBridge};
use crate::bridge::{AndroidUsbBridge, BridgeStream};
use crate::mdns::AndroidMdnsBridge;
use crate::wifi_aware::{AndroidWifiAwareBackend, AndroidWifiAwareBridge};
use crate::wifi_direct::AndroidWifiDirectBridge;

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
const ANDROID_PORT: &str = "android-usb";
const LOCAL_RNS_PORT: u16 = 37428;
const RPC_PORT: u16 = LOCAL_RNS_PORT + 1;

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

struct Engine {
    started_at: Instant,
    usb_status: TokioInterfaceStatus,
    wifi_status: AutoWifiStatus,
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    wd_status: Arc<Mutex<Option<WifiDirectStatus>>>,
    wa_status: Arc<Mutex<Option<WifiAwareStatus>>>,
    bridge: AndroidUsbBridge,
    ble: AndroidBleBridge,
    wd: AndroidWifiDirectBridge,
    wa: AndroidWifiAwareBridge,
    mdns: AndroidMdnsBridge,
    handle: PrnsNodeHandle,
    destination: DestinationHash,
    rpc_key: Vec<u8>,
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
    for byte in &engine().rpc_key {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn toggle_interface(id: InterfaceId) {
    let engine = engine();
    if id == USB_INTERFACE_ID {
        engine.usb_status.toggle_enabled();
    } else if id == engine.wifi_status.id() {
        engine.wifi_status.toggle_enabled();
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        if let Ok(slot) = engine.ble_status.lock() {
            if let Some(status) = slot.as_ref() {
                status.toggle_enabled();
            }
        }
    } else if id.kind() == Some(InterfaceKind::WifiDirect) {
        if let Ok(slot) = engine.wd_status.lock() {
            if let Some(status) = slot.as_ref() {
                status.toggle_enabled();
            }
        }
    } else if id.kind() == Some(InterfaceKind::WifiAware) {
        if let Ok(slot) = engine.wa_status.lock() {
            if let Some(status) = slot.as_ref() {
                status.toggle_enabled();
            }
        }
    }
}

pub(crate) fn sleep_interfaces() {
    let engine = engine();
    engine.usb_status.disable();
    engine.wifi_status.disable();
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.disable();
        }
    }
    if let Ok(slot) = engine.wd_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.disable();
        }
    }
    if let Ok(slot) = engine.wa_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.disable();
        }
    }
}

pub(crate) fn wake_interfaces() {
    let engine = engine();
    engine.usb_status.enable();
    engine.wifi_status.enable();
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.enable();
        }
    }
    if let Ok(slot) = engine.wd_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.enable();
        }
    }
    if let Ok(slot) = engine.wa_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.enable();
        }
    }
}

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

pub(crate) fn wd_bridge() -> AndroidWifiDirectBridge {
    engine().wd.clone()
}

pub(crate) fn wa_bridge() -> AndroidWifiAwareBridge {
    engine().wa.clone()
}

pub(crate) fn mdns_bridge() -> AndroidMdnsBridge {
    engine().mdns.clone()
}

pub(crate) fn ensure_started() {
    let _ = engine();
}

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
    } else if id.kind() == Some(InterfaceKind::WifiDirect) {
        Some((CardKind::Wifi, card_label("WiFi Direct")))
    } else if id.kind() == Some(InterfaceKind::WifiAware) {
        Some((CardKind::Wifi, card_label("WiFi/P2P")))
    } else {
        let bytes = id.as_bytes();
        let (kind, tag) = match id.kind() {
            Some(InterfaceKind::BluetoothPeer) => (CardKind::Ble, "BLE"),
            Some(InterfaceKind::WifiDirectPeer) => (CardKind::Wifi, "Direct"),
            Some(InterfaceKind::WifiAwarePeer) => (CardKind::Wifi, "P2P"),
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
    handle: PrnsNodeHandle,
    destination: DestinationHash,
    rpc_key: Vec<u8>,
}

fn spawn_engine() -> Engine {
    let bridge = AndroidUsbBridge::new();
    let ble = AndroidBleBridge::new();
    let ble_status = Arc::new(Mutex::new(None));
    let wd = AndroidWifiDirectBridge::new();
    let wd_status = Arc::new(Mutex::new(None));
    let wa = AndroidWifiAwareBridge::new();
    let wa_status = Arc::new(Mutex::new(None));
    let mdns = AndroidMdnsBridge::new();
    let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
    let worker_bridge = bridge.clone();
    let worker_ble = ble.clone();
    let worker_ble_status = Arc::clone(&ble_status);
    let worker_wa = wa.clone();
    let worker_wa_status = Arc::clone(&wa_status);
    let worker_mdns = mdns.clone();
    let _ = thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || {
            run_engine(
                ready_tx,
                worker_bridge,
                worker_ble,
                worker_ble_status,
                worker_wa,
                worker_wa_status,
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
        wd_status,
        wa_status,
        bridge,
        ble,
        wd,
        wa,
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

#[allow(clippy::too_many_arguments)]
fn run_engine(
    ready_tx: Sender<Ready>,
    bridge: AndroidUsbBridge,
    ble: AndroidBleBridge,
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    wa: AndroidWifiAwareBridge,
    wa_status: Arc<Mutex<Option<WifiAwareStatus>>>,
    mdns: AndroidMdnsBridge,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the engine thread builds its tokio runtime");

    runtime.block_on(async move {
        let identity = load_identity_secret_key();
        let credentials = SharedInstanceCredentials::from_identity_secret(&identity);
        let rpc_key = credentials.rpc_key().as_bytes().to_vec();
        let transport_secret = identity.clone();
        let ble_identity = ephemeral_ble_identity();
        ble.set_local_identity(ble_identity);

        let announce_destination = PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: ANNOUNCE_APP_NAME,
            aspects: ANNOUNCE_ASPECTS,
            identity,
            announce_app_data: ANNOUNCE_APP_DATA,
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::Ratcheted,
            request_handlers: RequestHandlerRegistration::None,
        };
        let destination = announce_destination
            .destination_hash()
            .expect("the lxmf.delivery name is valid");

        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(transport_secret),
            pre_configured_destinations: [announce_destination],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            interfaces: Manual,
            on_event: |_event, _state: &()| {},
        });
        let handle = node.handle();

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

        handle.supervise(SharedInstanceServer::with_port(LOCAL_RNS_PORT));
        let rpc = match SharedInstanceRpcServer::tcp(credentials, RPC_PORT, handle.clone())
            .bind()
            .await
        {
            Ok(rpc) => rpc,
            Err(error) => {
                log::error!("shared-instance RPC listener failed to bind: {error:?}");
                return;
            }
        };
        tokio::spawn(rpc.run());

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

        {
            let handle = handle.clone();
            tokio::spawn(async move {
                let wifi_aware = WifiAwareAuto::new(AndroidWifiAwareBackend::new(wa));
                let status = wifi_aware.status();
                if let Ok(mut slot) = wa_status.lock() {
                    *slot = Some(status);
                }
                handle.supervise(wifi_aware);
            });
        }

        let _ = ready_tx.send(Ready {
            usb_status,
            wifi_status,
            handle: handle.clone(),
            destination,
            rpc_key,
        });

        match node.run().await {
            Ok(()) => log::error!("hopspot engine stopped"),
            Err(error) => log::error!("hopspot engine stopped: {error}"),
        }
    });
}
