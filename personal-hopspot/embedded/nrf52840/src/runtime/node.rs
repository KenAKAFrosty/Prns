use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use personal_rns::engine::IssuedCommand;
use personal_rns::interfaces::bluetooth_auto::BLE_HW_MTU;
use personal_rns::interfaces::lora::LORA_MAX_PAYLOAD;
use personal_rns::interfaces::InterfaceId;
use personal_rns::lora::LoRaControl;
use personal_rns::manifold::embassy::{EmbassyHost, InterfaceLifecycle};
use personal_rns::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, ManifoldLaneSet, PrnsEvent, PrnsNode, PrnsNodeHandle,
    StaticManifoldLane,
};
use personal_rns::storage::{StorageCapacity, StorageLayout};

use super::bluetooth_auto;

pub(super) const LANE_COUNT: usize = 3;
pub(super) const LANE_DEPTH: usize = 1;
/// Two full-size radio records remain behind the active packet. Together with this lane's complete
/// five-frame reaction, the constrained board retains eight outbound LoRa packets end to end.
pub(super) const LORA_TX_QUEUE_BYTES: usize = 1024;
const LORA_OUTBOUND_DEPTH: usize = EngineStorageType::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
/// One resource request can synchronously emit every locally materialized part plus one hashmap
/// update. Keep that complete reaction lossless on page-serving packet interfaces.
const BLE_OUTBOUND_DEPTH: usize = EngineStorageType::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
const INTERFACE_CAPACITY: usize = 2 + bluetooth_auto::MEMBERS;
pub(super) const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 2;
pub(super) const LIFECYCLE_CAP: usize = bluetooth_auto::MEMBERS;
const COMPLETIONS_CAP: usize = 4;
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize =
    match <EngineStorageType as StorageLayout>::LIMITS.packet_hashes {
        StorageCapacity::Fixed(capacity) => capacity,
        StorageCapacity::Dynamic => panic!("embedded packet PHY retention needs fixed capacity"),
    };
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

const _: () = assert!(EngineStorageType::LINK_SESSIONS > bluetooth_auto::MEMBERS);

pub(crate) type Mtx = CriticalSectionRawMutex;
type EngineStorageType = crate::boards::selected::Storage;
type InterfaceStore = EmbassyInterfaceStore<
    Mtx,
    INTERFACE_STORE_CAP,
    PACKET_PHY_RETENTION_CAPACITY,
    PACKET_PHY_INDEX_BUCKETS,
>;
pub(super) type Node = PrnsNode<
    (),
    personal_hopspot_core::node_pages::NodePageRoutes,
    for<'a> fn(PrnsEvent<'a>, &()),
    EngineStorageType,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    LANE_COUNT,
    INTERFACE_CAPACITY,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;
pub(super) type ManifoldLanes = ManifoldLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;
/// The concrete command-handle type shared by the render loop and the headless
/// config task. Backed by the static [`COMMANDS`] channel and [`COMPLETION`] pool.
pub(super) type UiHandle = PrnsNodeHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;

pub(super) static LORA_CONTROL: LoRaControl = LoRaControl::new();
pub(super) static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
pub(super) static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
pub(super) static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
pub(super) static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
pub(super) static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
pub(super) static LORA_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    LORA_MAX_PAYLOAD,
    LANE_DEPTH,
    LORA_OUTBOUND_DEPTH,
> = StaticManifoldLane::new();
pub(super) static BLE_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    BLE_HW_MTU,
    LANE_DEPTH,
    BLE_OUTBOUND_DEPTH,
> = StaticManifoldLane::new();
pub(super) static USB_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    LANE_DEPTH,
> = StaticManifoldLane::new();

pub(super) fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}
