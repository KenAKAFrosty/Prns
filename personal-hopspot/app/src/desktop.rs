//! The Linux debug face of the Personal Hopspot — one of the app's two targets.
//!
//! Runs the *same* announcing `Runtime` the S3 firmware does — over the plug-and-play
//! USB-auto interface — and renders the *same* Hopspot status screen the OLED shows, in
//! an `embedded-graphics-simulator` window. Run `cargo desktop` (no arguments), plug in
//! a Personal board, and watch the cards tick as announces cross the link.
//!
//! The engine runs on its own thread; the SDL2 window owns the main thread (SDL
//! requires it) and repaints the latest runtime snapshot at ~30 fps.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::ReannounceSchedule;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Delivered, EngineCommand,
    IssuedCommand, RatchetPolicy, SendSingle, SendSinglePayload, Settlement,
    MAX_SEND_SINGLE_PLAINTEXT_LEN,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
// use personal_rns::interfaces::impls::rns_parity::auto_interface::wifi_lan_auto_interface;
use personal_rns::interfaces::impls::usb_auto::usb_auto_interface;
use personal_rns::interfaces::storage::{GrowableInterfaceSet, InterfaceSet};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::host::impls::{LinuxSync, WakeHandle};
use personal_rns::runtime::{
    block_on, Prns, PrnsEvent, Recipe, RuntimeSnapshot, StartingDestinationConfig,
};
use personal_rns::wire::DestinationHash;

use personal_hopspot_ui::{
    self as screen, BatteryState, Card, CardKind, InputEvent, UiAction, UiState,
};

/// Stable id for this node's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);
const WIFI_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD1; 16]);

/// The destination this node announces itself as (`personal.node`).
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

/// In-flight capacity of each of the interface's data rings.
const MAX_BUFFERED_PACKETS: usize = 64;

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence. The engine pushes snapshots event-driven; this just keeps
/// the window responsive and repaints between snapshots.
const FRAME: Duration = Duration::from_millis(33);
/// Presses at or above this duration enter the long-press path.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(650);

/// Announce cadence: slow enough that goodput chunks, not announces, own the traffic line.
const ANNOUNCE_EVERY_MS: u64 = 60_000;
/// Cadence of the goodput chunks once a peer is heard.
const CHUNK_EVERY: Duration = Duration::from_secs(2);
/// Print the cumulative goodput line after this many chunk settlements.
const CHUNK_STATS_EVERY: u64 = 10;

enum DemoEvent {
    PeerHeard(DestinationHash),
    Settled(CommandId, Settlement),
}

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

/// Spawn the engine on its own thread, then own the SDL2 window on this (the main) thread: SDL requires it.
pub fn run() {
    // Loaded once: the engine answers as this identity, and the UI derives the
    // destination its Announce commands name from the same key.
    let identity_secret_key = load_identity_secret_key();
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&identity_secret_key);
    let name = expand_name(SELF_ANNOUNCE_APP_NAME, SELF_ANNOUNCE_ASPECTS)
        .expect("the self-announce name is valid");
    let self_destination = derive_destination_hash(&identity.identity_hash(), &name);

    // Two lanes between the threads: snapshots flow engine -> UI, commands flow
    // UI -> engine. The wake handle lets a button press cut the engine's sleep
    // short so its command is picked up on the next cycle.
    let (snap_tx, snap_rx) = mpsc::channel::<RuntimeSnapshot>();
    let (command_tx, command_rx) = mpsc::channel::<IssuedCommand>();
    let (demo_tx, demo_rx) = mpsc::channel::<DemoEvent>();
    let host = LinuxSync::new();
    let wake = host.wake_handle();
    let next_command_id = Arc::new(AtomicU64::new(0));

    std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(host, identity_secret_key, snap_tx, command_rx, demo_tx))
        .expect("spawn engine thread");

    let goodput_command_tx = command_tx.clone();
    let goodput_wake = wake.clone();
    let goodput_command_id = next_command_id.clone();
    std::thread::Builder::new()
        .name("hopspot-goodput".into())
        .spawn(move || {
            run_goodput_demo(
                demo_rx,
                goodput_command_tx,
                goodput_wake,
                goodput_command_id,
                self_destination,
            )
        })
        .expect("spawn goodput thread");

    run_window(snap_rx, command_tx, wake, next_command_id, self_destination);
}

