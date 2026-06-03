//! The Linux debug face of the Personal Hopspot.
//!
//! Runs the *same* engine the S3 firmware does — an announcing
//! `Runtime` over a USB-serial interface — and renders the *same*
//! Hopspot status screen the OLED shows, in an `embedded-graphics-simulator`
//! window. Point it at a peer's serial device (`personal-hopspot-desktop
//! /dev/ttyACM0`) and watch the cards tick as announces cross the link.
//!
//! The engine runs on its own thread; the SDL2 window owns the main thread (SDL
//! requires it) and repaints the latest runtime snapshot at ~30 fps. This is the
//! daemon's engine wiring (mirrors `personal-rnsd`) with a face bolted on — the
//! two will diverge, so the ~60 shared lines are duplicated rather than factored.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::{ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::storage::FixedCapacity;
use personal_rns::interfaces::impls::rns_parity::serial::std_serial_interface;
use personal_rns::interfaces::storage::{GrowableInterfaceSet, InterfaceSet};
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::runtime::{block_on, Prns, Recipe, RuntimeSnapshot};

use personal_hopspot_ui::{self as screen, BatteryState, Card, CardKind, InputEvent, UiState};

/// Stable id for this node's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this node announces itself as (`personal.node`).
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

/// CDC-ACM nominal baud (USB ignores it, but `serialport` wants a value).
const USB_BAUD: u32 = 115_200;
/// The worker's blocking read window: short enough that a quiet link still loops
/// back to service outbound, long enough to not busy-spin.
const READ_POLL: Duration = Duration::from_millis(50);
/// How long to wait before re-opening the port after an open failure or unplug.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);
/// In-flight capacity of each of the interface's data rings.
const MAX_BUFFERED_PACKETS: usize = 64;

/// This debug face's engine-storage sizing (mirrors the daemon — a generous std
/// host): 64 dests / 64 ids each / 4 KB arena / 4 floor / 512 overflow / 64 held.
const ENGINE_STORAGE: FixedCapacity<64, 64, 4096, 4, 512, 64> = FixedCapacity;

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence. The engine pushes snapshots event-driven; this just keeps
/// the window responsive and repaints between snapshots.
const FRAME: Duration = Duration::from_millis(33);
/// Presses at or above this duration enter the long-press path.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(650);

/// This node's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519
/// private keys). Handed to the engine through a [`Zeroizing`] buffer so it is
/// wiped from this stack frame once construction copies it in.
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

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: personal-hopspot-desktop <serial-device>   (e.g. /dev/ttyACM0)");
        std::process::exit(2);
    };

    // The engine thread feeds each cycle's snapshot to the UI thread.
    let (snap_tx, snap_rx) = mpsc::channel::<RuntimeSnapshot>();
    std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(path, snap_tx))
        .expect("spawn engine thread");

    // SDL2 wants the window on the main thread.
    run_window(snap_rx);
}

/// Build the announcing engine + serial interface (the rnsd wiring) and drive the
/// runtime forever, forwarding each cycle's snapshot to the UI thread.
fn run_engine(path: String, snap_tx: Sender<RuntimeSnapshot>) {
    // The std poll-loop host owns the clock + wake.
    let host = LinuxSync::new();

    let open = move || {
        serialport::new(&path, USB_BAUD)
            .timeout(READ_POLL)
            .open()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    };
    let interface = std_serial_interface(USB_INTERFACE_ID, open, RECONNECT_INTERVAL);

    // `attach` glues the interface's seam (keyed by the id the interface carries),
    // starts its worker thread, and bundles what the runtime pools.
    let mut interfaces = GrowableInterfaceSet::new();
    let _ = interfaces.push(host.attach(interface, MAX_BUFFERED_PACKETS));

    block_on(Prns::run(
        Recipe {
            engine_storage: ENGINE_STORAGE,
            identity_secret_key: load_identity_secret_key(),
            self_announce: SelfAnnounceConfig {
                app_name: SELF_ANNOUNCE_APP_NAME,
                aspects: SELF_ANNOUNCE_ASPECTS,
                app_data: SELF_ANNOUNCE_APP_DATA,
                schedule: ReannounceSchedule::default(),
            },
            interfaces,
            host,
        },
        move |snapshot: &RuntimeSnapshot| {
            let _ = snap_tx.send(snapshot.clone());
        },
    ));
}

