use embassy_executor::Spawner;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2"
))]
use embassy_futures::join::join;
#[cfg(feature = "board-mesh-tower-v2")]
use embassy_futures::join::join3;
use embassy_futures::join::join4;
#[cfg(feature = "board-t096")]
use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use personal_hopspot_core as hopspot;
use personal_rns::engine::IssuedCommand;
#[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::lora::{AirtimePolicy, DEFAULT_915_PROFILE, LORA_MAX_PAYLOAD};
use personal_rns::interfaces::usb_auto::{WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
#[cfg(feature = "board-t096")]
use personal_rns::interfaces::{
    InterfaceGravity, InterfaceMode, InterfaceSnapshot, InterfaceStatus, Membership,
};
#[cfg(feature = "board-t096")]
use personal_rns::lora::LoRaApplyOutcome;
use personal_rns::lora::{LoRaControl, LoRaInterface, LoRaInterfaceInput, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::{EmbassyHost, EmbassyInterfaceStatus, InterfaceLifecycle};
use personal_rns::manifold::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, ManifoldLaneSet, NoPersistence, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, StaticManifoldLane,
};
use personal_rns::storage::{StorageCapacity, StorageLayout};
use personal_rns::usb_auto::{
    UsbAutoDevice, UsbAutoDeviceInput, WebUsbAutoClass, WebUsbAutoState,
    WEBUSB_AUTO_CONTROL_BUFFER_BYTES, WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES, WEBUSB_AUTO_PACKET_SIZE,
};

use crate::boards::selected as board;
use board::{
    Board, Hardware, LoraInterface, Storage, ANNOUNCE_APP_DATA, NODE_ANNOUNCE_APP_DATA,
    USB_INTERFACE_ID, USB_MANUFACTURER, USB_PRODUCT, USB_SERIAL_NUMBER,
};

#[cfg(feature = "board-mesh-tower-v2")]
use super::bluetooth_auto::{
    acceptor, scanner, serve_slot, softdevice_config, softdevice_task, L2capPacket, NrfBleBackend,
    Server, BLE_SHARED, BLE_SUPERVISOR_ID, HUB, MEMBERS, OUTBOUND_WAKE, POOL,
};
use super::entropy::{initialize_runtime_entropy, runtime_entropy, RUNTIME_ENTROPY_SEED_LEN};
#[cfg(feature = "board-mesh-tower-v2")]
use nrf_softdevice::ble::l2cap;
#[cfg(feature = "board-mesh-tower-v2")]
use nrf_softdevice::Softdevice;
#[cfg(feature = "board-mesh-tower-v2")]
use personal_rns::bluetooth_auto::BluetoothAuto;
#[cfg(feature = "board-mesh-tower-v2")]
use personal_rns::interfaces::bluetooth_auto::{Endpoint, LinkCapabilities, Nrf52Host, BLE_HW_MTU};
#[cfg(feature = "board-mesh-tower-v2")]
use personal_rns::runtime::Fleet;

const USB_CONFIG_DESCRIPTOR_BYTES: usize = 64;
const USB_BOS_DESCRIPTOR_BYTES: usize = 64;
const WINDOWS_MSOS_VENDOR_CODE: u8 = 0x20;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e"
))]
const INTERFACE_CAPACITY: usize = 2;
#[cfg(feature = "board-mesh-tower-v2")]
const INTERFACE_CAPACITY: usize = 2 + MEMBERS;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e"
))]
const LANE_COUNT: usize = INTERFACE_CAPACITY;
#[cfg(feature = "board-mesh-tower-v2")]
const LANE_COUNT: usize = 3;
const LANE_DEPTH: usize = 1;
const LORA_TX_QUEUE_BYTES: usize = 1024;
const LORA_OUTBOUND_DEPTH: usize = Storage::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
#[cfg(feature = "board-mesh-tower-v2")]
const BLE_OUTBOUND_DEPTH: usize = Storage::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 2;
const LIFECYCLE_CAP: usize = INTERFACE_CAPACITY;
const COMPLETIONS_CAP: usize = 4;
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = match <Storage as StorageLayout>::LIMITS.packet_hashes
{
    StorageCapacity::Fixed(capacity) => capacity,
    StorageCapacity::Dynamic => panic!("embedded packet PHY retention needs fixed capacity"),
};
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

#[cfg(feature = "board-mesh-tower-v2")]
const _: () = assert!(Storage::LINK_SESSIONS > MEMBERS);