fn run_engine(
    host: LinuxSync,
    identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    snap_tx: Sender<RuntimeSnapshot>,
    command_rx: Receiver<IssuedCommand>,
    demo_tx: Sender<DemoEvent>,
) {
    let mut interfaces = GrowableInterfaceSet::new();
    let _ =
        interfaces.push(host.attach(usb_auto_interface(USB_INTERFACE_ID), MAX_BUFFERED_PACKETS));
    // WiFi interface deliberately unregistered for now: the working rig is
    // USB-only, so every routed byte is attributable to the cable. Re-enable by
    // restoring this push when the rig needs WiFi again.
    // let _ = interfaces.push(host.attach(
    //     wifi_lan_auto_interface(WIFI_INTERFACE_ID),
    //     MAX_BUFFERED_PACKETS,
    // ));

    block_on(Prns::run(
        Recipe {
            engine_storage: GrowableHeap,
            starting_destinations: [StartingDestinationConfig::Single {
                app_name: SELF_ANNOUNCE_APP_NAME,
                aspects: SELF_ANNOUNCE_ASPECTS,
                identity_secret_key,
                proof_strategy: ProofStrategy::ProveAll,
                ratchet_policy: RatchetPolicy::Ratcheted,
                announce: Some(AnnounceConfig {
                    app_data: SELF_ANNOUNCE_APP_DATA,
                    schedule: ReannounceSchedule::every(ANNOUNCE_EVERY_MS),
                }),
            }],
            interfaces,
            host,
        },
        move |event: PrnsEvent<'_>| match event {
            PrnsEvent::SnapshotUpdated(snapshot) => {
                let _ = snap_tx.send(snapshot.clone());
            }
            PrnsEvent::Delivered(Delivery::Plain(delivery)) => {
                println!(
                    "HOPSPOT_USB_RX_DELIVERY kind=plain destination={:02x?} bytes={}",
                    delivery.destination.as_bytes(),
                    delivery.payload.len(),
                );
            }
            PrnsEvent::Delivered(Delivery::Single(delivery)) => {
                println!(
                    "HOPSPOT_USB_RX_DELIVERY kind=single destination={:02x?} bytes={}",
                    delivery.destination.as_bytes(),
                    delivery.plaintext.len(),
                );
            }
            PrnsEvent::AnnounceHeard {
                destination,
                hops,
                source_interface,
            } => {
                println!(
                    "HOPSPOT_ANNOUNCE_HEARD destination={:02x?} hops={hops} interface={:02x?}",
                    destination.as_bytes(),
                    source_interface.as_bytes(),
                );
                let _ = demo_tx.send(DemoEvent::PeerHeard(destination));
            }
            PrnsEvent::CommandSettled { id, settlement } => {
                println!("HOPSPOT_COMMAND_SETTLED id={} {settlement:?}", id.0);
                let _ = demo_tx.send(DemoEvent::Settled(id, settlement));
            }
        },
        // The engine's tap into the UI's command queue: every cycle sips until
        // the queue is dry that cycle (one command per cycle, then an Immediate
        // re-wake drains any burst).
        move || command_rx.try_recv().ok(),
    ));
}

/// A full single-packet chunk: a readable seq-stamped header, then a fill
/// pattern out to the 383-byte MDU so every send carries maximum goodput.
fn goodput_chunk(seq: u64) -> SendSinglePayload {
    let mut bytes = format!("hopspot-goodput seq={seq:06} ").into_bytes();
    while bytes.len() < MAX_SEND_SINGLE_PLAINTEXT_LEN {
        bytes.push(b'a' + ((seq + bytes.len() as u64) % 26) as u8);
    }
    SendSinglePayload::from_slice(&bytes).expect("the chunk is exactly the single-packet MDU")
}

