//! The desktop face of the Personal Hopspot — one of the app's two targets.
//!
//! Runs the *same* Hopspot engine the Heltec V4 firmware does, here on the high-level [`Prns`]
//! runtime: the recipe stands up the `lxmf.delivery` destination and the transport role, then the
//! app attaches the plug-and-play USB-auto interface (every Personal board on a CDC port) and
//! supervises the WiFi/LAN auto-interface (every Personal node on the same WiFi). It renders the
//! *same* Hopspot status screen the OLED shows — a card per interface, the WiFi supervisor's
//! aggregate plus a card per peer it stands up — in an `embedded-graphics-simulator` window. Run
//! `cargo run -p hopspot`, plug in a board or join a peer, and watch the cards tick as traffic crosses.
//!
//! The node runs on its own thread inside a tokio runtime; the SDL2 window owns the main thread
//! (SDL requires it) and repaints the interfaces' live status handles at ~30 fps — read straight
//! off each interface, never laundered through the engine.

use core::fmt::Write as _;
use std::collections::HashMap;
use std::io;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::Mutex;
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettings, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent,
};
use heapless::Vec as HVec;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use personal_rns::ble::tokio::BluetoothAutoStatus;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::lora::core::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::interfaces::shared_instance::core as instance_core;
use personal_rns::interfaces::tcp::core as tcp_core;
#[cfg(target_os = "macos")]
use personal_rns::interfaces::wifi_auto::core as wifi_core;
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::prelude::*;
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::ProofStrategy;
use personal_rns::shared_instance::rpc_compat::{
    reticulum_storage_dir, rpc_key_from_rns_identity, SharedInstanceRpcCompat,
};
use personal_rns::shared_instance::server::LocalServer;
use personal_rns::storage::{GrowableHeap, StorageLayout};
use personal_rns::tcp::client::TcpClientInterface;
use personal_rns::usb::UsbAutoHost;
use personal_rns::wifi::{AutoWifi, AutoWifiStatus};
use personal_rns::wire::{DestinationHash, TransportId};
use personal_rns::{interfaces, routes};
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use tokio::sync::Notify;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::host_usb::{open_usb_auto_target, scan_usb_auto_targets, HostUsb};
use personal_hopspot_core::{self as screen, Card, CardKind, InputEvent, UiAction, UiState};

/// Stable id for this node's USB-auto interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);

/// CDC-ACM nominal baud (USB ignores it, but the port builder wants a value).
const USB_BAUD: u32 = 115_200;

/// The destination this node announces itself as: `lxmf.delivery`, the aspect LXMF apps
/// message — so the hopspot surfaces as a real, messageable peer.
const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence — keeps the window responsive and re-reads the live status each frame.
const FRAME: Duration = Duration::from_millis(screen::COALESCE_MS);
const LIVE_REFRESH: Duration = Duration::from_millis(250);
const STATUS_LOG_THROTTLE: Duration = Duration::from_millis(1000);
const NOTICE_TIMEOUT: Duration = Duration::from_millis(900);
/// Presses at or above this duration enter the long-press path.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(500);

/// This node's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519 private
/// keys). Handed to the recipe through a [`Zeroizing`] buffer so it is wiped from this stack
/// frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    if cfg!(feature = "fixture-identity") {
        // Deterministic bring-up / HITL identity: the X25519 0x22 ‖ Ed25519 0x11
        // keypair the oracle vectors pin. Never ship this — every fixture-identity
        // node shares one identity.
        let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
        key[..32].fill(0x22);
        key[32..].fill(0x11);
        return key;
    }
    personal_rns::runtime::generate_identity_secret()
}

/// The card icon and label for an interface id: the fixed USB interface, the WiFi supervisor's
/// aggregate, an explicit TCP client, or one of its fleet members — a direct WiFi LAN peer
/// (`Peer 1a2b`) or an auto-wifi TCP-fold rendezvous link (`TCP 1a2b`, the TCP icon setting it apart
/// from a WiFi peer at a glance). Returning `None` would drop the card; every id we render maps, the
/// medium-derived hex tag telling two of a kind apart.
fn classify(
    id: InterfaceId,
    wifi_id: InterfaceId,
    tcp_id: Option<InterfaceId>,
    tcp_target: Option<&str>,
) -> Option<(CardKind, screen::CardLabel)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, screen::card_label("USB")))
    } else if id == wifi_id {
        Some((CardKind::Wifi, screen::card_label("WiFi/LAN")))
    } else if Some(id) == tcp_id {
        Some((
            CardKind::Tcp,
            screen::tcp_card_label(tcp_target.unwrap_or("")),
        ))
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        Some((CardKind::Ble, screen::card_label("BLE")))
    } else {
        let bytes = id.as_bytes();
        let (kind, tag) = match id.kind() {
            Some(InterfaceKind::TcpClient | InterfaceKind::TcpServerPeer) => (CardKind::Tcp, "TCP"),
            Some(InterfaceKind::BluetoothPeer) => (CardKind::Ble, "BLE"),
            _ => (CardKind::Peer, "Peer"),
        };
        let mut label = screen::CardLabel::new();
        let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
        Some((kind, label))
    }
}

/// What the node thread hands the window once the runtime is assembled: a command handle — which
/// issues announces and yields the whole fleet's status snapshots from the runtime's central
/// registry, the cards the window draws — the interface status handles it toggles enabled (USB and
/// the optional TCP client) and reads the WiFi supervisor's id from, and the destination its
/// announces name. The node owns everything else.
struct WindowHandles {
    handle: TokioPrnsHandle,
    usb_status: TokioInterfaceStatus,
    wifi_status: AutoWifiStatus,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    ble_status: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    ble_enabled: Arc<AtomicBool>,
    tcp_status: Option<TokioInterfaceStatus>,
    tcp_id: Option<InterfaceId>,
    tcp_target: Option<String>,
    destination: DestinationHash,
}

