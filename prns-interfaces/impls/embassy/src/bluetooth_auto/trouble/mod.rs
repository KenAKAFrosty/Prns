mod backend;
mod discovery;
mod platform;
mod sessions;
mod tasks;

use core::cell::{Cell, RefCell};

use bt_hci::transport::Transport;
use bt_hci::FromHciBytesError;
use embassy_futures::select::{select, select3, select4, Either, Either3, Either4};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use embassy_sync::signal::Signal;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{with_timeout, Duration, Instant, Timer};
use heapless_09::Vec as GattVec;
use portable_atomic::{AtomicBool, Ordering};
use trouble_host::att::{AttClient, AttReq};
use trouble_host::prelude::*;

use prns_core::interfaces::bluetooth_auto::{
    columba_connection_role, columba_role_capabilities, contains_service, default_group_tag,
    discovery_groups_match, encode_stream_frame, fragments_of, BleAddress, BleIdentity,
    BleRoleCapabilities, ColumbaConnectionRole, Control, Fragment, L2capPlan, PeerProtocol,
    Reassembler, BLE_HW_MTU, BLE_SERVICE_UUID_BYTES, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
    STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};

use super::connection_slots::{
    ConnectionSlotDataOwners, ConnectionSlotLease, ConnectionSlotLinkLease, ConnectionSlotOwners,
    ConnectionSlotPool, ConnectionSlotSinkLease, ConnectionSlotSourceLease,
    ConnectionSlotWorkerLease, ReadyConnectionSlot, ReadyConnectionSlotParts,
};
use super::frame_pool::{FrameLease, FramePoolError, SharedFramePool};
use super::runtime::BluetoothAutoStatus;

pub use platform::PEER_CAPACITY;
use platform::*;

const HCI_COMMAND_CAPACITY: usize = 20;
const ATTRIBUTE_TABLE: usize = 32;
const CCCD_TABLE: usize = 4;
pub const GATT_VALUE_CAP: usize = 244;
const MAX_SERVICES: usize = 2;

const CONTROL_UUID_LAST: u8 = 0xe7;
const DATA_UUID_LAST: u8 = 0xe8;
const COLUMBA_TX_UUID_LAST: u8 = 0xe4;
const COLUMBA_RX_UUID_LAST: u8 = 0xe5;
const COLUMBA_IDENTITY_UUID_LAST: u8 = 0xe6;
const SERVICE_UUID_LAST: u8 = 0xe3;

const GATT_REASSEMBLY_CAP: usize = 600;

const GATT_OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// A bounded connect scan frees the slot and lets policy back off when a whitelisted peer is absent.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(6);

/// Advertising and scanning alternate to avoid simultaneous-role controller limits; dial decisions made during an off-window remain buffered.
const ADV_WINDOW: Duration = Duration::from_millis(600);
const DISCOVERY_TURN_REST: Duration = Duration::from_millis(20);

const CONTROL_QUEUE_DEPTH: usize = 2;
const FRAME_QUEUE_DEPTH: usize = 2;
const FRAME_POOL_CAPACITY: usize = PEER_CAPACITY;
const FRAME_POOL_WAITERS: usize = PEER_CAPACITY + 1;
const SIGHTING_DEPTH: usize = PEER_CAPACITY * 2;
const SIGHTING_COALESCE_MS: u64 = 2_000;
const RADIO_WAITERS: usize = 2;

/// One L2CAP SDU carries one length-prefixed stream frame; modest credits and MPS keep two RX reservations inside the packet pool alongside GATT and TX.
pub const L2CAP_PSM: u16 = 0x0080;
const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const L2CAP_SDU_LENGTH_PREFIX_LEN: u16 = size_of::<u16>() as u16;
const L2CAP_CREDITS: u16 = (L2CAP_SDU_LEN as u16 + L2CAP_SDU_LENGTH_PREFIX_LEN).div_ceil(L2CAP_MPS);
const _: () = assert!(L2CAP_SDU_LEN <= <DefaultPacketPool as trouble_host::PacketPool>::MTU);
const L2CAP_HANDSHAKE_WINDOW: Duration = Duration::from_secs(5);
const L2CAP_SETUP_RETRY: Duration = Duration::from_millis(150);
const PHY_UPDATE_TIMEOUT: Duration = Duration::from_secs(2);
/// Retains the address kind needed to whitelist a peer when policy identifies it by six address bytes.
const SEEN_CAP: usize = PEER_CAPACITY * 2;
const FRAME_CAP: usize = BLE_HW_MTU;

type RadioArbiter = FairSemaphore<BridgeMutex, RADIO_WAITERS>;
type RadioPermit<'a> = SemaphoreReleaser<'a, RadioArbiter>;
type DiscoveryTurnArbiter = FairSemaphore<BridgeMutex, RADIO_WAITERS>;
type DiscoveryTurnPermit<'a> = SemaphoreReleaser<'a, DiscoveryTurnArbiter>;
type BleSlotPool = ConnectionSlotPool<BridgeMutex, PEER_CAPACITY>;
type BleSlotLease = ConnectionSlotLease<BridgeMutex>;
type BleSlotWorker = ConnectionSlotWorkerLease<BridgeMutex>;
type BleSlotLink = ConnectionSlotLinkLease<BridgeMutex>;
type BleSlotSource = ConnectionSlotSourceLease<BridgeMutex>;
type BleSlotSink = ConnectionSlotSinkLease<BridgeMutex>;
type BleReadySlot = ReadyConnectionSlot<BridgeMutex>;
type BleFramePool =
    SharedFramePool<BridgeMutex, FRAME_CAP, FRAME_POOL_CAPACITY, FRAME_POOL_WAITERS>;
type BleFrameLease = FrameLease<BridgeMutex, FRAME_CAP, FRAME_POOL_CAPACITY, FRAME_POOL_WAITERS>;
pub trait TroubleTransport: Transport<Error: From<FromHciBytesError>> {}
impl<T: Transport<Error: From<FromHciBytesError>>> TroubleTransport for T {}
pub type TroubleController<T> = ExternalController<T, HCI_COMMAND_CAPACITY>;
pub type TroubleStack<T> = Stack<'static, TroubleController<T>, DefaultPacketPool>;
pub type GattServer = AttributeServer<
    'static,
    NoopRawMutex,
    DefaultPacketPool,
    ATTRIBUTE_TABLE,
    CCCD_TABLE,
    PEER_CAPACITY,
>;
pub type GattCharacteristic = Characteristic<GattVec<u8, GATT_VALUE_CAP>>;
pub type ReticulumAttributeTable = AttributeTable<'static, NoopRawMutex, ATTRIBUTE_TABLE>;

pub use backend::{
    BleHub, Closed, EmbeddedBleBackend, EmbeddedBleLink, EmbeddedBleSink, EmbeddedBleSource,
};
pub use sessions::{
    columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid, data_uuid,
    reticulum_attribute_table, service_uuid, ReticulumGattCharacteristics, ReticulumGattUuids,
};
pub use tasks::{acceptor, dialer, host_runner, serve_slot};
