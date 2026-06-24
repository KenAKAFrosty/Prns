//! The desktop face of the Personal Hopspot — one of the app's two targets.
//!
//! Runs the *same* announcing engine the Heltec V4 firmware does, here on the high-level [`Prns`]
//! runtime: the recipe stands up the `lxmf.delivery` destination and the transport role, then the
//! app attaches the plug-and-play USB-auto interface (every Personal board on a CDC port) and
//! supervises the WiFi/LAN auto-interface (every Personal node on the same WiFi). It renders the
//! *same* Hopspot status screen the OLED shows — a card per interface, the WiFi supervisor's
//! aggregate plus a card per peer it stands up — in an `embedded-graphics-simulator` window. Run
//! `cargo desktop`, plug in a board or join a peer, and watch the cards tick as announces cross.
//!
//! The node runs on its own thread inside a tokio runtime; the SDL2 window owns the main thread
//! (SDL requires it) and repaints the interfaces' live status handles at ~30 fps — read straight
//! off each interface, never laundered through the engine.

use core::fmt::Write as _;
use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::local::core as local_core;
use personal_rns::interfaces::rns_parity::local::impls::rpc_compat::{
    reticulum_storage_dir, rpc_key_from_rns_identity, SharedInstanceRpcCompat,
};
use personal_rns::interfaces::rns_parity::local::impls::tokio::LocalServer;
use personal_rns::interfaces::rns_parity::lora::core::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::interfaces::rns_parity::tcp::client::tokio::TcpClientInterface;
use personal_rns::interfaces::rns_parity::tcp::core as tcp_core;
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::interfaces::usb_auto::impls::tokio::UsbAutoHost;
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Diagnostic, Message, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe, TokioPrnsHandle,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, TransportId};
use personal_rns::{interfaces, routes};
use serialport::SerialPort;
use tokio::sync::Notify;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use personal_hopspot_ui::{
    self as screen, BatteryState, Card, CardKind, InputEvent, UiAction, UiState,
};

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
/// Presses at or above this duration enter the long-press path.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(650);

/// This node's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519 private
/// keys). Handed to the recipe through a [`Zeroizing`] buffer so it is wiped from this stack
/// frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the X25519 0x22 ‖ Ed25519 0x11
        // keypair the oracle vectors pin. Never ship this — every fixture-identity
        // node shares one identity.
        key[..32].fill(0x22);
        key[32..].fill(0x11);
    }

    #[cfg(not(feature = "fixture-identity"))]
    {
        getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    }

    key
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
        Some((CardKind::Wifi, screen::card_label("WiFi")))
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
fn spawn_bluetooth(handle: TokioPrnsHandle, identity_hash: [u8; 16]) {
    use personal_rns::interfaces::bluetooth_auto::core::{
        AppleHost, BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
    use personal_rns_ffi::ble::macos::MacosBleBackend;

    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match MacosBleBackend::new().await {
            Ok(backend) => {
                let psm = backend.psm();
                handle.supervise(BluetoothAuto::new(
                    backend,
                    ble_identity,
                    Endpoint::CoreBluetooth(AppleHost::MacOs),
                    LinkCapabilities {
                        l2cap: Some(psm),
                        link_mtu: BLE_HW_MTU as u16,
                    },
                ));
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
fn spawn_bluetooth(handle: TokioPrnsHandle, identity_hash: [u8; 16]) {
    use personal_rns::interfaces::bluetooth_auto::core::{
        BleIdentity, Endpoint, LinkCapabilities, WinRtHost, BLE_HW_MTU,
    };
    use personal_rns::interfaces::bluetooth_auto::impls::tokio::BluetoothAuto;
    use personal_rns_ffi::ble::windows::WindowsBleBackend;

    let ble_identity = BleIdentity::new(identity_hash);
    tokio::spawn(async move {
        match WindowsBleBackend::new().await {
            Ok(backend) => {
                handle.supervise(BluetoothAuto::new(
                    backend,
                    ble_identity,
                    Endpoint::WinRt(WinRtHost::Windows),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                ));
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
            scan_cdc_ports,
            open_cdc_port,
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

        let wifi = AutoWifi::new();
        let wifi_status = wifi.status();
        if std::env::var_os("HOPSPOT_WIFI_OFF").is_some() {
            println!("wifi: not supervised (HOPSPOT_WIFI_OFF set)");
        } else {
            handle.supervise(wifi);
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        spawn_bluetooth(handle.clone(), identity_hash);

        handle.supervise(LocalServer::new());
        println!(
            "shared instance: local RNS apps (Sideband, NomadNet, MeshChat) can connect on 127.0.0.1:{}",
            local_core::DEFAULT_LOCAL_PORT
        );

        let rpc_port = local_core::DEFAULT_LOCAL_PORT + 1;
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
                    local_core::DEFAULT_SOCKET_PATH,
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
            tcp_status,
            tcp_id,
            tcp_target,
            destination,
        });

        if let Some(secs) = std::env::var("HOPSPOT_ANNOUNCE_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
        {
            let announce_handle = handle.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(secs));
                loop {
                    ticker.tick().await;
                    if let Some(id) = announce_handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })) {
                        println!(
                            "HOPSPOT_TX_ANNOUNCE_NOW id={} kind=periodic destination={:02x?}",
                            id.0,
                            destination.as_bytes(),
                        );
                    }
                }
            });
            println!("announce: emitting every {secs}s (HOPSPOT_ANNOUNCE_SECS)");
        }

        node.run().await;
    });
}

