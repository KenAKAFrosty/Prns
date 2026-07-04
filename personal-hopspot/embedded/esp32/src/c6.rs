//! The Seeed XIAO ESP32-C6 Hopspot board: single-core, no-PSRAM, headless. The engine is constructed
//! and run on the one RISC-V core (no second-core hand-off like the S3), with its columns inline in
//! internal SRAM. The Hopspot build is intentionally narrow: USB-auto, ESP-NOW, and BLE.

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::{base_mac_address, MacAddress};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::peripherals::{BT, USB_DEVICE};
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use heapless::Vec as HVec;
use portable_atomic::{AtomicU64, Ordering};
use static_cell::StaticCell;

use personal_rns::engine::{InstantMillis, IssuedCommand, RatchetPolicy};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::usb_auto::core::device_descriptor;
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::reactor::impls::embassy_reactor::{
    EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, EmbassyInterfaceSeam,
    EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::reactor::timebase::EmbassyTimebase;
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbassyPrnsHandle, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::usb::UsbAutoDevice;

use crate::storage::{C6Storage, EngineStorageType};

use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
#[cfg(feature = "ble-bringup-c6")]
use personal_rns::ble::BluetoothAutoShared;
use personal_rns::interfaces::bluetooth_auto::limits;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::reactor::grant::FrameSlot;
use personal_rns::reactor::impls::embassy_reactor::embassy_grant_lane;
use personal_rns::runtime::{Fleet, MemberWire};
use static_cell::ConstStaticCell;

#[cfg(feature = "espnow-c6")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "espnow-c6")]
use esp_radio::wifi::ControllerConfig;
#[cfg(feature = "espnow-c6")]
use personal_rns::esp_now::EspNowInterface;
#[cfg(feature = "espnow-c6")]
use personal_rns::interfaces::esp_now::core::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy,
};
use personal_rns::reactor::interface_seam::Interface;

esp_app_desc!();

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";

const USB_LANE: usize = 1;
const ESPNOW_LANE: usize = cfg!(feature = "espnow-c6") as usize;
const BLE_LANE: usize = cfg!(feature = "ble-bringup-c6") as usize;
const LANE_COUNT: usize = USB_LANE + ESPNOW_LANE + BLE_LANE;
const IFACES: usize = if LANE_COUNT == 0 { 1 } else { LANE_COUNT };
pub const BLE_MEMBERS: usize = limits::ESP32_C6_MAX_PEERS;
pub const BLE_CONTROLLER_CONNECTIONS: usize = 8;
const MAX_IFACES: usize = IFACES + BLE_LANE * BLE_MEMBERS + 1;
pub const NOTIFY_CAP: usize = 32;
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 32;
const COMPLETIONS_CAP: usize = 4;
const STORE_CAP: usize = 32;
#[cfg(feature = "ble-bringup-c6")]
const BLE_START_DELAY: Duration = Duration::from_secs(3);
// BLE needs heap for esp-radio's controller + trouble-host's boxed GATT clients/reassemblers; 64 KB
// covers it with margin. Kept off the larger end so the leftover linker `.stack` region stays big
// enough for the BLE construction transient (the single-core main task runs on `.stack` — esp-rtos
// gives it no separate task stack, so RAM spent on the heap is RAM taken from that one stack).
#[cfg(not(any(feature = "ble-bringup-c6", feature = "espnow-c6")))]
const HEAP_BYTES: usize = 32 * 1024;
#[cfg(all(feature = "ble-bringup-c6", not(feature = "espnow-c6")))]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(all(feature = "espnow-c6", not(feature = "ble-bringup-c6")))]
const HEAP_BYTES: usize = 72 * 1024;
#[cfg(all(feature = "espnow-c6", feature = "ble-bringup-c6"))]
const HEAP_BYTES: usize = 88 * 1024;
#[cfg(feature = "ble-bringup-c6")]
fn c6_ble_config() -> esp_radio::ble::Config {
    esp_radio::ble::Config::default()
        .with_task_priority(0)
        .with_task_stack_size(4096)
        .with_max_connections(BLE_CONTROLLER_CONNECTIONS as u16)
        .with_default_tx_power(esp_radio::ble::TxPower::P20)
}

const LANE_DEPTH: usize = 1;
const USB_SLOT: usize = 0;
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"hopsp-c6");
#[cfg(feature = "espnow-c6")]
const ESPNOW_SLOT: usize = USB_LANE;
#[cfg(feature = "ble-bringup-c6")]
const BLE_FLEET_SLOT: usize = USB_LANE + ESPNOW_LANE;
#[cfg(feature = "ble-bringup-c6")]
const BLE_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);

