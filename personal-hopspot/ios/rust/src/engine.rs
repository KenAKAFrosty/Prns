use std::fmt::Write as _;
#[cfg(target_os = "ios")]
use std::net::SocketAddr;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use personal_hopspot_ui::{card_label, CardKind, CardLabel};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAutoStatus;
use personal_rns::interfaces::rns_parity::wifi_auto::{
    core as wifi_core, AutoWifi, AutoWifiStatus,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe, TokioPrnsHandle,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use personal_rns::{interfaces, routes};

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(8);

struct Engine {
    handle: TokioPrnsHandle,
    wifi_status: AutoWifiStatus,
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(spawn_engine)
}

pub(crate) fn start() {
    let _ = engine();
}

pub(crate) fn interface_snapshots() -> std::vec::Vec<InterfaceSnapshot> {
    engine().handle.interfaces()
}

pub(crate) fn toggle_interface(id: InterfaceId) {
    let engine = engine();
    if id == engine.wifi_status.id() {
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
    engine.wifi_status.set_enabled(false);
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.set_enabled(false);
        }
    }
}

pub(crate) fn wake_interfaces() {
    let engine = engine();
    engine.wifi_status.set_enabled(true);
    if let Ok(slot) = engine.ble_status.lock() {
        if let Some(status) = slot.as_ref() {
            status.set_enabled(true);
        }
    }
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, CardLabel)> {
    match id.kind() {
        Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("WiFi/LAN"))),
        Some(InterfaceKind::BluetoothAuto) => Some((CardKind::Ble, card_label("BLE"))),
        Some(InterfaceKind::TcpServerPeer | InterfaceKind::TcpClient | InterfaceKind::WifiPeer) => {
            Some(peer_card(id, CardKind::Peer, "LAN"))
        }
        Some(InterfaceKind::BluetoothPeer) => Some(peer_card(id, CardKind::Ble, "BLE")),
        _ => None,
    }
}

fn peer_card(id: InterfaceId, kind: CardKind, tag: &str) -> (CardKind, CardLabel) {
    let bytes = id.as_bytes();
    let mut label = CardLabel::new();
    let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
    (kind, label)
}

struct Ready {
    handle: TokioPrnsHandle,
    wifi_status: AutoWifiStatus,
}

fn spawn_engine() -> Engine {
    let ble_status = Arc::new(Mutex::new(None));
    let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
    let worker_ble_status = Arc::clone(&ble_status);
    let _ = thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(ready_tx, worker_ble_status));
    let ready = ready_rx
        .recv()
        .expect("the engine hands its handle out before run() starts");
    Engine {
        handle: ready.handle,
        wifi_status: ready.wifi_status,
        ble_status,
    }
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine(ready_tx: Sender<Ready>, ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>) {
    #[cfg(not(target_os = "ios"))]
    let _ = &ble_status;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the engine thread builds its tokio runtime");

    runtime.block_on(async move {
        let secret_key = load_identity_secret_key();
        let identity_hash = {
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
            *signer.identity_hash().as_bytes()
        };

        let announce_destination = PreConfiguredDestination::Single {
            resource_strategy: ResourceStrategy::AcceptNone,
            app_name: ANNOUNCE_APP_NAME,
            aspects: ANNOUNCE_ASPECTS,
            identity: secret_key,
            announce_app_data: ANNOUNCE_APP_DATA,
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::Ratcheted,
        };
        let destination = announce_destination
            .destination_hash()
            .expect("the lxmf.delivery name is valid");

        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [announce_destination],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            interfaces: interfaces![],
            on_event: |_event: PrnsEvent<'_>, _state: &()| {},
        });
        let handle = node.handle();

        #[cfg(target_os = "ios")]
        let (mdns_tx, mdns_rx) = tokio::sync::mpsc::unbounded_channel::<SocketAddr>();
        #[cfg(target_os = "ios")]
        let wifi = AutoWifi::new().with_mdns(mdns_rx);
        #[cfg(not(target_os = "ios"))]
        let wifi = AutoWifi::new();
        let wifi_status = wifi.status();
        handle.supervise(wifi);
        println!(
            "HOPSPOT_IOS_ENGINE supervising WiFi/LAN: rendezvous on 0.0.0.0:{} (usbmux loopback rides it) + Bonjour mDNS discovery",
            wifi_core::TCP_RENDEZVOUS_PORT
        );
        #[cfg(target_os = "ios")]
        spawn_mdns(wifi_core::TCP_RENDEZVOUS_PORT, mdns_tx);

        #[cfg(target_os = "ios")]
        spawn_bluetooth(handle.clone(), identity_hash, ble_status);
        #[cfg(not(target_os = "ios"))]
        let _ = identity_hash;

        let _ = ready_tx.send(Ready {
            handle: handle.clone(),
            wifi_status,
        });
        tokio::spawn(announce_loop(handle, destination));

        node.run().await;
    });
}

/// The native CoreBluetooth BLE auto-interface as a supervised fleet, on its own task so a slow or
/// denied radio never blocks the node coming up — the macOS desktop pattern, with the iOS endpoint.
/// `MacosBleBackend::new` awaits power-on and the L2CAP publish; on failure (most often Bluetooth not
/// granted) it logs and the node runs without BLE.
#[cfg(target_os = "ios")]
fn spawn_bluetooth(
    handle: TokioPrnsHandle,
    identity_hash: [u8; 16],
    status_slot: Arc<Mutex<Option<BluetoothAutoStatus>>>,
) {
    use personal_rns::interfaces::bluetooth_auto::core::{
        AppleHost, BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
    use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
    use personal_rns_ffi::ble::macos::MacosBleBackend;

    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match MacosBleBackend::new().await {
            Ok(backend) => {
                let psm = backend.psm();
                let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::Ios),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                if let Ok(mut slot) = status_slot.lock() {
                    *slot = Some(bluetooth.status());
                }
                handle.supervise(bluetooth);
                println!(
                    "bluetooth: supervising CoreBluetooth (iOS), L2CAP psm {:#06x}",
                    psm.get()
                );
            }
            Err(error) => {
                eprintln!(
                    "bluetooth disabled ({error:?}); grant Bluetooth in Settings > Privacy & Security > Bluetooth"
                );
            }
        }
    });
}

/// Advertise the TCP rendezvous over Bonjour and feed every resolved peer into the WiFi/LAN
/// supervisor's mDNS channel, so peers that cannot run raw multicast (iOS) are still discovered and
/// dialed. Standard Bonjour rides the system mDNSResponder, so this is free-team (Local Network
/// permission + the `NSBonjourServices` declaration), never the multicast entitlement.
#[cfg(target_os = "ios")]
fn spawn_mdns(port: u16, sightings: tokio::sync::mpsc::UnboundedSender<SocketAddr>) {
    use personal_rns_ffi::mdns::macos::MacosMdnsBackend;

    tokio::spawn(async move {
        match MacosMdnsBackend::new(port, &[("v", b"1")]).await {
            Ok(mut backend) => {
                println!("mdns: advertising + browsing _reticulum._tcp on :{port}");
                while let Some(addr) = backend.next_sighting().await {
                    println!("mdns: discovered peer rendezvous at {addr}");
                    if sightings.send(addr).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                eprintln!("mdns: failed ({error:?})");
            }
        }
    });
}

/// The face's announce cadence: the engine does not originate announces, so the app fires a scheduled
/// `lxmf.delivery` announce on its own timer through the handle.
async fn announce_loop(handle: TokioPrnsHandle, destination: DestinationHash) {
    let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        interval.tick().await;
        if handle
            .issue(EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }))
            .is_none()
        {
            return;
        }
    }
}
