use core::fmt::Write as _;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::interfaces::shared_instance as instance_core;
use personal_rns::interfaces::tcp;
use personal_rns::interfaces::{InterfaceId, InterfaceKind};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::manifold::tokio::TokioInterfaceStatus;
use personal_rns::prelude::*;
use personal_rns::shared_instance::rns_rpc::{
    reticulum_storage_dir, SharedInstanceCredentials, SharedInstanceRpcServer,
};
use personal_rns::shared_instance::SharedInstanceServer;
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::usb_auto::{
    open_native_usb_auto_target, scan_native_usb_auto_targets, NativeUsbAutoStream,
    UsbAutoCandidate, UsbAutoHost,
};
#[cfg(target_os = "macos")]
use personal_rns::wifi_auto::apple_service_discovery;
use personal_rns::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::wire::DestinationHash;
use tokio::sync::Notify;

use personal_hopspot_core::{
    self as screen, load_host_ble_identity, load_host_node_identity, CardKind,
    HopspotDestinationSet, IdentityPersistence, BLE_IDENTITY_STORAGE, NODE_IDENTITY_STORAGE,
};

use super::persistence;
use super::ui::run_window;

pub(super) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);

const USB_BAUD: u32 = 115_200;

const ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot (Desktop)";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot (Desktop)";

pub(super) fn classify(
    id: InterfaceId,
    wifi_id: InterfaceId,
    tcp_id: Option<InterfaceId>,
    tcp_target: Option<&str>,
) -> Option<(CardKind, screen::CardLabel)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, screen::card_label("USB")))
    } else if id == wifi_id {
        Some((CardKind::Wifi, screen::card_label("LAN")))
    } else if Some(id) == tcp_id {
        Some((
            CardKind::Tcp,
            screen::tcp_card_label(tcp_target.unwrap_or("")),
        ))
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        Some((CardKind::Ble, screen::card_label("BLE")))
    } else if id.kind() == Some(InterfaceKind::WifiDirect) {
        Some((CardKind::Wifi, screen::card_label("Direct")))
    } else {
        let bytes = id.as_bytes();
        let (kind, tag) = match id.kind() {
            Some(InterfaceKind::TcpClient | InterfaceKind::TcpServerPeer) => (CardKind::Tcp, "TCP"),
            Some(InterfaceKind::BluetoothPeer) => (CardKind::Ble, "BLE"),
            Some(InterfaceKind::WifiDirectPeer) => (CardKind::Wifi, "WD"),
            _ => (CardKind::Peer, "Peer"),
        };
        let mut label = screen::CardLabel::new();
        let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
        Some((kind, label))
    }
}

pub(super) struct WindowHandles {
    pub(super) handle: PrnsNodeHandle,
    pub(super) usb_status: TokioInterfaceStatus,
    pub(super) wifi_status: AutoWifiStatus,
    pub(super) ble_status: Option<BluetoothAutoStatus>,
    pub(super) tcp_status: Option<TokioInterfaceStatus>,
    pub(super) tcp_id: Option<InterfaceId>,
    pub(super) tcp_target: Option<String>,
    pub(super) node_page_destination: DestinationHash,
}

pub fn run() {
    init_observability();

    let (ready_tx, ready_rx) = mpsc::channel::<(WindowHandles, persistence::ShutdownFlush)>();
    std::thread::Builder::new()
        .name("hopspot-node".into())
        .spawn(move || run_node(ready_tx))
        .expect("spawn node thread");

    let (handles, shutdown_flush) = ready_rx
        .recv()
        .expect("the node hands the window its handles before the runtime runs");
    run_window(handles);
    shutdown_flush.flush_before_exit();
}

fn init_observability() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_log::LogTracer::init();
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer());
    let _ = subscriber.try_init();
}

fn log_identity_persistence(
    identity: &str,
    persistence: &IdentityPersistence<impl core::fmt::Display>,
) {
    match persistence {
        IdentityPersistence::Loaded => {}
        IdentityPersistence::Created => tracing::info!(event = "identity_created", identity),
        IdentityPersistence::Recovered(error) => {
            tracing::warn!(event = "identity_recovered", identity, %error)
        }
        IdentityPersistence::Ephemeral(error) => {
            tracing::error!(event = "identity_ephemeral", identity, %error)
        }
    }
}