/// Spawn the node on its own thread, then own the SDL2 window on this (the main) thread: SDL
/// requires it. The node thread hands the window its command handle and status handles back
/// before the runtime starts running.
pub fn run() {
    // Surface the host backends' `log` diagnostics (notably the BLE lifecycle traces) when the
    // operator opts in via RUST_LOG; silent by default, so the normal run is unchanged. try_init
    // never panics if a logger is already installed.
    let _ = env_logger::try_init();

    let identity_secret_key = load_identity_secret_key();

    let (ready_tx, ready_rx) = mpsc::channel::<WindowHandles>();
    std::thread::Builder::new()
        .name("hopspot-node".into())
        .spawn(move || run_node(ready_tx, identity_secret_key))
        .expect("spawn node thread");

    let handles = ready_rx
        .recv()
        .expect("the node hands the window its handles before the runtime runs");
    run_window(handles);
}

/// Stand up the native CoreBluetooth BLE auto-interface as a supervised fleet, on its own task so a
/// slow or denied radio never blocks the node coming up. `MacosBleBackend::new` awaits power-on and
/// the L2CAP publish; on failure (most often Bluetooth not granted to this binary) it logs and the
/// node runs without BLE. The supervisor's id is medium-stable to the node identity, so the Hopspot
/// renders it as one "BLE" card the same way the Android face does.
#[cfg(target_os = "macos")]
fn spawn_bluetooth(
    handle: TokioPrnsHandle,
    identity_hash: [u8; 16],
    status_slot: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    desired_enabled: Arc<AtomicBool>,
) {
    use personal_rns::ble::tokio::BluetoothAuto;
    use personal_rns::interfaces::bluetooth_auto::core::{
        AppleHost, BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
    use prns_ffi::ble::macos::MacosBleBackend;

    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match MacosBleBackend::new().await {
            Ok(backend) => {
                let psm = backend.psm();
                let bluetooth = BluetoothAuto::<_, { MacosBleBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::MacOs),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                let status = bluetooth.status();
                status.set_enabled(desired_enabled.load(Ordering::Relaxed));
                if let Ok(mut slot) = status_slot.lock() {
                    *slot = Some(status);
                }
                handle.supervise(bluetooth);
                println!(
                    "bluetooth: supervising CoreBluetooth, L2CAP psm {:#06x}",
                    psm.get()
                );
            }
            Err(error) => {
                eprintln!(
                    "bluetooth disabled ({error:?}); grant Bluetooth in System Settings > Privacy & Security > Bluetooth"
                );
            }
        }
    });
}

/// Stand up the native WinRT BLE auto-interface as a supervised fleet, mirroring the macOS path on
/// its own task so a slow or off radio never blocks the node coming up. Windows is GATT-only (no
/// app-level L2CAP), so `LinkCapabilities` advertises no PSM; the GATT-data floor carries every
/// frame. `WindowsBleBackend::new` brings the adapter up; on failure (no radio, radio off, or the
/// peripheral role unsupported) it logs and the node runs without BLE.
#[cfg(target_os = "windows")]
fn spawn_bluetooth(
    handle: TokioPrnsHandle,
    identity_hash: [u8; 16],
    status_slot: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    desired_enabled: Arc<AtomicBool>,
) {
    use personal_rns::ble::tokio::BluetoothAuto;
    use personal_rns::interfaces::bluetooth_auto::core::{
        BleIdentity, Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;
    use prns_ffi::ble::windows::WindowsBleBackend;

    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match WindowsBleBackend::new().await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { WindowsBleBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::WinRt(WinRtHost::Windows),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                let status = bluetooth.status();
                status.set_enabled(desired_enabled.load(Ordering::Relaxed));
                if let Ok(mut slot) = status_slot.lock() {
                    *slot = Some(status);
                }
                handle.supervise(bluetooth);
                println!("bluetooth: supervising WinRT (GATT-only)");
            }
            Err(error) => {
                eprintln!(
                    "bluetooth disabled ({error:?}); check that Bluetooth is on and supported on this machine"
                );
            }
        }
    });
}

/// Stand up the native BlueZ BLE auto-interface on Linux using BlueR. Linux is the reference BLE host
/// backend: it advertises the shared Reticulum service, scans for peers, and uses the LE CoC PSM when
/// available while keeping the same aggregate "BLE" card the other desktop faces render.
#[cfg(target_os = "linux")]
fn spawn_bluetooth(
    handle: TokioPrnsHandle,
    identity_hash: [u8; 16],
    status_slot: Arc<Mutex<Option<BluetoothAutoStatus>>>,
    desired_enabled: Arc<AtomicBool>,
) {
    use personal_rns::ble::bluer::BluerBackend;
    use personal_rns::ble::tokio::BluetoothAuto;
    use personal_rns::interfaces::bluetooth_auto::core::{
        BleIdentity, BlueZHost, Endpoint, LinkCapabilities, Psm, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::seam::BleBackend;

    const CONTROL_PSM: u16 = 0x0083;

    let Some(psm) = Psm::new(CONTROL_PSM) else {
        eprintln!("bluetooth disabled: invalid Linux control PSM {CONTROL_PSM:#x}");
        return;
    };
    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match BluerBackend::open(psm).await {
            Ok(backend) => {
                let bluetooth = BluetoothAuto::<_, { BluerBackend::MAX_PEERS }>::new(
                    backend,
                    ble_identity,
                    Endpoint::BlueZ(BlueZHost::Linux),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                );
                let status = bluetooth.status();
                status.set_enabled(desired_enabled.load(Ordering::Relaxed));
                if let Ok(mut slot) = status_slot.lock() {
                    *slot = Some(status);
                }
                handle.supervise(bluetooth);
                println!("bluetooth: supervising BlueZ/BlueR, control psm {CONTROL_PSM:#x}");
            }
            Err(error) => {
                eprintln!(
                    "bluetooth disabled ({error:?}); check bluetoothd, adapter power, and BlueZ LE advertising/GATT support"
                );
            }
        }
    });
}

