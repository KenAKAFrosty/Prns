//! The Linux debug face of the Personal Hopspot — one of the app's two targets.
//!
//! Runs the *same* announcing engine the S3 firmware does — here on the async reactor, over the
//! plug-and-play USB-auto interface that discovers and multiplexes every Personal board on a CDC
//! port — and renders the *same* Hopspot status screen the OLED shows, in an
//! `embedded-graphics-simulator` window. Run `cargo desktop`, plug in a Personal board, and watch
//! the cards tick as announces cross the link.
//!
//! The reactor runs on its own thread inside a tokio runtime; the SDL2 window owns the main
//! thread (SDL requires it) and repaints the interface's live status handle at ~30 fps — read
//! straight off the interface, never laundered through the engine.

use std::cell::Cell;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    IssuedCommand, Journaled, RatchetPolicy, SelfAnnounceAppData,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceStatus};
use personal_rns::reactor::impls::tokio_reactor::{
    run as run_reactor, Egress, TokioHost, TokioInterfaceStatus,
};
use personal_rns::reactor::interface_seam::{InboundFrame, OutboundFrame};
use personal_rns::reactor::interfaces::usb_auto::impls::tokio::UsbAutoHost;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;
use serialport::SerialPort;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use personal_hopspot_ui::{
    self as screen, BatteryState, Card, CardKind, InputEvent, UiAction, UiState,
};

/// Stable id for this node's USB-auto interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// CDC-ACM nominal baud (USB ignores it, but the port builder wants a value).
const USB_BAUD: u32 = 115_200;

/// The destination this node announces itself as: `lxmf.delivery`, the aspect LXMF apps
/// message — so the hopspot surfaces as a real, messageable peer.
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
/// How often the desktop re-announces itself.
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence — keeps the window responsive and re-reads the live status each frame.
const FRAME: Duration = Duration::from_millis(33);
/// Presses at or above this duration enter the long-press path.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(650);

/// This node's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519 private
/// keys). Handed to the engine through a [`Zeroizing`] buffer so it is wiped from this stack
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

fn interface_label(id: InterfaceId) -> &'static str {
    if id == USB_INTERFACE_ID {
        "USB"
    } else {
        "Unknown"
    }
}

fn classify(id: InterfaceId) -> Option<(CardKind, &'static str)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, "USB"))
    } else {
        None
    }
}

/// What the engine thread hands the window once the reactor is assembled: the command lane the
/// UI issues announces on, the live status handle it renders, and the destination its announces
/// name. The reactor owns everything else.
struct EngineHandles {
    command_tx: UnboundedSender<IssuedCommand>,
    status: TokioInterfaceStatus,
    destination: DestinationHash,
}

/// Spawn the reactor on its own thread, then own the SDL2 window on this (the main) thread: SDL
/// requires it. The engine thread hands the window its command lane and status handle back
/// before the reactor starts running.
pub fn run() {
    let identity_secret_key = load_identity_secret_key();

    let (ready_tx, ready_rx) = mpsc::channel::<EngineHandles>();
    std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(ready_tx, identity_secret_key))
        .expect("spawn engine thread");

    let handles = ready_rx
        .recv()
        .expect("the engine hands the window its handles before the reactor runs");
    run_window(handles);
}