/// The live goodput demonstration: adopt the first peer the engine hears, then
/// stream one full chunk every [`CHUNK_EVERY`], tallying settlements into a
/// running goodput line.
fn run_goodput_demo(
    demo_rx: Receiver<DemoEvent>,
    command_tx: Sender<IssuedCommand>,
    wake: WakeHandle,
    next_command_id: Arc<AtomicU64>,
    self_destination: DestinationHash,
) {
    let mut peer: Option<DestinationHash> = None;
    let mut next_chunk_at: Option<Instant> = None;
    let mut seq = 0u64;
    let mut outstanding: HashMap<u64, (u64, usize)> = HashMap::new();
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut failed = 0u64;
    let mut delivered_bytes = 0u64;
    let mut rtt_total_ms = 0u64;

    loop {
        let timeout = match next_chunk_at {
            Some(at) => at.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(3_600),
        };
        match demo_rx.recv_timeout(timeout) {
            Ok(DemoEvent::PeerHeard(destination)) => {
                if peer.is_none() && destination != self_destination {
                    println!(
                        "HOPSPOT_GOODPUT_TARGET destination={:02x?}",
                        destination.as_bytes(),
                    );
                    peer = Some(destination);
                    next_chunk_at = Some(Instant::now());
                }
            }
            Ok(DemoEvent::Settled(id, settlement)) => {
                if let Some((chunk_seq, bytes)) = outstanding.remove(&id.0) {
                    match settlement {
                        Settlement::SendSingle(Ok(Delivered { rtt_ms })) => {
                            delivered += 1;
                            delivered_bytes += bytes as u64;
                            rtt_total_ms += rtt_ms;
                            println!("HOPSPOT_CHUNK_DELIVERED seq={chunk_seq:06} rtt_ms={rtt_ms}");
                        }
                        other => {
                            failed += 1;
                            println!("HOPSPOT_CHUNK_FAILED seq={chunk_seq:06} {other:?}");
                        }
                    }
                    if (delivered + failed) % CHUNK_STATS_EVERY == 0 {
                        let avg_rtt_ms = rtt_total_ms / delivered.max(1);
                        println!(
                            "HOPSPOT_GOODPUT sent={sent} delivered={delivered} failed={failed} \
                             delivered_bytes={delivered_bytes} avg_rtt_ms={avg_rtt_ms}",
                        );
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        if let (Some(destination), Some(at)) = (peer, next_chunk_at) {
            if Instant::now() >= at {
                let id = CommandId(next_command_id.fetch_add(1, Ordering::Relaxed));
                let payload = goodput_chunk(seq);
                println!(
                    "HOPSPOT_TX_SEND_SINGLE id={} seq={seq:06} bytes={} destination={:02x?}",
                    id.0,
                    payload.len(),
                    destination.as_bytes(),
                );
                outstanding.insert(id.0, (seq, payload.len()));
                let _ = command_tx.send(IssuedCommand {
                    id,
                    command: EngineCommand::SendSingle(SendSingle {
                        destination,
                        payload,
                    }),
                });
                wake.poke();
                sent += 1;
                seq += 1;
                next_chunk_at = Some(Instant::now() + CHUNK_EVERY);
            }
        }
    }
}

/// Desktop-only stand-ins so the single-button selection and pagination path can
/// be exercised before the runtime grows multiple real interfaces. Parked while we
/// drive real boards — re-add the call in `run_window` to bring them back.
#[allow(dead_code)]
fn append_desktop_dummy_cards(cards: &mut HVec<Card, 8>) {
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

/// Own the SDL2 window: repaint the latest snapshot as the Hopspot screen until
/// the window is closed.
fn run_window(
    snap_rx: Receiver<RuntimeSnapshot>,
    command_tx: Sender<IssuedCommand>,
    wake: WakeHandle,
    next_command_id: Arc<AtomicU64>,
    self_destination: DestinationHash,
) {
    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    // Every input path funnels its UiAction here: selecting "Announce" in the
    // global menu queues the command for the engine thread and pokes its wake.
    let apply_action = move |action: UiAction| match action {
        UiAction::None => {}
        UiAction::Announce => {
            let id = CommandId(next_command_id.fetch_add(1, Ordering::Relaxed));
            let _ = command_tx.send(IssuedCommand {
                id,
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: self_destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Scheduled,
                }),
            });
            wake.poke();
            println!(
                "HOPSPOT_TX_ANNOUNCE_NOW id={} destination={:02x?}",
                id.0,
                self_destination.as_bytes(),
            );
        }
    };

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
            Some(snap) => {
                // Only the real interfaces from the runtime snapshot — the dummy
                // stand-ins (`append_desktop_dummy_cards`) stay parked for now.
                let cards: HVec<Card, 8> = screen::snapshot_to_cards(snap, |id| {
                    if id == USB_INTERFACE_ID {
                        Some((CardKind::Usb, "USB"))
                    } else if id == WIFI_INTERFACE_ID {
                        Some((CardKind::Wifi, "WiFi"))
                    } else {
                        None
                    }
                });
                let card_count = cards.len();
                ui_state.sync_card_count(card_count);
                apply_action(dispatch_long_press_if_ready(
                    &mut active_press,
                    Instant::now(),
                    card_count,
                    &mut ui_state,
                ));
                screen::draw_with_state(&mut display, &cards, BatteryState::Unknown, &ui_state);
                card_count
            }
            None => {
                ui_state.sync_card_count(0);
                apply_action(dispatch_long_press_if_ready(
                    &mut active_press,
                    Instant::now(),
                    0,
                    &mut ui_state,
                ));
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