type Mtx = CriticalSectionRawMutex;
type InterfaceStore = EmbassyInterfaceStore<
    Mtx,
    INTERFACE_STORE_CAP,
    PACKET_PHY_RETENTION_CAPACITY,
    PACKET_PHY_INDEX_BUCKETS,
>;
type Node = PrnsNode<
    (),
    hopspot::node_pages::NodePageRoutes,
    for<'a> fn(PrnsEvent<'a>, &()),
    Storage,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    LANE_COUNT,
    INTERFACE_CAPACITY,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;
type ManifoldLanes = ManifoldLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;

static LORA_CONTROL: LoRaControl = LoRaControl::new();
static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
static LORA_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    LORA_MAX_PAYLOAD,
    LANE_DEPTH,
    LORA_OUTBOUND_DEPTH,
> = StaticManifoldLane::new();
#[cfg(feature = "board-mesh-tower-v2")]
static BLE_MANIFOLD_LANE: StaticManifoldLane<Mtx, BLE_HW_MTU, LANE_DEPTH, BLE_OUTBOUND_DEPTH> =
    StaticManifoldLane::new();
static USB_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticManifoldLane::new();

#[embassy_executor::task]
async fn manifold_task(node: &'static mut Node) {
    node.run_manifold_with_interface_store(&INTERFACE_STORE)
        .await
}