/// Advertise this node's WiFi/LAN rendezvous over Bonjour and feed every resolved peer into the
/// AutoWifi supervisor's mDNS channel, so the Mac discovers and is discovered by peers that cannot
/// run raw multicast (iOS) — both ends now speak the same WiFi/LAN protocol. Standard Bonjour rides
/// the system mDNSResponder, so this needs only Local Network permission, never the multicast
/// entitlement. On its own task so a slow or denied responder never blocks the node coming up.
#[cfg(target_os = "macos")]
fn spawn_mdns(port: u16, sightings: tokio::sync::mpsc::UnboundedSender<std::net::SocketAddr>) {
    use prns_ffi::mdns::macos::MacosMdnsBackend;

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
                eprintln!("mdns: failed ({error:?}); grant Local Network in System Settings > Privacy & Security");
            }
        }
    });
}

fn run_node(
    ready_tx: Sender<WindowHandles>,
    identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the node thread builds its tokio runtime");

    runtime.block_on(async move {
        let identity_hash = {
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&identity_secret_key);
            *signer.identity_hash().as_bytes()
        };
        let transport_id = TransportId::new(identity_hash);
        let rpc_key = rpc_key_from_rns_identity(&reticulum_storage_dir(), &identity_secret_key[..]);

        let announce_destination = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: ANNOUNCE_APP_NAME,
            aspects: ANNOUNCE_ASPECTS,
            identity: identity_secret_key,
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
            on_event: |event, _state: &()| log_event(event),
        });
        let handle = node.handle();

        let rescan = Arc::new(Notify::new());
        let usb = UsbAutoHost::new(
            USB_INTERFACE_ID,
            scan_usb_auto_targets,
            open_usb_auto_target_with_baud,
            rescan.clone(),
        );
        let usb_status = usb.status();
        handle.add_interface(usb);
        if std::env::var_os("HOPSPOT_USB_OFF").is_some() {
            usb_status.set_enabled(false);
            println!("usb: added but starting disabled (HOPSPOT_USB_OFF set)");
        }

        #[cfg(target_os = "linux")]
        spawn_hotplug_watcher(rescan.clone());

        #[cfg(target_os = "windows")]
        {
            let rescan = rescan.clone();
            prns_ffi::usb_hotplug::watch_serial_hotplug(move || rescan.notify_one());
        }

        #[cfg(target_os = "macos")]
        let (mdns_tx, mdns_rx) =
            tokio::sync::mpsc::unbounded_channel::<std::net::SocketAddr>();
        #[cfg(target_os = "macos")]
        let wifi = AutoWifi::default().with_mdns(mdns_rx);
        #[cfg(not(target_os = "macos"))]
        let wifi = AutoWifi::default();
        let wifi_status = wifi.status();
        if std::env::var_os("HOPSPOT_WIFI_OFF").is_some() {
            wifi_status.set_enabled(false);
            println!("wifi: added but starting disabled (HOPSPOT_WIFI_OFF set)");
        } else {
            #[cfg(target_os = "macos")]
            spawn_mdns(wifi_core::TCP_RENDEZVOUS_PORT, mdns_tx);
        }
        handle.supervise(wifi);

        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let ble_status = Arc::new(Mutex::new(None));
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let ble_enabled = Arc::new(AtomicBool::new(true));
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        spawn_bluetooth(
            handle.clone(),
            identity_hash,
            ble_status.clone(),
            ble_enabled.clone(),
        );

        handle.supervise(LocalServer::default());
        println!(
            "shared instance: local RNS apps (Sideband, NomadNet, MeshChat) can connect on 127.0.0.1:{}",
            instance_core::DEFAULT_LOCAL_PORT
        );

        let rpc_port = instance_core::DEFAULT_LOCAL_PORT + 1;
        println!(
            "  attachments to local RNS clients flow over the shared Reticulum identity (rpc_key {})",
            rpc_key
                .iter()
                .map(|byte| std::format!("{byte:02x}"))
                .collect::<String>()
        );

        let (tcp_status, tcp_id, tcp_target) = match std::env::var("HOPSPOT_TCP_TARGET") {
            Ok(target) if !target.is_empty() => {
                let tcp = TcpClientInterface::new(
                    target.clone(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                    Duration::from_secs(5),
                );
                let status = tcp.status();
                let id = tcp.id();
                handle.add_interface(tcp);
                (Some(status), Some(id), Some(target))
            }
            _ => (None, None, None),
        };

        {
            let snapshot_handle = handle.clone();
            tokio::spawn(
                SharedInstanceRpcCompat::tcp(rpc_key, rpc_port, handle.clone())
                    .with_interfaces(move || snapshot_handle.interface_vitals())
                    .run(),
            );
        }
        #[cfg(target_os = "linux")]
        {
            let snapshot_handle = handle.clone();
            tokio::spawn(
                SharedInstanceRpcCompat::abstract_unix(
                    rpc_key,
                    instance_core::DEFAULT_SOCKET_PATH,
                    handle.clone(),
                )
                .with_interfaces(move || snapshot_handle.interface_vitals())
                .run(),
            );
        }

        let _ = ready_tx.send(WindowHandles {
            handle: handle.clone(),
            usb_status,
            wifi_status,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            ble_status,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            ble_enabled,
            tcp_status,
            tcp_id,
            tcp_target,
            destination,
        });

        node.run().await;
    });
}