fn run_engine(
    ready_tx: Sender<EngineHandles>,
    identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the engine thread builds its tokio runtime");

    runtime.block_on(async move {
        let mut engine = EngineState::<GrowableHeap>::new(identity_secret_key);
        let node = engine.held_identity_hashes()[0];
        engine
            .set_transport_identity(&node)
            .expect("the held identity takes the transport role");
        let destination = engine
            .register_single_destination(
                &node,
                SELF_ANNOUNCE_APP_NAME,
                SELF_ANNOUNCE_ASPECTS,
                ProofStrategy::ProveAll,
                RatchetPolicy::Ratcheted,
            )
            .expect("registers the lxmf.delivery destination");

        let (command_tx, command_rx) = unbounded_channel::<IssuedCommand>();
        let (funnel_tx, funnel_rx) = unbounded_channel::<InboundFrame>();
        let (outbound_tx, outbound_rx) = unbounded_channel::<OutboundFrame>();

        let egress = Egress::new(std::vec![(USB_INTERFACE_ID, outbound_tx)]);

        let interface = UsbAutoHost::new(USB_INTERFACE_ID, scan_cdc_ports, open_cdc_port);
        let status = interface.status();
        let interfaces = std::vec![interface.descriptor()];

        let _ = ready_tx.send(EngineHandles {
            command_tx: command_tx.clone(),
            status,
            destination,
        });

        let app_data = SelfAnnounceAppData::from_slice(SELF_ANNOUNCE_APP_DATA)
            .expect("the lxmf app_data fits");
        tokio::spawn(announce_loop(command_tx, destination, app_data));
        tokio::spawn(interface.run(funnel_tx, outbound_rx));

        run_reactor(
            engine,
            interfaces,
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            log_journaled,
        )
        .await;
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

/// The desktop's own announce cadence: the engine no longer self-announces, so the app fires a
/// scheduled `lxmf.delivery` announce on its own timer (the manual "Announce" menu item fires
/// the same command on demand).
async fn announce_loop(
    commands: UnboundedSender<IssuedCommand>,
    destination: DestinationHash,
    app_data: SelfAnnounceAppData,
) {
    let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
    let mut next_id = 0u64;
    loop {
        interval.tick().await;
        next_id += 1;
        let command = IssuedCommand {
            id: CommandId(next_id),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(app_data.clone()),
            }),
        };
        if commands.send(command).is_err() {
            return;
        }
        println!(
            "HOPSPOT_TX_ANNOUNCE_NOW id={next_id} kind=scheduled destination={:02x?}",
            destination.as_bytes(),
        );
    }
}

/// What the desktop observes — `Journaled` is the only thing the reactor hands the app (it
/// carries out the `Directive`s itself). Logs heard announces and inbound deliveries; command
/// settlements report the outcome of an issued announce.
fn log_journaled(journaled: Journaled<'_>) {
    match journaled {
        Journaled::AnnounceHeard {
            destination,
            hops,
            source_interface,
        } => {
            println!(
                "HOPSPOT_ANNOUNCE_HEARD destination={:02x?} hops={hops} interface={:02x?}",
                destination.as_bytes(),
                source_interface.as_bytes(),
            );
        }
        Journaled::Delivered(Delivery::Plain(delivery)) => {
            println!(
                "HOPSPOT_USB_RX_DELIVERY kind=plain destination={:02x?} bytes={}",
                delivery.destination.as_bytes(),
                delivery.payload.len(),
            );
        }
        Journaled::Delivered(Delivery::Single(delivery)) => {
            println!(
                "HOPSPOT_USB_RX_DELIVERY kind=single destination={:02x?} bytes={}",
                delivery.destination.as_bytes(),
                delivery.plaintext.len(),
            );
        }
        Journaled::CommandSettled { id, settlement } => {
            println!("HOPSPOT_COMMAND_SETTLED id={} {settlement:?}", id.0);
        }
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
    ui_state.handle_input(InputEvent::LongPress, card_count)
}

fn finish_press(
    active_press: &mut Option<PressStart>,
    source: PressSource,
    released_at: Instant,
    card_count: usize,
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
    ui_state.handle_input(event, card_count)
}

/// Own the SDL2 window: repaint the interface's live status as the Hopspot screen until the
/// window is closed, and funnel the menu's "Announce" item into the reactor's command lane.
fn run_window(handles: EngineHandles) {
    let EngineHandles {
        command_tx,
        status,
        destination,
    } = handles;

    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    let statuses = std::vec![status];
    let app_data =
        SelfAnnounceAppData::from_slice(SELF_ANNOUNCE_APP_DATA).expect("the lxmf app_data fits");
    let next_command_id = Cell::new(0u64);

    let apply_action = move |action: UiAction| match action {
        UiAction::None => {}
        UiAction::Announce => {
            let id = next_command_id.get() + 1;
            next_command_id.set(id);
            let _ = command_tx.send(IssuedCommand {
                id: CommandId(id),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Data(app_data.clone()),
                }),
            });
            println!(
                "HOPSPOT_TX_ANNOUNCE_NOW id={id} kind=manual destination={:02x?}",
                destination.as_bytes(),
            );
        }
    };

    let mut ui_state = UiState::new();
    let mut active_press: Option<PressStart> = None;
    let mut last_logged_connection: Option<ConnectionState> = None;
    loop {
        if let Some(status) = statuses.first() {
            let connection = status.connection();
            if last_logged_connection != Some(connection) {
                println!(
                    "HOPSPOT_STATUS interface={} state={connection:?} rx={} tx={}",
                    interface_label(status.id()),
                    status.rx_bytes(),
                    status.tx_bytes(),
                );
                last_logged_connection = Some(connection);
            }
        }

        let cards: HVec<Card, 8> = screen::statuses_to_cards(&statuses, classify);
        let card_count = cards.len();
        ui_state.sync_card_count(card_count);
        apply_action(dispatch_long_press_if_ready(
            &mut active_press,
            Instant::now(),
            card_count,
            &mut ui_state,
        ));

        let connecting = statuses
            .iter()
            .all(|status| status.connection() == ConnectionState::Initializing);
        if connecting {
            screen::splash(&mut display, "connecting");
        } else {
            screen::draw_with_state(&mut display, &cards, BatteryState::Unknown, &ui_state);
        }

        window.update(&display);
        for event in window.events() {
            match event {
                SimulatorEvent::Quit => return,
                SimulatorEvent::KeyDown { repeat: false, .. } => {
                    active_press.get_or_insert(press_start(PressSource::Key));
                }
                SimulatorEvent::KeyUp { .. } => {
                    apply_action(finish_press(
                        &mut active_press,
                        PressSource::Key,
                        Instant::now(),
                        card_count,
                        &mut ui_state,
                    ));
                }
                SimulatorEvent::MouseButtonDown { .. } => {
                    active_press.get_or_insert(press_start(PressSource::Mouse));
                }
                SimulatorEvent::MouseButtonUp { .. } => {
                    apply_action(finish_press(
                        &mut active_press,
                        PressSource::Mouse,
                        Instant::now(),
                        card_count,
                        &mut ui_state,
                    ));
                }
                SimulatorEvent::KeyDown { repeat: true, .. }
                | SimulatorEvent::MouseWheel { .. }
                | SimulatorEvent::MouseMove { .. } => {}
            }
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
            &mut ui_state,
        );
        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD + Duration::from_millis(1),
            4,
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
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(ui_state.selected_card(4), None);
        assert_eq!(ui_state.global_menu_selected_item(), Some(0));
    }
}