fn run_node(ready_tx: Sender<(WindowHandles, persistence::ShutdownFlush)>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the node thread builds its tokio runtime");

    runtime.block_on(async move {
        let storage_dir = reticulum_storage_dir();
        let node_bootstrap =
            load_host_node_identity(&storage_dir.join(NODE_IDENTITY_STORAGE.as_str()));
        log_identity_persistence("node", node_bootstrap.persistence());
        let node_identity = node_bootstrap.into_identity();
        let transport_secret = node_identity.transport_secret();
        let credentials = SharedInstanceCredentials::from_identity_secret(node_identity.secret());
        let ble_bootstrap =
            load_host_ble_identity(&storage_dir.join(BLE_IDENTITY_STORAGE.as_str()));
        log_identity_persistence("bluetooth", ble_bootstrap.persistence());
        let ble_identity = ble_bootstrap.into_identity();

        let destination_secret = node_identity.into_destination_secret();
        let destinations = HopspotDestinationSet::new(
            destination_secret,
            ANNOUNCE_APP_DATA,
            NODE_ANNOUNCE_APP_DATA,
        );
        let destination_hashes = destinations
            .destination_hashes()
            .expect("the hopspot destination names are valid");
        let node_page_destination = destination_hashes.node_page;

        let persistence_store = match persistence::open(&storage_dir) {
            Ok(persistence_store) => Some(persistence_store),
            Err(error) => {
                tracing::error!(event = "persistence_unavailable", %error);
                None
            }
        };
        let (rotated_tx, rotated_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(transport_secret),
            pre_configured_destinations: destinations.into_preconfigured_destinations(),
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: screen::node_pages::NodePageRoutes,
            remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: move |event, _state: &()| {
                if let PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination }) = event
                {
                    let _ = rotated_tx.send(destination);
                }
            },
        });
        if let Some(persistence_store) = &persistence_store {
            node = node.with_timeline_origin(persistence_store.timeline_origin());
            let restored = persistence_store.restore(&mut node);
            tracing::info!(
                event = "persistence_restored",
                routes = restored.routes.seeded_count,
                destinations = restored.destination_identities.seeded_count,
                tunnels = restored.tunnels.seeded_count,
                ratchets = restored.ratchets.seeded_count,
                refused = restored.refused_total(),
                dropped = restored.dropped_total(),
            );
        }
        let handle = node.handle();
        let shutdown_flush = match persistence_store {
            Some(persistence_store) => {
                persistence::spawn_worker(persistence_store, handle.clone(), rotated_rx)
            }
            None => persistence::ShutdownFlush::disabled(),
        };

        let rescan = Arc::new(Notify::new());
        let usb = UsbAutoHost::new(
            USB_INTERFACE_ID,
            scan_native_usb_auto_targets,
            open_native_usb_auto_target_with_baud,
            rescan.clone(),
        );
        let usb_status = usb.status();
        handle.add_interface(usb);
        if std::env::var_os("HOPSPOT_USB_OFF").is_some() {
            usb_status.disable();
            tracing::info!(event = "usb_started_disabled");
        }

        #[cfg(target_os = "linux")]
        spawn_hotplug_watcher(rescan.clone());

        #[cfg(target_os = "windows")]
        {
            let rescan = rescan.clone();
            prns_ffi::usb_hotplug::watch_serial_hotplug(move || rescan.notify_one());
        }

        #[cfg(target_os = "macos")]
        let wifi = AutoWifi::default().with_host_discovery(apple_service_discovery());
        #[cfg(not(target_os = "macos"))]
        let wifi = AutoWifi::default();
        let wifi_status = wifi.status();
        if std::env::var_os("HOPSPOT_WIFI_OFF").is_some() {
            wifi_status.disable();
            tracing::info!(event = "wifi_started_disabled");
        }
        handle.supervise(wifi);

        let ble_status = Some(handle.attach(AutoBle::new(ble_identity)).status());

        handle.supervise(SharedInstanceServer::default());
        tracing::info!(
            event = "shared_instance_started",
            bus_port = instance_core::DEFAULT_LOCAL_PORT,
            control_port = instance_core::DEFAULT_LOCAL_PORT + 1,
        );

        let rpc_port = instance_core::DEFAULT_LOCAL_PORT + 1;

        let (tcp_status, tcp_id, tcp_target) = match std::env::var("HOPSPOT_TCP_TARGET") {
            Ok(target) if !target.is_empty() => {
                let tcp = TcpClientInterface::new_with_bitrate(
                    target.clone(),
                    tcp::TCP_BITRATE_ESTIMATE,
                    ReconnectPolicy::STANDARD,
                );
                let status = tcp.status();
                let id = tcp.id();
                handle.add_interface(tcp);
                (Some(status), Some(id), Some(target))
            }
            _ => (None, None, None),
        };

        let tcp_rpc =
            match SharedInstanceRpcServer::tcp(credentials.clone(), rpc_port, handle.clone())
                .bind()
                .await
            {
                Ok(rpc) => rpc,
                Err(error) => {
                    tracing::error!(
                        event = "shared_instance_rpc_bind_failed",
                        transport = "tcp",
                        error = ?error,
                    );
                    return;
                }
            };
        tokio::spawn(tcp_rpc.run());
        #[cfg(target_os = "linux")]
        {
            let abstract_unix_rpc = match SharedInstanceRpcServer::abstract_unix(
                credentials,
                instance_core::DEFAULT_SOCKET_PATH,
                handle.clone(),
            )
            .bind()
            .await
            {
                Ok(rpc) => rpc,
                Err(error) => {
                    tracing::error!(
                        event = "shared_instance_rpc_bind_failed",
                        transport = "abstract_unix",
                        error = ?error,
                    );
                    return;
                }
            };
            tokio::spawn(abstract_unix_rpc.run());
        }

        let _ = ready_tx.send((
            WindowHandles {
                handle: handle.clone(),
                usb_status,
                wifi_status,
                ble_status,
                tcp_status,
                tcp_id,
                tcp_target,
                node_page_destination,
            },
            shutdown_flush,
        ));

        match node.run().await {
            Ok(()) => tracing::error!(event = "node_stopped"),
            Err(error) => tracing::error!(event = "node_panic_shutdown", error = ?error),
        }
    });
}