/// Open one USB Auto target into an async stream. The target may be a CDC port, a Prns vendor
/// bulk device, or an Android Open Accessory stream; the shared host still sees one byte pipe.
async fn open_usb_auto_target_with_baud(name: String) -> io::Result<HostUsb> {
    open_usb_auto_target(name, USB_BAUD).await
}

/// Watch the OS for serial-device hot-plug and poke the host's rescan signal on each event, so a
/// board appears the instant it is plugged in rather than on the host's fallback scan. Linux
/// only (udev); on other hosts the fallback timer carries discovery on its own.
///
/// The udev monitor holds non-`Send` handles, so it rides its own thread with a current-thread
/// runtime — `block_on` keeps it off the multi-thread reactor while the cross-thread `Notify`
/// pokes the host.
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

/// What the desktop observes — `Prns::run` maps the engine's `Journaled` stream into a curated
/// [`PrnsEvent`], split into the data plane ([`Message`]) and observability ([`Diagnostic`]).
fn log_event(event: PrnsEvent<'_>) {
    match event {
        PrnsEvent::Message(message) => log_message(message),
        PrnsEvent::Diagnostic(diagnostic) => log_diagnostic(diagnostic),
    }
}

fn log_message(message: Message<'_>) {
    match message {
        Message::Delivered(Delivery::Plain(delivery)) => println!(
            "HOPSPOT_RX_DELIVERY kind=plain destination={:02x?} bytes={}",
            delivery.destination.as_bytes(),
            delivery.payload.len(),
        ),
        Message::Delivered(Delivery::Single(delivery)) => println!(
            "HOPSPOT_RX_DELIVERY kind=single destination={:02x?} bytes={}",
            delivery.destination.as_bytes(),
            delivery.plaintext.len(),
        ),
        Message::Delivered(Delivery::Group(delivery)) => println!(
            "HOPSPOT_RX_DELIVERY kind=group destination={:02x?} bytes={}",
            delivery.destination.as_bytes(),
            delivery.plaintext.len(),
        ),
        Message::Delivered(Delivery::Link(delivery)) => println!(
            "HOPSPOT_RX_DELIVERY kind=link link_id={:02x?} bytes={}",
            delivery.link_id.as_bytes(),
            delivery.plaintext.len(),
        ),
        Message::Request {
            link_id,
            request_id,
            data,
            ..
        } => println!(
            "HOPSPOT_REQUEST_RECEIVED link_id={:02x?} request_id={:02x?} bytes={}",
            link_id.as_bytes(),
            request_id.as_bytes(),
            data.len(),
        ),
        Message::Response {
            link_id,
            request_id,
            data,
        } => println!(
            "HOPSPOT_RESPONSE_RECEIVED link_id={:02x?} request_id={:02x?} bytes={}",
            link_id.as_bytes(),
            request_id.as_bytes(),
            data.len(),
        ),
        Message::Resource {
            link_id,
            hash,
            data,
        } => println!(
            "HOPSPOT_RX_RESOURCE link_id={:02x?} hash={:02x?} bytes={}",
            link_id.as_bytes(),
            hash.as_bytes(),
            data.len(),
        ),
        Message::ResourceNeedsDecompression {
            link_id,
            hash,
            stream,
            uncompressed_data_len,
        } => println!(
            "HOPSPOT_RX_RESOURCE_COMPRESSED link_id={:02x?} hash={:02x?} stream_bytes={} uncompressed_bytes={}",
            link_id.as_bytes(),
            hash.as_bytes(),
            stream.len(),
            uncompressed_data_len,
        ),
        Message::ResourceSegment {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            data,
        } => println!(
            "HOPSPOT_RX_RESOURCE_SEGMENT link_id={:02x?} original_hash={:02x?} segment={segment_index}/{total_segments} bytes={}",
            link_id.as_bytes(),
            original_hash.as_bytes(),
            data.len(),
        ),
        Message::ChannelMessage {
            link_id,
            message_type,
            data,
        } => println!(
            "HOPSPOT_RX_CHANNEL link_id={:02x?} type={message_type:?} bytes={}",
            link_id.as_bytes(),
            data.len(),
        ),
    }
}