/// Enumerate the CDC (USB-serial) ports currently present — the names the host probes and
/// multiplexes. The host re-runs this on its own scan cadence to pick up hot-plugged boards.
fn scan_cdc_ports() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| info.port_name)
        .collect()
}

/// Open one CDC port into an async stream, settling the modem lines so an ESP32's native
/// USB-serial-JTAG (which maps RTS→EN, DTR→GPIO0) is never knocked into reset. Linux cdc-acm
/// opens the port at DTR=1/RTS=1; we lower RTS *before* DTR — (1,1)→(1,0)→(0,0) — so the lines
/// never pass through the reset combination DTR=0/RTS=1. A board behind a USB-UART bridge
/// ignores these, so it is harmless there.
async fn open_cdc_port(name: String) -> io::Result<SerialStream> {
    let mut port = tokio_serial::new(&name, USB_BAUD)
        .open_native_async()
        .map_err(io::Error::other)?;
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);
    Ok(port)
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
    ui_state.handle_input(InputEvent::LongPress, card_count, selected_kind)
}

fn finish_press(
    active_press: &mut Option<PressStart>,
    source: PressSource,
    released_at: Instant,
    card_count: usize,
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
    ui_state.handle_input(event, card_count, selected_kind)
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

/// Own the SDL2 window: repaint the interfaces' live status as the Hopspot screen until the
/// window is closed, and funnel the menu's "Announce" item into the node's command handle.
fn run_window(handles: WindowHandles) {
    let WindowHandles {
        handle,
        usb_status,
        wifi_status,
        tcp_status,
        tcp_id,
        tcp_target,
        destination,
    } = handles;

    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    let wifi_id = wifi_status.id();
    let tcp_target = tcp_target.as_deref();
    let classify = move |id: InterfaceId| classify(id, wifi_id, tcp_id, tcp_target);

    let query_handle = handle.clone();

    let toggle_usb = usb_status.clone();
    let toggle_tcp = tcp_status.clone();
    let apply_action =
        move |action: UiAction,
              selected_id: Option<InterfaceId>,
              ui_state: &mut UiState,
              working_lora_profile: &mut RadioProfile| match action {
            UiAction::None => {}
            UiAction::Announce => {
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
                    toggle_usb.set_enabled(!toggle_usb.is_enabled());
                }
                Some(id) if Some(id) == tcp_id => {
                    if let Some(tcp) = &toggle_tcp {
                        tcp.set_enabled(!tcp.is_enabled());
                    }
                }
                _ => {}
            },
            UiAction::OpenLoRaEditor => ui_state.open_lora_editor(*working_lora_profile),
            UiAction::SetLoRaProfile(profile) => *working_lora_profile = profile,
        };

    let mut ui_state = UiState::new();
    let mut working_lora_profile = DEFAULT_915_PROFILE;
    let mut active_press: Option<PressStart> = None;
    let mut last_logged: HashMap<InterfaceId, LoggedStatus> = HashMap::new();
    let mut interface_changes = query_handle.interface_store().subscribe();
    let mut cards: HVec<Card, 16> = HVec::new();
    let mut needs_redraw = true;
    let mut last_redraw = Instant::now();
    // Prime the SDL window before the first `events()` poll: the simulator creates the window
    // lazily on the first `update()`, and `events()` panics if called before it exists. The loop
    // enters with `needs_redraw = true`, so the real first frame is drawn immediately below.
    window.update(&display);
    loop {
        for event in window.events() {
            match event {
                SimulatorEvent::Quit => return,
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
                        selected_kind,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, cards.len(), &cards);
                    apply_action(released, selected, &mut ui_state, &mut working_lora_profile);
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
                        selected_kind,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, cards.len(), &cards);
                    apply_action(released, selected, &mut ui_state, &mut working_lora_profile);
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
            selected_kind,
            &mut ui_state,
        );
        let selected = selected_card_id(&ui_state, cards.len(), &cards);
        apply_action(
            long_press,
            selected,
            &mut ui_state,
            &mut working_lora_profile,
        );

        if holding || interface_changes.drain_changed() || last_redraw.elapsed() >= LIVE_REFRESH {
            needs_redraw = true;
        }

        if needs_redraw {
            let snapshots = query_handle.interfaces();
            let now = Instant::now();
            for status in &snapshots {
                let connection = status.connection;
                let prev = last_logged.get(&status.id);
                let state_changed = prev.map_or(true, |p| p.connection != connection);
                let bytes_changed = prev.map_or(
                    status.rx_bytes != 0 || status.tx_bytes != 0,
                    |p| p.rx_bytes != status.rx_bytes || p.tx_bytes != status.tx_bytes,
                );
                let throttle_ok =
                    prev.map_or(true, |p| now.duration_since(p.last_emit) >= STATUS_LOG_THROTTLE);
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
            let _ = cards.push(Card {
                id: InterfaceId::new(*b"lorademo"),
                kind: CardKind::LoRa,
                label: screen::card_label("LoRa"),
                selected: false,
                liveness: screen::Liveness::Dormant,
                tx_bytes: 0,
                rx_bytes: 0,
                links: 0,
                destinations: 0,
                rate_bytes_per_sec: 0,
                last_activity_secs: None,
            });
            ui_state.sync_card_count(cards.len());
            screen::draw_with_state(&mut display, &cards, BatteryState::Unknown, &ui_state);
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
            None,
            &mut ui_state,
        );
        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD + Duration::from_millis(1),
            4,
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
            None,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(ui_state.selected_card(4), None);
        assert_eq!(ui_state.global_menu_selected_item(), Some(0));
    }
}
