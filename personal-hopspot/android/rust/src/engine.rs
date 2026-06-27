use core::fmt::Write as _;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use personal_hopspot_ui::{card_label, CardKind, CardLabel};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::core::{
    AndroidHost, BleIdentity, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
};
use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAutoStatus;
use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
use personal_rns::interfaces::rns_parity::local::impls::tokio::LocalServer;
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::interfaces::usb_auto::impls::tokio::UsbAutoHost;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe, TokioPrnsHandle};
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

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(8);

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
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RuntimeHealth {
    pub uptime_millis: u64,
    pub interface_count: u32,
    pub online_interface_count: u32,
    pub local_client_count: u32,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

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

/// Fire an immediate `lxmf.delivery` announce across every interface — the manual counterpart to the
/// scheduled [`announce_loop`], driven by the Hopspot's global "Announce" menu item.
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

impl RuntimeHealth {
    fn from_snapshots(uptime: Duration, snapshots: &[InterfaceSnapshot]) -> Self {
        let mut health = Self {
            uptime_millis: millis_u64(uptime),
            interface_count: snapshots.len() as u32,
            ..Self::default()
        };
        for snapshot in snapshots {
            if matches!(
                snapshot.connection,
                personal_rns::interfaces::ConnectionState::Connected
                    | personal_rns::interfaces::ConnectionState::Degraded
            ) {
                health.online_interface_count = health.online_interface_count.saturating_add(1);
            }
            if snapshot.id.kind() == Some(InterfaceKind::LocalClient) {
                health.local_client_count = health.local_client_count.saturating_add(1);
            }
            health.route_count = health.route_count.saturating_add(snapshot.destinations);
            health.link_count = health.link_count.saturating_add(snapshot.links);
            health.transported_link_count = health
                .transported_link_count
                .saturating_add(snapshot.transported_links);
            health.rx_bytes = health.rx_bytes.saturating_add(snapshot.rx_bytes);
            health.tx_bytes = health.tx_bytes.saturating_add(snapshot.tx_bytes);
            if let Some(rates) = snapshot.transfer_rates {
                health.rx_bps = health.rx_bps.saturating_add(u64::from(rates.rx_bps));
                health.tx_bps = health.tx_bps.saturating_add(u64::from(rates.tx_bps));
            }
        }
        health
    }
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        let identity_hash = {
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&identity);
            *signer.identity_hash().as_bytes()
        };
        let transport_id = TransportId::new(identity_hash);
        let ble_identity = BleIdentity::new(identity_hash);

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

        handle.supervise(LocalServer::new());

        let wifi = match mdns.take_receiver() {
            Some(rx) => AutoWifi::new().with_mdns(rx),
            None => AutoWifi::new(),
        };
        let wifi_status = wifi.status();
        handle.supervise(wifi);

        {
            let handle = handle.clone();
            tokio::spawn(async move {
                let psm = ble.await_psm().await;
                let Some(psm) = Psm::new(psm) else {
                    return;
                };
                let bluetooth = BluetoothAuto::<_, { AndroidBleBackend::MAX_PEERS }>::new(
                    AndroidBleBackend::new(ble),
                    ble_identity,
                    Endpoint::Android(AndroidHost::Android),
                    LinkCapabilities {
                        l2cap: Some(psm),
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
        });

        tokio::spawn(announce_loop(handle, destination));
        node.run().await;
    });
}

/// The face's announce cadence: the engine does not originate announces, so the app fires a
/// scheduled `lxmf.delivery` announce on its own timer. The handle mints the command id.
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

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{ConnectionState, Membership, TransferRates};

    #[test]
    fn runtime_health_aggregates_interface_snapshots() {
        let local_client = InterfaceSnapshot {
            id: InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app"),
            connection: ConnectionState::Connected,
            rx_bytes: 10,
            tx_bytes: 20,
            transfer_rates: Some(TransferRates {
                rx_bps: 3,
                tx_bps: 4,
            }),
            destinations: 2,
            links: 1,
            transported_links: 0,
            membership: Membership::Independent,
        };
        let wifi_peer = InterfaceSnapshot {
            id: InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer"),
            connection: ConnectionState::Reconnecting,
            rx_bytes: 5,
            tx_bytes: 7,
            transfer_rates: None,
            destinations: 1,
            links: 0,
            transported_links: 2,
            membership: Membership::FleetMember {
                supervisor_id: InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"wifi"),
            },
        };

        let health =
            RuntimeHealth::from_snapshots(Duration::from_millis(123), &[local_client, wifi_peer]);

        assert_eq!(health.uptime_millis, 123);
        assert_eq!(health.interface_count, 2);
        assert_eq!(health.online_interface_count, 1);
        assert_eq!(health.local_client_count, 1);
        assert_eq!(health.route_count, 3);
        assert_eq!(health.link_count, 1);
        assert_eq!(health.transported_link_count, 2);
        assert_eq!(health.rx_bytes, 15);
        assert_eq!(health.tx_bytes, 27);
        assert_eq!(health.rx_bps, 3);
        assert_eq!(health.tx_bps, 4);
    }
}