fn log_diagnostic(diagnostic: Diagnostic) {
    match diagnostic {
        Diagnostic::AnnounceHeard {
            destination,
            hops,
            source_interface,
        } => println!(
            "HOPSPOT_ANNOUNCE_HEARD destination={:02x?} hops={hops} interface={:02x?}",
            destination.as_bytes(),
            source_interface.as_bytes(),
        ),
        Diagnostic::CommandSettled { id, settlement } => {
            println!("HOPSPOT_COMMAND_SETTLED id={} {settlement:?}", id.0);
        }
        Diagnostic::LinkEstablished(established) => println!(
            "HOPSPOT_LINK_ESTABLISHED link_id={:02x?} rtt_ms={}",
            established.link_id.as_bytes(),
            established.rtt_ms,
        ),
        Diagnostic::PeerIdentified { link_id, identity } => println!(
            "HOPSPOT_PEER_IDENTIFIED link_id={:02x?} identity={:02x?}",
            link_id.as_bytes(),
            identity.as_bytes(),
        ),
        Diagnostic::LinkClosed { link_id, reason } => println!(
            "HOPSPOT_LINK_CLOSED link_id={:02x?} reason={reason:?}",
            link_id.as_bytes(),
        ),
        Diagnostic::ResourceFailed { link_id, hash } => println!(
            "HOPSPOT_RESOURCE_FAILED link_id={:02x?} hash={:02x?}",
            link_id.as_bytes(),
            hash.as_bytes(),
        ),
        Diagnostic::RouteExpired { destination } => println!(
            "HOPSPOT_ROUTE_EXPIRED destination={:02x?}",
            destination.as_bytes(),
        ),
        Diagnostic::RouteEvicted { destination } => println!(
            "HOPSPOT_ROUTE_EVICTED destination={:02x?}",
            destination.as_bytes(),
        ),
        Diagnostic::RouteInterfaceGone { destination } => println!(
            "HOPSPOT_ROUTE_INTERFACE_GONE destination={:02x?}",
            destination.as_bytes(),
        ),
        Diagnostic::ResourceAssembled {
            link_id,
            original_hash,
            total_size,
        } => println!(
            "HOPSPOT_RESOURCE_ASSEMBLED link_id={:02x?} original_hash={:02x?} total_size={total_size}",
            link_id.as_bytes(),
            original_hash.as_bytes(),
        ),
        Diagnostic::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => println!(
            "HOPSPOT_LINK_INTERFACE_MISMATCH link_id={:02x?} attached={:02x?} arrived_on={:02x?}",
            link_id.as_bytes(),
            attached_interface.as_bytes(),
            arrived_on.as_bytes(),
        ),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PressSource {
    Key,
    Mouse,
}

#[derive(Clone, Copy)]
struct PressStart {
    source: PressSource,
    started_at: Instant,
    long_press_sent: bool,
}

fn press_start(source: PressSource) -> PressStart {
    PressStart {
        source,
        started_at: Instant::now(),
        long_press_sent: false,
    }
}

fn dispatch_long_press_if_ready(
    active_press: &mut Option<PressStart>,
    now: Instant,
    card_count: usize,
    has_footer: bool,
    selected_kind: Option<CardKind>,
    ui_state: &mut UiState,
) -> UiAction {
    let Some(press) = active_press.as_mut() else {
        return UiAction::None;
    };
    if card_count == 0
        || press.long_press_sent
        || now.duration_since(press.started_at) < LONG_PRESS_THRESHOLD
    {
        return UiAction::None;
    }

    press.long_press_sent = true;
    ui_state.handle_input_with_footer(InputEvent::LongPress, card_count, has_footer, selected_kind)
}

fn finish_press(
    active_press: &mut Option<PressStart>,
    source: PressSource,
    released_at: Instant,
    card_count: usize,
    has_footer: bool,
    selected_kind: Option<CardKind>,
    ui_state: &mut UiState,
) -> UiAction {
    let Some(press) = active_press.take() else {
        return UiAction::None;
    };
    if press.source != source {
        *active_press = Some(press);
        return UiAction::None;
    }

    if press.long_press_sent {
        return UiAction::None;
    }

    let event = if released_at.duration_since(press.started_at) >= LONG_PRESS_THRESHOLD {
        InputEvent::LongPress
    } else {
        InputEvent::ShortPress
    };
    ui_state.handle_input_with_footer(event, card_count, has_footer, selected_kind)
}

/// The interface id of the currently focused card, if any — what a "Turn Off/On" toggle acts on.
fn selected_card_id(ui_state: &UiState, card_count: usize, cards: &[Card]) -> Option<InterfaceId> {
    ui_state
        .selected_card(card_count)
        .and_then(|index| cards.get(index))
        .map(|card| card.id)
}

fn selected_card_kind(ui_state: &UiState, card_count: usize, cards: &[Card]) -> Option<CardKind> {
    ui_state
        .selected_card(card_count)
        .and_then(|index| cards.get(index))
        .map(|card| card.kind)
}

struct LoggedStatus {
    connection: ConnectionState,
    rx_bytes: u64,
    tx_bytes: u64,
    last_emit: Instant,
}

enum DesktopControl {
    ShowWindow,
    HideWindow,
    Announce,
    Quit,
}

struct TrayController {
    icon: TrayIcon,
    window_item: MenuItem,
    announce_item: MenuItem,
    quit_item: MenuItem,
    window_open: bool,
}

impl TrayController {
    fn new(window_open: bool) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        gtk::init().map_err(|error| format!("gtk init failed: {error}"))?;

        let window_item = MenuItem::with_id(
            "hopspot-window",
            if window_open {
                "Hide Hopspot"
            } else {
                "Open Hopspot"
            },
            true,
            None,
        );
        let announce_item = MenuItem::with_id("hopspot-announce", "Announce Now", true, None);
        let quit_item = MenuItem::with_id("hopspot-quit", "Quit Hopspot", true, None);
        let separator = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[&window_item, &announce_item, &separator, &quit_item])
            .map_err(|error| format!("tray menu build failed: {error}"))?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Personal Hopspot is running")
            .with_icon(hopspot_tray_icon()?)
            .with_menu_on_left_click(true)
            .build()
            .map_err(|error| format!("tray icon build failed: {error}"))?;

        Ok(Self {
            icon,
            window_item,
            announce_item,
            quit_item,
            window_open,
        })
    }

    fn set_window_open(&mut self, open: bool) {
        if self.window_open == open {
            return;
        }
        self.window_open = open;
        self.window_item
            .set_text(if open { "Hide Hopspot" } else { "Open Hopspot" });
    }

    fn drain_controls(&mut self, window_open: bool) -> Vec<DesktopControl> {
        self.set_window_open(window_open);
        pump_tray_platform_events();

        let mut controls = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == self.window_item.id() {
                controls.push(if self.window_open {
                    DesktopControl::HideWindow
                } else {
                    DesktopControl::ShowWindow
                });
            } else if id == self.announce_item.id() {
                controls.push(DesktopControl::Announce);
            } else if id == self.quit_item.id() {
                controls.push(DesktopControl::Quit);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    id,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } if id == *self.icon.id() => controls.push(DesktopControl::ShowWindow),
                TrayIconEvent::DoubleClick {
                    id,
                    button: MouseButton::Left,
                    ..
                } if id == *self.icon.id() => controls.push(DesktopControl::ShowWindow),
                _ => {}
            }
        }
        controls
    }
}