#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) -> ! {
    #[cfg(any(
        feature = "board-t096",
        feature = "board-t114",
        feature = "board-t1000e"
    ))]
    let ((node_bootstrap, runtime_entropy_seed), hardware) = Board::initialize(|nvmc, rng| {
        let mut fill_entropy = |bytes: &mut [u8]| rng.blocking_fill_bytes(bytes);
        let node_bootstrap = board::bootstrap_node_identity(nvmc, &mut fill_entropy);
        let mut runtime_entropy_seed =
            personal_rns::identity::Zeroizing::new([0u8; RUNTIME_ENTROPY_SEED_LEN]);
        fill_entropy(&mut runtime_entropy_seed[..]);
        (node_bootstrap, runtime_entropy_seed)
    })
    .await;
    #[cfg(feature = "board-mesh-tower-v2")]
    let ((node_bootstrap, ble_bootstrap, runtime_entropy_seed), hardware) =
        Board::initialize(|nvmc, rng| {
            let mut fill_entropy = |bytes: &mut [u8]| rng.blocking_fill_bytes(bytes);
            let node_bootstrap = board::bootstrap_node_identity(nvmc, &mut fill_entropy);
            let ble_bootstrap = board::bootstrap_ble_identity(nvmc, &mut fill_entropy);
            let mut runtime_entropy_seed =
                personal_rns::identity::Zeroizing::new([0u8; RUNTIME_ENTROPY_SEED_LEN]);
            fill_entropy(&mut runtime_entropy_seed[..]);
            (node_bootstrap, ble_bootstrap, runtime_entropy_seed)
        })
        .await;
    initialize_runtime_entropy(&runtime_entropy_seed);
    drop(runtime_entropy_seed);
    let node_identity = node_bootstrap.into_identity();
    #[cfg(feature = "board-mesh-tower-v2")]
    let ble_identity = Some(ble_bootstrap.into_identity());
    #[cfg(feature = "board-t096")]
    let Hardware {
        usb: usb_driver,
        radio,
        mut display,
        mut battery,
        button,
        mut status_led,
        gnss,
    } = hardware;
    #[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
    let Hardware {
        usb: usb_driver,
        radio,
        mut status_led,
        ..
    } = hardware;
    #[cfg(feature = "board-mesh-tower-v2")]
    let Hardware {
        usb: usb_driver,
        vbus,
        radio,
        mut status_led,
        button,
    } = hardware;

    let mut usb_config = UsbConfig::new(WEBUSB_VENDOR_ID, WEBUSB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT);
    usb_config.serial_number = Some(USB_SERIAL_NUMBER);
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; USB_CONFIG_DESCRIPTOR_BYTES]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; USB_BOS_DESCRIPTOR_BYTES]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; WEBUSB_AUTO_CONTROL_BUFFER_BYTES]> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; USB_CONFIG_DESCRIPTOR_BYTES]),
        BOS_DESC.init([0; USB_BOS_DESCRIPTOR_BYTES]),
        MSOS_DESC.init([0; WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES]),
        CONTROL_BUF.init([0; WEBUSB_AUTO_CONTROL_BUFFER_BYTES]),
    );
    builder.msos_descriptor(
        embassy_usb::msos::windows_version::WIN8_1,
        WINDOWS_MSOS_VENDOR_CODE,
    );
    static USB_STATE: StaticCell<WebUsbAutoState> = StaticCell::new();
    let class = WebUsbAutoClass::new(
        &mut builder,
        USB_STATE.init(WebUsbAutoState::new(super::bootloader_entry::webusb_entry())),
        WEBUSB_AUTO_PACKET_SIZE,
    );
    let mut usb = builder.build();

    #[cfg(feature = "board-mesh-tower-v2")]
    let sd = {
        let sd = Softdevice::enable(&softdevice_config());
        static SERVER: StaticCell<Server> = StaticCell::new();
        let server: &'static Server = SERVER.init(Server::new(sd).unwrap());
        static L2CAP: StaticCell<l2cap::L2cap<L2capPacket>> = StaticCell::new();
        let l2cap: &'static l2cap::L2cap<L2capPacket> = L2CAP.init(l2cap::L2cap::init(sd));
        let sd: &'static Softdevice = sd;
        spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));
        if let Some(identity) = ble_identity {
            super::bluetooth_auto::set_columba_identity(sd, server, identity);
        }
        if ble_identity.is_some() {
            for idx in 0..POOL {
                spawner.spawn(serve_slot(idx, sd, l2cap, server, &HUB).expect("serve slot fits"));
            }
        }
        sd
    };

    let transport_secret = node_identity.transport_secret();
    let destination_secret = node_identity.into_destination_secret();
    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
    let node_page_destination = hopspot::HopspotDestinationSet::new(
        destination_secret.clone(),
        ANNOUNCE_APP_DATA,
        NODE_ANNOUNCE_APP_DATA,
    )
    .destination_hashes()
    .expect("the hopspot destination names are valid")
    .node_page;
    let mut manifold_lanes = ManifoldLanes::new();
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = LoraInterface::interface_id(&lora_profile);
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing),
    );
    static LORA_SPECTRUM: StaticCell<LoRaSpectrumStatus> = StaticCell::new();
    let lora_spectrum = LORA_SPECTRUM.init(LoRaSpectrumStatus::new());
    static LORA_TX_QUEUE: ConstStaticCell<[u8; LORA_TX_QUEUE_BYTES]> =
        ConstStaticCell::new([0; LORA_TX_QUEUE_BYTES]);
    let lora = match LoRaInterface::new(LoRaInterfaceInput {
        radio,
        profile: lora_profile,
        airtime_policy: AirtimePolicy::Regional,
        tx_queue: LORA_TX_QUEUE.take(),
        control: &LORA_CONTROL,
        status: lora_status,
        spectrum: lora_spectrum,
        lifecycle: LIFECYCLE.dyn_sender(),
    }) {
        Ok(lora) => lora,
        Err(_) => panic!("the built-in LoRa profile and regional policy must be valid"),
    };

    let (usb_tx, usb_rx) = class.split();
    static USB_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let usb_status: &'static EmbassyInterfaceStatus = USB_STATUS.init(EmbassyInterfaceStatus::new(
        USB_INTERFACE_ID,
        ConnectionState::Initializing,
    ));
    let usb_device = UsbAutoDevice::new(UsbAutoDeviceInput {
        rx: usb_rx,
        tx: usb_tx,
        status: usb_status,
        host_present: || true,
    });

    let lora_lane = manifold_lanes
        .claim_interface(&LORA_MANIFOLD_LANE, lora.descriptor())
        .expect("LoRa lane is available");
    #[cfg(feature = "board-mesh-tower-v2")]
    let ble_supervisor_lane = ble_identity.as_ref().map(|_| {
        manifold_lanes
            .claim_supervisor(&BLE_MANIFOLD_LANE, BLE_SUPERVISOR_ID, &OUTBOUND_WAKE)
            .expect("Bluetooth supervisor lane is available")
    });
    let usb_lane = manifold_lanes
        .claim_interface(&USB_MANIFOLD_LANE, usb_device.descriptor())
        .expect("USB lane is available");
    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let manifold_wiring = manifold_lanes.into_manifold_wiring(
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new(runtime_entropy as fn(&mut [u8]));
    static NODE: StaticCell<Node> = StaticCell::new();
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: hopspot::HopspotDestinationSet::new(
            destination_secret,
            ANNOUNCE_APP_DATA,
            NODE_ANNOUNCE_APP_DATA,
        )
        .into_preconfigured_destinations(),
        app_state: (),
        storage: Storage,
        request_endpoints: hopspot::node_pages::NodePageRoutes,
        interfaces: personal_rns::runtime::ManuallyAttached,
        persistence: NoPersistence,
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };
    let node = PrnsNode::init_static(&NODE, recipe, manifold_wiring, host);
    node.set_protocol_policy(hopspot::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
    spawner.spawn(manifold_task(node).expect("manifold task fits"));

    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    #[cfg(feature = "board-mesh-tower-v2")]
    let bluetooth = {
        let backend = NrfBleBackend::new(&HUB);
        ble_identity
            .zip(ble_supervisor_lane)
            .map(|(identity, lane)| {
                let supervisor = BluetoothAuto::new(
                    backend,
                    identity,
                    Endpoint::Nrf52(Nrf52Host::Nrf52),
                    LinkCapabilities {
                        l2cap: None,
                        link_mtu: BLE_HW_MTU as u16,
                    },
                    &BLE_SHARED,
                );
                let fleet: Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP> =
                    lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());
                (supervisor, fleet)
            })
    };
    let heartbeat = async move {
        loop {
            status_led.illuminate();
            Timer::after(Duration::from_millis(100)).await;
            status_led.extinguish();
            Timer::after(Duration::from_millis(900)).await;
            #[cfg(any(feature = "board-mesh-tower-v2", feature = "board-t114"))]
            board::maintain().await;
        }
    };
    let io = join4(
        usb.run(),
        usb_device.run(usb_seam),
        heartbeat,
        super::bootloader_entry::wait(),
    );
    #[cfg(feature = "board-t096")]
    {
        let ui_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
        let face = async move {
            if !display.is_initialized() {
                core::future::pending::<()>().await;
            }
            let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
                storage_limits: <Storage as StorageLayout>::LIMITS,
                display_power_control: hopspot::DisplayPowerControl::Unavailable,
                access_point: hopspot::AccessPointState::Unsupported,
                gnss: hopspot::GnssAvailability::Available,
            });
            let mut activity = hopspot::CardActivityTracker::<2>::new();
            let mut battery_gauge = hopspot::BatteryGauge::lipo();
            let mut working_lora_profile = lora_profile;
            let mut notice_until_ms = None;
            loop {
                let battery_mv = battery.sample_millivolts().await;
                let battery_state =
                    battery_gauge.update(Some(battery_mv), board::external_power_state());
                let snapshots = t096_snapshots(lora_status, usb_status);
                let mut cards: heapless::Vec<hopspot::Card, 2> =
                    hopspot::snapshots_to_cards(&snapshots, |id| {
                        if id == lora_status.id() {
                            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
                        } else if id == usb_status.id() {
                            Some((hopspot::CardKind::Usb, hopspot::card_label("USB")))
                        } else {
                            None
                        }
                    });
                let now_ms = embassy_time::Instant::now().as_millis();
                if let Some((until, owner)) = notice_until_ms {
                    if now_ms >= until {
                        notice_until_ms = None;
                        let _cleared = ui_state.clear_notice_if(owner);
                    }
                }
                activity.update(&mut cards, (now_ms / 1_000).min(u64::from(u32::MAX)) as u32);
                let content = hopspot::ScreenContent {
                    cards: &cards,
                    local_docs: None,
                };
                ui_state.sync(content);
                let details = hopspot::snapshots_to_interface_menu_details(
                    ui_state.selected_card(content.cards),
                    &snapshots,
                );
                hopspot::render(
                    &mut display,
                    hopspot::RenderFrame {
                        content,
                        battery: battery_state,
                        gnss: ui_state.gnss_visible().then(board::gnss_snapshot),
                        state: &ui_state,
                        interface_menu_details: &details,
                        animation_ms: now_ms,
                    },
                );
                // A transient panel fault must not take the networking node down. The display
                // retains the last confirmed frame, so the next face tick retries the update.
                let _panel_update = display.flush();
                match select3(
                    board::INPUT_EVENTS.receive(),
                    INTERFACE_STORE.changed(),
                    Timer::after(Duration::from_secs(1)),
                )
                .await
                {
                    Either3::First(event) => {
                        let now_ms = embassy_time::Instant::now().as_millis();
                        match ui_state.handle_input(event, content) {
                            hopspot::UiAction::Announce => {
                                let notice = hopspot::UiNotice::Announcing;
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + 900, notice));
                                let _issued =
                                    ui_handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                                        destination: node_page_destination,
                                        target: AnnounceTarget::AllInterfaces,
                                        app_data: AnnounceAppData::Registered,
                                    }));
                            }
                            hopspot::UiAction::Sleep => {
                                let notice = hopspot::UiNotice::Sleeping;
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + 900, notice));
                                lora_status.disable();
                                usb_status.disable();
                                board::control_gnss(hopspot::GnssReceiverCommand::Disable);
                            }
                            hopspot::UiAction::Wake => {
                                let notice = hopspot::UiNotice::Awake;
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + 900, notice));
                                lora_status.enable();
                                usb_status.enable();
                                if ui_state.gnss_visible() {
                                    board::control_gnss(hopspot::GnssReceiverCommand::Enable);
                                }
                            }
                            hopspot::UiAction::ControlGnss(command) => {
                                board::control_gnss(command);
                            }
                            hopspot::UiAction::ToggleSelectedInterface => {
                                if let Some(card) = ui_state.selected_card(content.cards) {
                                    let status = if card.id() == lora_status.id() {
                                        Some(lora_status)
                                    } else if card.id() == usb_status.id() {
                                        Some(usb_status)
                                    } else {
                                        None
                                    };
                                    if let Some(status) = status {
                                        let notice = if status.is_enabled() {
                                            hopspot::UiNotice::TurningOff
                                        } else {
                                            hopspot::UiNotice::TurningOn
                                        };
                                        ui_state.show_notice(notice);
                                        notice_until_ms = Some((now_ms + 900, notice));
                                        status.toggle_enabled();
                                    }
                                }
                            }
                            hopspot::UiAction::OpenLoRaEditor => {
                                ui_state.open_lora_editor(working_lora_profile);
                            }
                            hopspot::UiAction::SetLoRaProfile(profile) => {
                                let result = hopspot::apply_and_persist_radio_profile(
                                    async {
                                        LORA_CONTROL.apply(profile).await
                                            == LoRaApplyOutcome::Applied
                                    },
                                    || async { false },
                                )
                                .await;
                                if result.applied() {
                                    working_lora_profile = profile;
                                }
                                let notice = result.notice();
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + 900, notice));
                            }
                            hopspot::UiAction::ResetLoRaProfile => {
                                let result = hopspot::apply_and_persist_radio_profile(
                                    async {
                                        LORA_CONTROL.apply(DEFAULT_915_PROFILE).await
                                            == LoRaApplyOutcome::Applied
                                    },
                                    || async { false },
                                )
                                .await;
                                if result.applied() {
                                    working_lora_profile = DEFAULT_915_PROFILE;
                                }
                                let notice = result.notice();
                                ui_state.show_notice(notice);
                                notice_until_ms = Some((now_ms + 900, notice));
                            }
                            hopspot::UiAction::None
                            | hopspot::UiAction::OledOff
                            | hopspot::UiAction::ToggleOledAutoOff
                            | hopspot::UiAction::ToggleStationUplink
                            | hopspot::UiAction::SwapRadioMode
                            | hopspot::UiAction::OpenDocs => {}
                        }
                    }
                    Either3::Second(()) | Either3::Third(()) => {}
                }
            }
        };
        let primary = join4(io, lora.run(lora_seam), face, board::drive_button(button));
        join(primary, board::drive_gnss(gnss)).await;
    }
    #[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
    {
        join(io, lora.run(lora_seam)).await;
    }
    #[cfg(feature = "board-mesh-tower-v2")]
    {
        let ble_plane = async move {
            match bluetooth {
                Some((supervisor, fleet)) => {
                    join3(acceptor(sd, &HUB), scanner(sd, &HUB), supervisor.run(fleet)).await;
                }
                None => core::future::pending().await,
            }
        };
        let announce_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
        let announce = async move {
            loop {
                board::BUTTON_PRESSES.receive().await;
                while announce_handle
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: node_page_destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    Timer::after(Duration::from_millis(50)).await;
                }
            }
        };
        join(
            join3(io, ble_plane, lora.run(lora_seam)),
            join(board::drive_button(button), announce),
        )
        .await;
    }
    core::future::pending().await
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

#[cfg(feature = "board-t096")]
fn t096_snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> heapless::Vec<InterfaceSnapshot, 2> {
    let mut snapshots = heapless::Vec::new();
    for status in [lora, usb] {
        let counts = INTERFACE_STORE.counts(status.id());
        let _ = snapshots.push(InterfaceSnapshot {
            id: status.id(),
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            connection: status.connection(),
            failure_reason: status.failure_reason(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            destinations: counts.destinations,
            links: counts.links,
            transported_links: counts.transported_links,
            membership: Membership::Independent,
        });
    }
    snapshots
}