type Mtx = CriticalSectionRawMutex;
type ReactorInbound = HVec<
    (
        InterfaceId,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type ReactorEgressLanes = HVec<
    (
        InterfaceId,
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type LaneBuf = [FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>; LANE_DEPTH];
type LaneChannel = zerocopy_channel::Channel<'static, Mtx, FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>>;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
#[cfg(feature = "ble-bringup-c6")]
type C6BleFleet = Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP>;
type Node = Prns<
    (),
    (),
    for<'a> fn(PrnsEvent<'a>, &()),
    EngineStorageType,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    IFACES,
    MAX_IFACES,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;

const EMPTY_SLOT: FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN> = FrameSlot::empty();
const FREE_SLOT: InterfaceId = InterfaceId::new([0xff; 8]);

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_COUNTS: EmbassyInterfaceStore<Mtx, STORE_CAP> = EmbassyInterfaceStore::new();
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);
#[cfg(feature = "ble-bringup-c6")]
static BLE_SHARED: BluetoothAutoShared<BLE_MEMBERS> = BluetoothAutoShared::new(BLE_FLEET_ID);
#[cfg(feature = "ble-bringup-c6")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}

fn seeded_entropy(bytes: &mut [u8]) {
    let mut state = ENTROPY_STATE.load(Ordering::Relaxed);
    for byte in bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    ENTROPY_STATE.store(state, Ordering::Relaxed);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

fn c6_secret_key(mac: &MacAddress) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    for (i, byte) in mac.as_bytes().iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

#[embassy_executor::task]
async fn usb_device_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    seam: UsbSeam,
) {
    let mut last_sof = 0u16;
    let host_present = move || {
        let frame = USB_DEVICE::regs()
            .fram_num()
            .read()
            .sof_frame_index()
            .bits();
        let advanced = frame != last_sof;
        last_sof = frame;
        advanced
    };
    let device = UsbAutoDevice::new(USB_INTERFACE_ID, rx, tx, &USB_STATUS, host_present);
    device.run(seam).await
}

#[cfg(feature = "espnow-c6")]
const ESPNOW_SEND_RETRIES: u8 = 8;
#[cfg(feature = "espnow-c6")]
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);

#[cfg(feature = "espnow-c6")]
const fn espnow_phy_rate() -> WifiPhyRate {
    WifiPhyRate::Rate6m
}

#[cfg(feature = "ble-bringup-c6")]
#[embassy_executor::task]
async fn ble_task(
    spawner: Spawner,
    bt: BT<'static>,
    mac: [u8; 6],
    fleet: C6BleFleet,
    shared: &'static BluetoothAutoShared<BLE_MEMBERS>,
) {
    Timer::after(BLE_START_DELAY).await;
    let connector =
        esp_radio::ble::controller::BleConnector::new(bt, c6_ble_config()).expect("ble connector");
    crate::ble::run(connector, mac, fleet, shared, spawner).await;
}

#[cfg(feature = "espnow-c6")]
fn espnow_channel_policy() -> ChannelPolicy {
    ChannelPolicy::Fixed(EspNowChannel::DEFAULT)
}

#[cfg(feature = "espnow-c6")]
struct EspNowAdapter {
    manager: EspNowManager<'static>,
    sender: EspNowSender<'static>,
    receiver: EspNowReceiver<'static>,
    rate_applied: bool,
}

#[cfg(feature = "espnow-c6")]
impl EspNowAdapter {
    fn new(esp_now: EspNow<'static>) -> Self {
        let (manager, sender, receiver) = esp_now.split();
        Self {
            manager,
            sender,
            receiver,
            rate_applied: false,
        }
    }

    fn ensure_rate(&mut self) {
        if !self.rate_applied {
            let _ = self.manager.set_rate(espnow_phy_rate());
            self.rate_applied = true;
        }
    }
}

#[cfg(feature = "espnow-c6")]
impl espnow_core::EspNowRadio for EspNowAdapter {
    fn set_channel(&mut self, channel: EspNowChannel) {
        let _ = self.manager.set_channel(channel.as_u8());
    }

    async fn broadcast(&mut self, frame: &[u8]) -> bool {
        self.ensure_rate();
        for _ in 0..ESPNOW_SEND_RETRIES {
            if self
                .sender
                .send_async(&BROADCAST_ADDRESS, frame)
                .await
                .is_ok()
            {
                return true;
            }
            Timer::after(ESPNOW_SEND_RETRY_DELAY).await;
        }
        false
    }