fn append_desktop_dummy_cards(cards: &mut HVec<Card, 8>) {
    // Desktop-only stand-ins so the single-button selection and pagination path
    // can be exercised before the runtime grows multiple real interfaces.
    let _ = cards.push(Card {
        kind: CardKind::Wifi,
        label: "WiFi",
        selected: false,
        online: true,
        tx_bytes: 1_830,
        rx_bytes: 0,
        links: 1_234,
        destinations: 56_789,
        rate_bytes_per_sec: 12_400,
        last_activity_secs: Some(3),
    });
    let _ = cards.push(Card {
        kind: CardKind::EspNow,
        label: "ESP-NOW",
        selected: false,
        online: true,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 999_999,
        destinations: 1_234_567,
        rate_bytes_per_sec: 987_000,
        last_activity_secs: Some(0),
    });
    let _ = cards.push(Card {
        kind: CardKind::Ble,
        label: "BLE",
        selected: false,
        online: true,
        tx_bytes: 42,
        rx_bytes: 12_340,
        links: 7,
        destinations: 12,
        rate_bytes_per_sec: 1_200,
        last_activity_secs: Some(42),
    });
    let _ = cards.push(Card {
        kind: CardKind::LoRa,
        label: "LoRa",
        selected: false,
        online: false,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    });
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
) {
    let Some(press) = active_press.as_mut() else {
        return;
    };
    if card_count == 0
        || press.long_press_sent
        || now.duration_since(press.started_at) < LONG_PRESS_THRESHOLD
    {
        return;
    }

    ui_state.handle_input(InputEvent::LongPress, card_count);
    press.long_press_sent = true;
}

fn finish_press(
    active_press: &mut Option<PressStart>,
    source: PressSource,
    released_at: Instant,
    card_count: usize,
    ui_state: &mut UiState,
) {
    let Some(press) = active_press.take() else {
        return;
    };
    if press.source != source {
        *active_press = Some(press);
        return;
    }

    if press.long_press_sent {
        return;
    }

    let event = if released_at.duration_since(press.started_at) >= LONG_PRESS_THRESHOLD {
        InputEvent::LongPress
    } else {
        InputEvent::ShortPress
    };
    ui_state.handle_input(event, card_count);
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

/// Own the SDL2 window: repaint the latest snapshot as the Hopspot screen until
/// the window is closed.
fn run_window(snap_rx: Receiver<RuntimeSnapshot>) {
    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    let mut snapshot: Option<RuntimeSnapshot> = None;
    let mut ui_state = UiState::new();
    let mut active_press: Option<PressStart> = None;
    loop {
        // Coalesce to the most recent snapshot the engine has produced.
        let mut latest = None;
        while let Ok(snap) = snap_rx.try_recv() {
            latest = Some(snap);
        }
        if latest.is_some() {
            snapshot = latest;
        }

        let card_count = match &snapshot {
            // This node has exactly one interface (the serial link), so one card.
            Some(snap) => {
                let mut cards: HVec<Card, 8> =
                    screen::snapshot_to_cards(snap, |_id| Some((CardKind::Usb, "USB")));
                append_desktop_dummy_cards(&mut cards);
                let card_count = cards.len();
                ui_state.sync_card_count(card_count);
                dispatch_long_press_if_ready(
                    &mut active_press,
                    Instant::now(),
                    card_count,
                    &mut ui_state,
                );
                screen::draw_with_state(&mut display, &cards, BatteryState::Unknown, &ui_state);
                card_count
            }
            None => {
                ui_state.sync_card_count(0);
                dispatch_long_press_if_ready(&mut active_press, Instant::now(), 0, &mut ui_state);
                screen::splash(&mut display, "connecting");
                0
            }
        };

        window.update(&display);
        for event in window.events() {
            match event {
                SimulatorEvent::Quit => return,
                SimulatorEvent::KeyDown { repeat: false, .. } => {
                    active_press.get_or_insert(press_start(PressSource::Key));
                }
                SimulatorEvent::KeyUp { .. } => {
                    finish_press(
                        &mut active_press,
                        PressSource::Key,
                        Instant::now(),
                        card_count,
                        &mut ui_state,
                    );
                }
                SimulatorEvent::MouseButtonDown { .. } => {
                    active_press.get_or_insert(press_start(PressSource::Mouse));
                }
                SimulatorEvent::MouseButtonUp { .. } => {
                    finish_press(
                        &mut active_press,
                        PressSource::Mouse,
                        Instant::now(),
                        card_count,
                        &mut ui_state,
                    );
                }
                SimulatorEvent::KeyDown { repeat: true, .. }
                | SimulatorEvent::MouseWheel { .. }
                | SimulatorEvent::MouseMove { .. } => {}
            }
        }
        std::thread::sleep(FRAME);
    }
}