#[cfg(target_os = "linux")]
fn pump_tray_platform_events() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
fn pump_tray_platform_events() {}

fn hopspot_tray_icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let idx = ((y * SIZE + x) * 4) as usize;
            let pixel = &mut rgba[idx..idx + 4];
            let h_left = (10..=13).contains(&x) && (9..=23).contains(&y);
            let h_right = (19..=22).contains(&x) && (9..=23).contains(&y);
            let h_crossbar = (10..=22).contains(&x) && (15..=17).contains(&y);
            if (11.5..=14.0).contains(&distance) {
                pixel.copy_from_slice(&[44, 232, 178, 255]);
            } else if distance < 10.0 && (h_left || h_right || h_crossbar) {
                pixel.copy_from_slice(&[230, 255, 248, 255]);
            } else if distance < 12.0 {
                pixel.copy_from_slice(&[7, 20, 28, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|error| format!("tray icon pixels invalid: {error}"))
}

struct HopspotWindow {
    output: OutputSettings,
    canvas: sdl2::render::Canvas<sdl2::video::Window>,
    event_pump: sdl2::EventPump,
    visible: bool,
    _sdl: sdl2::Sdl,
}

impl HopspotWindow {
    fn new(title: &str, output: &OutputSettings, display: &SimulatorDisplay<BinaryColor>) -> Self {
        let sdl = sdl2::init().expect("SDL initializes");
        let video = sdl.video().expect("SDL video subsystem initializes");
        let size = display.output_size(output);
        let window = video
            .window(title, size.width, size.height)
            .position_centered()
            .build()
            .expect("SDL creates the Hopspot window");
        let canvas = window
            .into_canvas()
            .build()
            .expect("SDL creates the Hopspot canvas");
        let event_pump = sdl.event_pump().expect("SDL event pump initializes");
        let mut window = Self {
            output: output.clone(),
            canvas,
            event_pump,
            visible: true,
            _sdl: sdl,
        };
        window.update(display);
        window
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn show(&mut self) {
        if self.visible {
            self.canvas.window_mut().raise();
            return;
        }
        self.visible = true;
        self.canvas.window_mut().show();
        self.canvas.window_mut().restore();
        self.canvas.window_mut().raise();
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.canvas.window_mut().hide();
    }

    fn update(&mut self, display: &SimulatorDisplay<BinaryColor>) {
        if !self.visible {
            return;
        }
        let output = display.to_rgb_output_image(&self.output);
        let image = output.as_image_buffer();
        let size = display.output_size(&self.output);
        let creator = self.canvas.texture_creator();
        let mut texture = creator
            .create_texture_streaming(PixelFormatEnum::RGB24, size.width, size.height)
            .expect("SDL creates the Hopspot texture");
        texture
            .update(None, image.as_raw(), size.width as usize * 3)
            .expect("SDL updates the Hopspot texture");
        self.canvas
            .copy(&texture, None, None)
            .expect("SDL copies the Hopspot texture");
        self.canvas.present();
    }

    fn events(&mut self) -> Vec<SimulatorEvent> {
        let mut events = Vec::new();
        let output = self.output;
        let output_to_display = |x, y| {
            let pitch = output.scale.saturating_add(output.pixel_spacing) as i32;
            Point::new(x / pitch.max(1), y / pitch.max(1))
        };
        while let Some(event) = self.event_pump.poll_event() {
            match event {
                Event::Quit { .. }
                | Event::Window {
                    win_event: WindowEvent::Close,
                    ..
                }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => events.push(SimulatorEvent::Quit),
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } if self.visible => events.push(SimulatorEvent::KeyDown {
                    keycode,
                    keymod,
                    repeat,
                }),
                Event::KeyUp {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } if self.visible => events.push(SimulatorEvent::KeyUp {
                    keycode,
                    keymod,
                    repeat,
                }),
                Event::MouseButtonDown {
                    x, y, mouse_btn, ..
                } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseButtonDown { mouse_btn, point });
                }
                Event::MouseButtonUp {
                    x, y, mouse_btn, ..
                } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseButtonUp { mouse_btn, point });
                }
                Event::MouseWheel {
                    x, y, direction, ..
                } if self.visible => events.push(SimulatorEvent::MouseWheel {
                    scroll_delta: Point::new(x, y),
                    direction,
                }),
                Event::MouseMotion { x, y, .. } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseMove { point });
                }
                _ => {}
            }
        }
        events
    }
}

