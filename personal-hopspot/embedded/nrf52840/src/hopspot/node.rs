use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use personal_rns::engine::IssuedCommand;
use personal_rns::interfaces::bluetooth_auto::BLE_HW_MTU;
use personal_rns::interfaces::lora::LORA_MAX_PAYLOAD;
use personal_rns::interfaces::InterfaceId;
use personal_rns::lora::LoRaControl;
use personal_rns::reactor::embassy::{EmbassyHost, InterfaceLifecycle};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_reactor_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, PrnsEvent, PrnsNode, ReactorLaneSet, StaticReactorLane,
};
use personal_rns::storage::{StorageCapacity, StorageLayout};

use super::bluetooth_auto;

pub(super) const LANE_COUNT: usize = 3;
pub(super) const LANE_DEPTH: usize = 1;
const INTERFACE_CAPACITY: usize = 2 + bluetooth_auto::MEMBERS;
pub(super) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"techousb");
pub(super) const NOTIFY_CAP: usize = minimum_reactor_notification_capacity(LANE_COUNT, LANE_DEPTH);
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

pub(super) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x17Personal Hopspot T-Echo\xc0";
pub(super) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T-Echo";

pub(super) type Mtx = CriticalSectionRawMutex;
type EngineStorageType = crate::storage::TechoStorage;
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
pub(super) type ReactorLanes = ReactorLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;

pub(super) static LORA_CONTROL: LoRaControl = LoRaControl::new();
pub(super) static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
pub(super) static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
pub(super) static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
pub(super) static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
pub(super) static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
pub(super) static LORA_REACTOR_LANE: StaticReactorLane<Mtx, LORA_MAX_PAYLOAD, LANE_DEPTH> =
    StaticReactorLane::new();
pub(super) static BLE_REACTOR_LANE: StaticReactorLane<Mtx, BLE_HW_MTU, LANE_DEPTH> =
    StaticReactorLane::new();
pub(super) static USB_REACTOR_LANE: StaticReactorLane<
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    LANE_DEPTH,
> = StaticReactorLane::new();

pub(super) static ENTROPY_STATE: AtomicU32 = AtomicU32::new(0x9e37_79b9);

pub(super) fn seeded_entropy(bytes: &mut [u8]) {
    let mut state = ENTROPY_STATE.load(Ordering::Relaxed);
    for byte in bytes {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = (state >> 24) as u8;
    }
    ENTROPY_STATE.store(state, Ordering::Relaxed);
}

pub(super) fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}
