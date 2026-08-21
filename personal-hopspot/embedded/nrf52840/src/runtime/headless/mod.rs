use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use personal_hopspot_core as hopspot;
use personal_rns::engine::IssuedCommand;
#[cfg(not(feature = "board-t096"))]
use personal_rns::interfaces::lora::DEFAULT_915_PROFILE;
use personal_rns::interfaces::lora::{AirtimePolicy, LORA_MAX_PAYLOAD};
use personal_rns::interfaces::usb_auto::{WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::lora::{LoRaControl, LoRaInterface, LoRaInterfaceInput, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::{EmbassyHost, EmbassyInterfaceStatus, InterfaceLifecycle};
use personal_rns::manifold::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
#[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
use personal_rns::runtime::NoPersistence;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, ManifoldLaneSet, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    StaticManifoldLane,
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

use super::entropy::{initialize_runtime_entropy, runtime_entropy, RUNTIME_ENTROPY_SEED_LEN};

#[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
mod bluetooth;
#[cfg(feature = "board-mesh-tower-v2")]
#[path = "mesh_tower_v2.rs"]
mod selected;
#[cfg(feature = "board-t096")]
#[path = "t096.rs"]
mod selected;
#[cfg(feature = "board-t1000e")]
#[path = "t1000e.rs"]
mod selected;
#[cfg(feature = "board-t114")]
#[path = "t114.rs"]
mod selected;

const USB_CONFIG_DESCRIPTOR_BYTES: usize = 64;
const USB_BOS_DESCRIPTOR_BYTES: usize = 64;
const WINDOWS_MSOS_VENDOR_CODE: u8 = 0x20;
const INTERFACE_CAPACITY: usize = selected::INTERFACE_CAPACITY;
const LANE_COUNT: usize = selected::LANE_COUNT;
const LANE_DEPTH: usize = 1;
const LORA_TX_QUEUE_BYTES: usize = 1024;
const LORA_OUTBOUND_DEPTH: usize = Storage::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
#[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
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

#[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
const _: () = assert!(Storage::LINK_SESSIONS > bluetooth::MEMBERS);

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
#[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
static BLE_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    { bluetooth::BLE_HW_MTU },
    LANE_DEPTH,
    BLE_OUTBOUND_DEPTH,
> = StaticManifoldLane::new();
static USB_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticManifoldLane::new();

#[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
#[embassy_executor::task]
async fn manifold_task(node: &'static mut Node) {
    node.run_manifold_with_interface_store(&INTERFACE_STORE)
        .await
}

#[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
#[embassy_executor::task]
async fn manifold_task(node: &'static mut Node, persistence: &'static mut board::Persistence) {
    let _ = node.restore_embedded_persistence(persistence).await;
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) -> ! {
    #[cfg(any(feature = "board-t114", feature = "board-t1000e"))]
    let ((node_bootstrap, runtime_entropy_seed), hardware) = Board::initialize(|nvmc, rng| {
        let mut fill_entropy = |bytes: &mut [u8]| rng.blocking_fill_bytes(bytes);
        let node_bootstrap = board::bootstrap_node_identity(nvmc, &mut fill_entropy);
        let mut runtime_entropy_seed =
            personal_rns::identity::Zeroizing::new([0u8; RUNTIME_ENTROPY_SEED_LEN]);
        fill_entropy(&mut runtime_entropy_seed[..]);
        (node_bootstrap, runtime_entropy_seed)
    })
    .await;
    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
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
    #[cfg(feature = "board-t096")]
    let identity_startup_notice =
        board::identity_startup_notice(node_bootstrap.persistence(), ble_bootstrap.persistence());
    let node_identity = node_bootstrap.into_identity();
    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
    let ble_identity = Some(ble_bootstrap.into_identity());
    #[cfg(feature = "board-t096")]
    let Hardware {
        usb: usb_driver,
        vbus,
        radio,
        display,
        battery,
        button,
        mut status_led,
        gnss,
    } = hardware;
    #[cfg(feature = "board-t114")]
    let Hardware {
        usb: usb_driver,
        radio,
        mut status_led,
        ..
    } = hardware;
    #[cfg(feature = "board-t1000e")]
    let Hardware {
        flash,
        usb: usb_driver,
        radio,
        mut status_led,
        gnss,
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

    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
    let sd = bluetooth::enable(spawner, vbus, ble_identity);

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
    #[cfg(feature = "board-t096")]
    let (loaded_lora_profile, persistence) = selected::load_profile(sd).await;
    #[cfg(feature = "board-t1000e")]
    let persistence = board::new_persistence(flash);
    #[cfg(feature = "board-t096")]
    let lora_profile = loaded_lora_profile.profile;
    #[cfg(not(feature = "board-t096"))]
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = LoraInterface::interface_id(&lora_profile);
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing),
    );
    static LORA_SPECTRUM: StaticCell<LoRaSpectrumStatus> = StaticCell::new();
    let lora_spectrum: &'static LoRaSpectrumStatus = LORA_SPECTRUM.init(LoRaSpectrumStatus::new());
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
    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
    let ble_supervisor_lane = ble_identity.as_ref().map(|_| {
        manifold_lanes
            .claim_supervisor(
                &BLE_MANIFOLD_LANE,
                bluetooth::BLE_SUPERVISOR_ID,
                &bluetooth::OUTBOUND_WAKE,
            )
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
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
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
        persistence,
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };
    #[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
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
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
    let (node, persistence) =
        PrnsNode::init_static_with_persistence(&NODE, recipe, manifold_wiring, host);
    #[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
    let node = PrnsNode::init_static(&NODE, recipe, manifold_wiring, host);
    node.set_protocol_policy(hopspot::EMBEDDED_HOPSPOT_PROTOCOL_POLICY);
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
    {
        static PERSISTENCE: StaticCell<board::Persistence> = StaticCell::new();
        let persistence = PERSISTENCE.init(persistence);
        spawner.spawn(manifold_task(node, persistence).expect("manifold task fits"));
    }
    #[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
    spawner.spawn(manifold_task(node).expect("manifold task fits"));

    let lora_seam = lora_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    let usb_seam = usb_lane.into_seam(NOTIFY.sender(), runtime_entropy);
    #[cfg(any(feature = "board-t096", feature = "board-mesh-tower-v2"))]
    let bluetooth = bluetooth::prepare(ble_identity, ble_supervisor_lane);
    let heartbeat = async move {
        loop {
            status_led.illuminate();
            let illuminated = selected::heartbeat_illuminated_ms();
            Timer::after(Duration::from_millis(illuminated)).await;
            status_led.extinguish();
            Timer::after(Duration::from_millis(1_000 - illuminated)).await;
            selected::maintain().await;
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
        let face = selected::face(selected::FaceInput {
            display,
            battery,
            profile_store: loaded_lora_profile.store,
            identity_startup_notice,
            profile_startup_notice: loaded_lora_profile.startup_notice,
            lora_profile,
            lora_status,
            usb_status,
            lora_spectrum,
            node_page_destination,
        });
        selected::run(
            io,
            lora.run(lora_seam),
            face,
            bluetooth::run(sd, bluetooth),
            button,
            gnss,
        )
        .await;
    }
    #[cfg(feature = "board-t114")]
    selected::run(io, lora.run(lora_seam)).await;
    #[cfg(feature = "board-t1000e")]
    selected::run(io, lora.run(lora_seam), gnss).await;
    #[cfg(feature = "board-mesh-tower-v2")]
    selected::run(
        io,
        lora.run(lora_seam),
        bluetooth::run(sd, bluetooth),
        button,
        node_page_destination,
    )
    .await;
    core::future::pending().await
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}