/// Own the SDL2 window: repaint the interfaces' live status as the Hopspot screen until the
/// window is closed, and funnel the menu's "Announce" item into the node's command handle.
fn run_window(handles: WindowHandles) {
    let handle = handles.handle;
    let usb_status = handles.usb_status;
    let wifi_status = handles.wifi_status;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let ble_status = handles.ble_status;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let ble_enabled = handles.ble_enabled;
    let tcp_status = handles.tcp_status;
    let tcp_id = handles.tcp_id;
    let tcp_target = handles.tcp_target;
    let destination = handles.destination;

    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);
    let mut window = HopspotWindow::new("Personal Hopspot", &output, &display);
    let mut tray = match TrayController::new(true) {
        Ok(tray) => {
            println!("tray: Personal Hopspot stays running when the window is closed");
            Some(tray)
        }
        Err(error) => {
            eprintln!("tray: disabled ({error}); closing the window quits Hopspot");
            None
        }
    };

    let wifi_id = wifi_status.id();
    let tcp_target = tcp_target.as_deref();
    let classify = move |id: InterfaceId| classify(id, wifi_id, tcp_id, tcp_target);

    let query_handle = handle.clone();

    let toggle_usb = usb_status.clone();
    let toggle_wifi = wifi_status.clone();
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let toggle_ble = ble_status.clone();
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let toggle_ble_enabled = ble_enabled.clone();
    let toggle_tcp = tcp_status.clone();
    let apply_action = move |action: UiAction,
                             selected_id: Option<InterfaceId>,
                             ui_state: &mut UiState,
                             working_lora_profile: &mut RadioProfile,
                             notice_until: &mut Option<Instant>| match action
    {
        UiAction::None => {}
        UiAction::OledOff => {
            ui_state.show_notice(screen::UiNotice::OledOff);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
        }
        UiAction::Sleep => {
            ui_state.show_notice(screen::UiNotice::Sleeping);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            toggle_usb.set_enabled(false);
            toggle_wifi.set_enabled(false);
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                toggle_ble_enabled.store(false, Ordering::Relaxed);
                if let Ok(slot) = toggle_ble.lock() {
                    if let Some(ble) = slot.as_ref() {
                        ble.set_enabled(false);
                    }
                }
            }
            if let Some(tcp) = &toggle_tcp {
                tcp.set_enabled(false);
            }
        }
        UiAction::Wake => {
            ui_state.show_notice(screen::UiNotice::Awake);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            toggle_usb.set_enabled(true);
            toggle_wifi.set_enabled(true);
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                toggle_ble_enabled.store(true, Ordering::Relaxed);
                if let Ok(slot) = toggle_ble.lock() {
                    if let Some(ble) = slot.as_ref() {
                        ble.set_enabled(true);
                    }
                }
            }
            if let Some(tcp) = &toggle_tcp {
                tcp.set_enabled(true);
            }
        }
        UiAction::Announce => {
            ui_state.show_notice(screen::UiNotice::Announcing);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            if let Some(id) = handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })) {
                println!(
                    "HOPSPOT_TX_ANNOUNCE_NOW id={} kind=manual destination={:02x?}",
                    id.0,
                    destination.as_bytes(),
                );
            }
        }
        UiAction::ToggleSelectedInterface => match selected_id {
            Some(id) if id == USB_INTERFACE_ID => {
                ui_state.show_notice(if toggle_usb.is_enabled() {
                    screen::UiNotice::TurningOff
                } else {
                    screen::UiNotice::TurningOn
                });
                *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                toggle_usb.set_enabled(!toggle_usb.is_enabled());
            }
            Some(id) if id == toggle_wifi.id() => {
                ui_state.show_notice(if toggle_wifi.is_enabled() {
                    screen::UiNotice::TurningOff
                } else {
                    screen::UiNotice::TurningOn
                });
                *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                toggle_wifi.set_enabled(!toggle_wifi.is_enabled());
            }
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            Some(id) if id.kind() == Some(InterfaceKind::BluetoothAuto) => {
                if let Ok(slot) = toggle_ble.lock() {
                    if let Some(ble) = slot.as_ref() {
                        let next = !ble.is_enabled();
                        ui_state.show_notice(if next {
                            screen::UiNotice::TurningOn
                        } else {
                            screen::UiNotice::TurningOff
                        });
                        *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                        toggle_ble_enabled.store(next, Ordering::Relaxed);
                        ble.set_enabled(next);
                    }
                }
            }
            Some(id) if Some(id) == tcp_id => {
                if let Some(tcp) = &toggle_tcp {
                    ui_state.show_notice(if tcp.is_enabled() {
                        screen::UiNotice::TurningOff
                    } else {
                        screen::UiNotice::TurningOn
                    });
                    *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                    tcp.set_enabled(!tcp.is_enabled());
                }
            }
            _ => {}
        },
        UiAction::OpenLoRaEditor => ui_state.open_lora_editor(*working_lora_profile),
        UiAction::SetLoRaProfile(profile) => {
            ui_state.show_notice(screen::UiNotice::Saved);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            *working_lora_profile = profile;
        }
        UiAction::SwapRadioMode => {}
        UiAction::OpenDocs => {}
    };

    let mut ui_state = UiState::new();
    ui_state.set_storage_limits(<GrowableHeap as StorageLayout>::LIMITS);
    let mut working_lora_profile = DEFAULT_915_PROFILE;
    let mut notice_until: Option<Instant> = None;
    let mut active_press: Option<PressStart> = None;
    let mut last_logged: HashMap<InterfaceId, LoggedStatus> = HashMap::new();
    let mut interface_changes = query_handle.interface_store().subscribe();
    let mut cards: HVec<Card, 16> = HVec::new();
    let mut activity = screen::CardActivityTracker::<16>::new();
    let has_site_footer = false;
    let activity_started = Instant::now();
    let mut needs_redraw = true;
    let mut last_redraw = Instant::now();
    loop {
        if let Some(tray) = tray.as_mut() {
            for control in tray.drain_controls(window.is_visible()) {
                match control {
                    DesktopControl::ShowWindow => {
                        window.show();
                        active_press = None;
                        needs_redraw = true;
                    }
                    DesktopControl::HideWindow => {
                        window.hide();
                        active_press = None;
                    }
                    DesktopControl::Announce => {
                        apply_action(
                            UiAction::Announce,
                            None,
                            &mut ui_state,
                            &mut working_lora_profile,
                            &mut notice_until,
                        );
                        needs_redraw = true;
                    }
                    DesktopControl::Quit => return,
                }
            }
        }

        for event in window.events() {
            match event {
                SimulatorEvent::Quit => {
                    if tray.is_some() {
                        window.hide();
                        active_press = None;
                        continue;
                    }
                    return;
                }
                SimulatorEvent::KeyDown { repeat: false, .. } => {
                    active_press.get_or_insert(press_start(PressSource::Key));
                    needs_redraw = true;
                }
                SimulatorEvent::KeyUp { .. } => {
                    let selected_kind = selected_card_kind(&ui_state, cards.len(), &cards);
                    let released = finish_press(
                        &mut active_press,
                        PressSource::Key,
                        Instant::now(),
                        cards.len(),
                        has_site_footer,
                        selected_kind,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, cards.len(), &cards);
                    apply_action(
                        released,
                        selected,
                        &mut ui_state,
                        &mut working_lora_profile,
                        &mut notice_until,
                    );
                    needs_redraw = true;
                }
                SimulatorEvent::MouseButtonDown { .. } => {
                    active_press.get_or_insert(press_start(PressSource::Mouse));
                    needs_redraw = true;
                }
                SimulatorEvent::MouseButtonUp { .. } => {
                    let selected_kind = selected_card_kind(&ui_state, cards.len(), &cards);
                    let released = finish_press(
                        &mut active_press,
                        PressSource::Mouse,
                        Instant::now(),
                        cards.len(),
                        has_site_footer,
                        selected_kind,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, cards.len(), &cards);
                    apply_action(
                        released,
                        selected,
                        &mut ui_state,
                        &mut working_lora_profile,
                        &mut notice_until,
                    );
                    needs_redraw = true;
                }
                SimulatorEvent::KeyDown { repeat: true, .. }
                | SimulatorEvent::MouseWheel { .. }
                | SimulatorEvent::MouseMove { .. } => {}
            }
        }

        let holding = active_press.is_some();
        let selected_kind = selected_card_kind(&ui_state, cards.len(), &cards);
        let long_press = dispatch_long_press_if_ready(
            &mut active_press,
            Instant::now(),
            cards.len(),
            has_site_footer,
            selected_kind,
            &mut ui_state,
        );
        let selected = selected_card_id(&ui_state, cards.len(), &cards);
        apply_action(
            long_press,
            selected,
            &mut ui_state,
            &mut working_lora_profile,
            &mut notice_until,
        );

        let interfaces_changed = interface_changes.drain_changed();
        if window.is_visible()
            && (holding || interfaces_changed || last_redraw.elapsed() >= LIVE_REFRESH)
        {
            needs_redraw = true;
        }

        if notice_until.is_some_and(|until| Instant::now() >= until) {
            ui_state.clear_notice();
            notice_until = None;
            needs_redraw = true;
        }

        if needs_redraw && window.is_visible() {
            let snapshots = query_handle.interfaces();
            let now = Instant::now();
            for status in &snapshots {
                let connection = status.connection;
                let prev = last_logged.get(&status.id);
                let state_changed = prev.map_or(true, |p| p.connection != connection);
                let bytes_changed = prev
                    .map_or(status.rx_bytes != 0 || status.tx_bytes != 0, |p| {
                        p.rx_bytes != status.rx_bytes || p.tx_bytes != status.tx_bytes
                    });
                let throttle_ok = prev.map_or(true, |p| {
                    now.duration_since(p.last_emit) >= STATUS_LOG_THROTTLE
                });
                if state_changed || (bytes_changed && throttle_ok) {
                    println!(
                        "HOPSPOT_STATUS interface={} state={connection:?} rx={} tx={} links={} dst={}",
                        classify(status.id)
                            .as_ref()
                            .map_or("?", |(_, label)| label.as_str()),
                        status.rx_bytes,
                        status.tx_bytes,
                        status.links,
                        status.destinations,
                    );
                    last_logged.insert(
                        status.id,
                        LoggedStatus {
                            connection,
                            rx_bytes: status.rx_bytes,
                            tx_bytes: status.tx_bytes,
                            last_emit: now,
                        },
                    );
                }
            }
            cards = screen::snapshots_to_cards(&snapshots, classify);
            let activity_secs = activity_started
                .elapsed()
                .as_secs()
                .min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            ui_state.sync_card_count_with_footer(cards.len(), has_site_footer);
            let interface_menu_details = screen::snapshots_to_interface_menu_details(
                ui_state
                    .selected_card(cards.len())
                    .and_then(|index| cards.get(index)),
                &snapshots,
            );
            let battery = screen::BatteryGauge::lipo().sample(&mut screen::NoBattery);
            let animation_ms = activity_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            screen::draw_with_state_footer_details_at(
                &mut display,
                &cards,
                battery,
                &ui_state,
                None,
                &interface_menu_details,
                animation_ms,
            );
            window.update(&display);
            needs_redraw = false;
            last_redraw = Instant::now();
        }

        std::thread::sleep(FRAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_before_threshold_is_short_press() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = UiState::new();

        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD - Duration::from_millis(1),
            4,
            false,
            None,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(ui_state.selected_card(4), Some(0));
        assert_eq!(ui_state.menu_selected_item(), None);
    }

    #[test]
    fn hold_dispatches_long_press_at_threshold() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Mouse,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = UiState::new();

        dispatch_long_press_if_ready(
            &mut active_press,
            started_at + LONG_PRESS_THRESHOLD,
            4,
            false,
            None,
            &mut ui_state,
        );

        assert_eq!(ui_state.selected_card(4), None);
        assert_eq!(ui_state.global_menu_selected_item(), Some(0));
        assert!(active_press.expect("press remains active").long_press_sent);
    }

    #[test]
    fn release_after_dispatched_long_press_is_noop() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = UiState::new();

        dispatch_long_press_if_ready(
            &mut active_press,
            started_at + LONG_PRESS_THRESHOLD,
            4,
            false,
            None,
            &mut ui_state,
        );
        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD + Duration::from_millis(1),
            4,
            false,
            None,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(ui_state.selected_card(4), None);
        assert_eq!(ui_state.global_menu_selected_item(), Some(0));
    }

    #[test]
    fn release_at_threshold_is_long_press_fallback() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = UiState::new();

        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD,
            4,
            false,
            None,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(ui_state.selected_card(4), None);
        assert_eq!(ui_state.global_menu_selected_item(), Some(0));
    }
}