async fn open_native_usb_auto_target_with_baud(
    candidate: UsbAutoCandidate,
) -> std::io::Result<NativeUsbAutoStream> {
    open_native_usb_auto_target(candidate, USB_BAUD).await
}

#[cfg(target_os = "linux")]
fn spawn_hotplug_watcher(rescan: Arc<Notify>) {
    let _ = std::thread::Builder::new()
        .name("hopspot-udev".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(watch_hotplug(rescan));
        });
}

#[cfg(target_os = "linux")]
async fn watch_hotplug(rescan: Arc<Notify>) {
    use tokio_stream::StreamExt;

    let listener = tokio_udev::MonitorBuilder::new()
        .and_then(|builder| builder.match_subsystem("tty"))
        .and_then(|builder| builder.listen());
    let Ok(listener) = listener else {
        return;
    };
    let Ok(mut events) = tokio_udev::AsyncMonitorSocket::new(listener) else {
        return;
    };
    while let Some(event) = events.next().await {
        if event.is_ok() {
            rescan.notify_one();
        }
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn every_desktop_interface_label_fits_its_card() {
        let wifi_id = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"wifi");
        let tcp_id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"tcp");
        let ids = [
            USB_INTERFACE_ID,
            wifi_id,
            tcp_id,
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"ble"),
            InterfaceId::from_channel_tag(InterfaceKind::WifiDirect, b"direct"),
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"ble-peer"),
            InterfaceId::from_channel_tag(InterfaceKind::WifiDirectPeer, b"direct-peer"),
        ];
        for id in ids {
            let (kind, label) = classify(id, wifi_id, Some(tcp_id), Some("rns.michmesh.net"))
                .expect("known interface has a card");
            assert!(
                label.chars().count() <= screen::card_label_max_chars(kind),
                "{label:?} exceeds its card"
            );
        }
    }
}
