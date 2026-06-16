//! A focused Hopspot face for the WiFi/LAN auto-interface, on the high-level `Prns` runtime.
//!
//! Stands up a node with no pre-wired interfaces, supervises an `AutoWifi`, and renders the same
//! Hopspot status screen the OLED shows: the supervisor's aggregate "WiFi" card (Dormant until a
//! peer is confirmed, then Live), plus a card per peer the supervisor stands up. Run it on two
//! machines on the same WiFi and watch the cards go Live as the nodes peer.
//!
//!   cargo run --example wifi_hopspot
//!
//! The node runs on its own thread inside a tokio runtime; the SDL2 window owns the main thread
//! (SDL requires it) and repaints the supervisor's live status at ~30 fps.

use std::sync::mpsc;
use std::time::Duration;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiStatus};
use personal_rns::interfaces::{InterfaceId, InterfaceStatus};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};

use personal_hopspot_ui::{self as screen, BatteryState, Card, CardKind, UiState};

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence — re-reads the live status each frame.
const FRAME: Duration = Duration::from_millis(33);

fn main() {
    let (ready_tx, ready_rx) = mpsc::channel::<AutoWifiStatus>();
    std::thread::Builder::new()
        .name("wifi-hopspot-node".into())
        .spawn(move || run_node(ready_tx))
        .expect("spawn node thread");

    let wifi_status = ready_rx
        .recv()
        .expect("the node hands the window its wifi status before run() starts");
    run_window(wifi_status);
}

/// Build the node, supervise the WiFi auto-interface, hand the window the supervisor's status, then
/// drive the runtime forever on this thread.
fn run_node(ready_tx: mpsc::Sender<AutoWifiStatus>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the node thread builds its tokio runtime");

    runtime.block_on(async move {
        let mut identity = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
        getrandom::getrandom(&mut *identity).expect("OS CSPRNG must provide identity key material");

        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [PreConfiguredDestination::Single {
                app_name: "lxmf",
                aspects: &["delivery"],
                identity,
                announce_app_data: b"personal-hopspot-wifi",
                proof: ProofStrategy::ProveAll,
                ratchet: RatchetPolicy::Ratcheted,
            }],
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            on_event: |_event, _state| {},
            interfaces: interfaces![],
        });

        let handle = node.handle();
        let wifi = AutoWifi::new();
        let status = wifi.status();
        handle.supervise(wifi);

        let _ = ready_tx.send(status);
        node.run().await;
    });
}

/// Own the SDL2 window: repaint the supervisor's aggregate card plus a card per peer until the
/// window is closed.
fn run_window(wifi_status: AutoWifiStatus) {
    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot — WiFi", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    let supervisor_id = wifi_status.id();
    let classify = move |id: InterfaceId| -> Option<(CardKind, &'static str)> {
        if id == supervisor_id {
            Some((CardKind::Wifi, "WiFi"))
        } else {
            Some((CardKind::Wifi, "Peer"))
        }
    };

    let mut ui_state = UiState::new();
    loop {
        let members = wifi_status.members();
        let mut statuses: Vec<&dyn InterfaceStatus> = Vec::with_capacity(members.len() + 1);
        statuses.push(&wifi_status);
        for member in &members {
            statuses.push(member);
        }

        let cards: HVec<Card, 8> = screen::statuses_to_cards(&statuses, classify);
        ui_state.sync_card_count(cards.len());
        screen::draw_with_state(&mut display, &cards, BatteryState::Unknown, &ui_state);

        window.update(&display);
        for event in window.events() {
            if matches!(event, SimulatorEvent::Quit) {
                return;
            }
        }
        std::thread::sleep(FRAME);
    }
}