    async fn receive(&mut self, buf: &mut [u8]) -> usize {
        let frame = self.receiver.receive_async().await;
        let data = frame.data();
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        len
    }
}

pub async fn run(spawner: Spawner) {
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();

    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    let mac = base_mac_address();
    let secret_key = c6_secret_key(&mac);

    let transport_secret = secret_key.clone();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        let destination = personal_rns::routing::announce::derive_destination_hash(
            &signer.identity_hash(),
            &name,
        );
        destination
    };
    #[cfg(feature = "ble-bringup-c6")]
    let mut mac_octets = [0u8; 6];
    #[cfg(feature = "ble-bringup-c6")]
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);

    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );
    let mut inbound: ReactorInbound = HVec::new();
    let mut egress_lanes: ReactorEgressLanes = HVec::new();

    let usb_seam = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        EmbassyInterfaceSeam::new(USB_INTERFACE_ID, in_producer, NOTIFY.sender(), out_consumer)
    };
    spawner.spawn(usb_device_task(usb_rx, usb_tx, usb_seam).expect("usb device task fits"));

    #[cfg(feature = "espnow-c6")]
    let (_espnow_controller, espnow, _espnow_status) = {
        let wifi_config = ControllerConfig::default()
            .with_static_rx_buf_num(4)
            .with_rx_ba_win(3);
        let (controller, interfaces) =
            esp_radio::wifi::new(p.WIFI, wifi_config).expect("wifi controller");
        let esp_now_radio = interfaces.esp_now;
        let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
            EmbassyInterfaceStatus,
            EmbassyInterfaceStatus::new(espnow_core::interface_id(), ConnectionState::Initializing)
        );
        let espnow = EspNowInterface::new(
            EspNowAdapter::new(esp_now_radio),
            espnow_channel_policy(),
            espnow_status,
        );
        (controller, espnow, espnow_status)
    };

    #[cfg(feature = "espnow-c6")]
    let espnow_seam = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        EmbassyInterfaceSeam::new(espnow.id(), in_producer, NOTIFY.sender(), out_consumer)
    };

    #[cfg(feature = "ble-bringup-c6")]
    let ble_fleet: C6BleFleet = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        out_producer.set_outbound_wake(&BLE_OUTBOUND_WAKE);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        Fleet::new(
            MemberWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &BLE_OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };

    let handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, seeded_entropy as fn(&mut [u8]));

    static NODE: StaticCell<Node> = StaticCell::new();
    let node: &'static mut Node = NODE.init(Prns::new(
        PrnsRecipe {
            transport_identity: Some(transport_secret),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                resource_strategy:
                    personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: secret_key,
                announce_app_data: ANNOUNCE_APP_DATA,
                proof: personal_rns::routing::ProofStrategy::ProveAll,
                ratchet: RatchetPolicy::Ratcheted,
            }],
            app_state: (),
            storage: C6Storage,
            routes: personal_rns::routes![],
            interfaces: personal_rns::runtime::Manual,
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    node.activate(USB_SLOT, device_descriptor(USB_INTERFACE_ID));
    #[cfg(feature = "espnow-c6")]
    node.activate(ESPNOW_SLOT, espnow.descriptor());
    #[cfg(feature = "ble-bringup-c6")]
    node.activate_fleet(BLE_FLEET_SLOT, BLE_FLEET_ID);
    node.set_interface_store(&INTERFACE_COUNTS);

    #[cfg(all(feature = "ble-bringup-c6", feature = "espnow-c6"))]
    {
        spawner.spawn(
            ble_task(spawner, p.BT, mac_octets, ble_fleet, &BLE_SHARED).expect("ble task fits"),
        );
        join(node.run_reactor(), espnow.run(espnow_seam)).await;
    }
    #[cfg(all(feature = "espnow-c6", not(feature = "ble-bringup-c6")))]
    {
        join(node.run_reactor(), espnow.run(espnow_seam)).await;
    }
    #[cfg(all(feature = "ble-bringup-c6", not(feature = "espnow-c6")))]
    {
        // Single-core: the reactor and BLE supervisor run on the one executor — where the dual-core
        // S3 hands the reactor to core 1 and runs BLE on core 0.
        spawner.spawn(
            ble_task(spawner, p.BT, mac_octets, ble_fleet, &BLE_SHARED).expect("ble task fits"),
        );
        node.run_reactor().await;
    }
    #[cfg(not(any(feature = "ble-bringup-c6", feature = "espnow-c6")))]
    node.run_reactor().await;
}
