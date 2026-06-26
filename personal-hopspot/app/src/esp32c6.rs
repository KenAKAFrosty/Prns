//! The Seeed XIAO ESP32-C6 Hopspot board: single-core, no-PSRAM, headless. The engine is constructed
//! and run on the one RISC-V core (no second-core hand-off like the S3), with its columns inline in
//! internal SRAM. USB-Serial-JTAG carries diagnostics only — there is no USB network interface. First
//! light brings the node up with zero interfaces and a heartbeat log; BLE is the first real interface.

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::{base_mac_address, MacAddress};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
#[cfg(not(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6")))]
use embassy_futures::join::join;
#[cfg(any(
    all(feature = "ble-bringup-c6", not(feature = "wifi-bringup-c6")),
    all(feature = "wifi-bringup-c6", not(feature = "ble-bringup-c6"))
))]
use embassy_futures::join::join3;
#[cfg(all(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use embassy_futures::join::join4;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use heapless::Vec as HVec;
use portable_atomic::{AtomicU64, Ordering};
use static_cell::StaticCell;

use personal_rns::engine::{InstantMillis, IssuedCommand, RatchetPolicy};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::embassy_reactor::{
    EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbassyPrnsHandle, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::wire::TransportId;

use crate::engine_storage::{C6Storage, EngineStorageType};

#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use embassy_sync::signal::Signal;
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use embassy_sync::zerocopy_channel;
#[cfg(feature = "ble-bringup-c6")]
use personal_rns::interfaces::bluetooth_auto::BluetoothAutoShared;
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use personal_rns::interfaces::InterfaceKind;
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use personal_rns::reactor::grant::FrameSlot;
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use personal_rns::reactor::impls::embassy_reactor::embassy_grant_lane;
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use personal_rns::runtime::{Fleet, MemberWire};
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
use static_cell::ConstStaticCell;

#[cfg(feature = "wifi-bringup-c6")]
use embassy_net::udp::{PacketMetadata, UdpSocket};
#[cfg(feature = "wifi-bringup-c6")]
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, Ipv6Cidr, Runner, StackResources, StaticConfigV6,
};
#[cfg(feature = "wifi-bringup-c6")]
use esp_hal::rng::Rng;
#[cfg(feature = "wifi-bringup-c6")]
use esp_radio::wifi::scan::ScanConfig;
#[cfg(feature = "wifi-bringup-c6")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "wifi-bringup-c6")]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, WifiController,
};
#[cfg(feature = "wifi-bringup-c6")]
use personal_rns::interfaces::rns_parity::wifi_auto::core as wifi_core;
#[cfg(feature = "wifi-bringup-c6")]
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiShared};
#[cfg(feature = "wifi-bringup-c6")]
use personal_rns::interfaces::MacAddress as RnsMac;

esp_app_desc!();

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";

const WIFI_LANE: usize = cfg!(feature = "wifi-bringup-c6") as usize;
const BLE_LANE: usize = cfg!(feature = "ble-bringup-c6") as usize;
const FLEET_COUNT: usize = WIFI_LANE + BLE_LANE;
const IFACES: usize = if FLEET_COUNT == 0 { 1 } else { FLEET_COUNT };
pub const BLE_MEMBERS: usize = 2;
const WIFI_MEMBERS: usize = 4;
const MAX_IFACES: usize = IFACES + BLE_LANE * BLE_MEMBERS + WIFI_LANE * WIFI_MEMBERS + 1;
pub const NOTIFY_CAP: usize = 16;
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 16;
const COMPLETIONS_CAP: usize = 4;
const STORE_CAP: usize = 16;
// BLE needs heap for esp-radio's controller + trouble-host's boxed GATT clients/reassemblers; 64 KB
// covers it with margin. Kept off the larger end so the leftover linker `.stack` region stays big
// enough for the BLE construction transient (the single-core main task runs on `.stack` — esp-rtos
// gives it no separate task stack, so RAM spent on the heap is RAM taken from that one stack).
#[cfg(not(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6")))]
const HEAP_BYTES: usize = 32 * 1024;
#[cfg(all(feature = "ble-bringup-c6", not(feature = "wifi-bringup-c6")))]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(all(feature = "wifi-bringup-c6", not(feature = "ble-bringup-c6")))]
const HEAP_BYTES: usize = 72 * 1024;
#[cfg(all(feature = "wifi-bringup-c6", feature = "ble-bringup-c6"))]
const HEAP_BYTES: usize = 96 * 1024;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1000);

#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
const LANE_DEPTH: usize = 1;
#[cfg(feature = "wifi-bringup-c6")]
const WIFI_FLEET_SLOT: usize = 0;
#[cfg(feature = "wifi-bringup-c6")]
const WIFI_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
#[cfg(feature = "ble-bringup-c6")]
const BLE_FLEET_SLOT: usize = WIFI_LANE;
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
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
type LaneBuf = [FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>; LANE_DEPTH];
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
type LaneChannel = zerocopy_channel::Channel<'static, Mtx, FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>>;
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

#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
const EMPTY_SLOT: FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN> = FrameSlot::empty();
#[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
const FREE_SLOT: InterfaceId = InterfaceId::new([0xff; 8]);

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_COUNTS: EmbassyInterfaceStore<Mtx, STORE_CAP> = EmbassyInterfaceStore::new();
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
#[cfg(feature = "ble-bringup-c6")]
static BLE_SHARED: BluetoothAutoShared<BLE_MEMBERS> = BluetoothAutoShared::new(BLE_FLEET_ID);
#[cfg(feature = "ble-bringup-c6")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
#[cfg(feature = "wifi-bringup-c6")]
static WIFI_SHARED: AutoWifiShared<WIFI_MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);
#[cfg(feature = "wifi-bringup-c6")]
static WIFI_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

#[cfg(feature = "wifi-bringup-c6")]
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

#[cfg(feature = "wifi-bringup-c6")]
const WIFI_SSID: &str = match option_env!("HOPSPOT_WIFI_SSID") {
    Some(s) => s,
    None => "",
};
#[cfg(feature = "wifi-bringup-c6")]
const WIFI_PASSWORD: &str = match option_env!("HOPSPOT_WIFI_PASSWORD") {
    Some(s) => s,
    None => "",
};

#[cfg(feature = "wifi-bringup-c6")]
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

#[cfg(feature = "wifi-bringup-c6")]
#[embassy_executor::task]
async fn wifi_connect_task(mut controller: WifiController<'static>) -> ! {
    let base = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());
    let _ = controller.set_config(&WifiConfig::Station(base.clone()));
    let mut station = base.clone();
    if let Ok(networks) = controller.scan_async(&ScanConfig::default()).await {
        let mut best: Option<([u8; 6], u8, i8)> = None;
        for ap in &networks {
            if ap.ssid.as_str() == WIFI_SSID
                && best.is_none_or(|(_, _, rssi)| ap.signal_strength > rssi)
            {
                best = Some((ap.bssid, ap.channel, ap.signal_strength));
            }
        }
        if let Some((bssid, channel, rssi)) = best {
            log::info!(
                "wifi: pinned BSSID {:02x?} ch {} (rssi {})",
                bssid,
                channel,
                rssi
            );
            station = base.clone().with_bssid(bssid).with_channel(channel);
        }
    }
    let config = WifiConfig::Station(station);
    loop {
        if controller.is_connected() {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        if controller.set_config(&config).is_err() {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        if controller.connect_async().await.is_err() {
            Timer::after(Duration::from_secs(2)).await;
        }
    }
}

pub async fn run(spawner: Spawner) {
    #[cfg(not(feature = "wifi-bringup-c6"))]
    let _ = spawner;
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    println!("HOPSPOT_XIAO_C6 boot — single-core RISC-V reactor, headless (USB = diagnostics)");

    let mac = base_mac_address();
    let secret_key = c6_secret_key(&mac);

    let (self_destination, transport_id) = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        let destination = personal_rns::routing::announce::derive_destination_hash(
            &signer.identity_hash(),
            &name,
        );
        let transport = TransportId::new(*signer.identity_hash().as_bytes());
        (destination, transport)
    };
    #[cfg(feature = "ble-bringup-c6")]
    let node_identity: [u8; 16] = *transport_id.as_bytes();
    #[cfg(feature = "ble-bringup-c6")]
    let mut mac_octets = [0u8; 6];
    #[cfg(feature = "ble-bringup-c6")]
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    #[cfg(feature = "wifi-bringup-c6")]
    let mut wifi_mac = [0u8; 6];
    #[cfg(feature = "wifi-bringup-c6")]
    wifi_mac.copy_from_slice(&mac.as_bytes()[..6]);

    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );
    println!(
        "mac={:02x?} lxmf.delivery={:02x?} transport={:02x?}",
        mac.as_bytes(),
        self_destination.as_bytes(),
        transport_id.as_bytes(),
    );

    #[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
    let mut inbound: ReactorInbound = HVec::new();
    #[cfg(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
    let mut egress_lanes: ReactorEgressLanes = HVec::new();
    #[cfg(not(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6")))]
    let inbound: ReactorInbound = HVec::new();
    #[cfg(not(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6")))]
    let egress_lanes: ReactorEgressLanes = HVec::new();

    #[cfg(feature = "wifi-bringup-c6")]
    let (wifi, wifi_data_buf) = {
        let wifi_config = ControllerConfig::default()
            .with_static_rx_buf_num(4)
            .with_rx_ba_win(3);
        let (mut controller, interfaces) =
            esp_radio::wifi::new(p.WIFI, wifi_config).expect("wifi controller");
        let _ = controller.set_config(&WifiConfig::Station(StationConfig::default()));

        let link_local = wifi_core::link_local_from_mac(RnsMac::new(wifi_mac));
        let mut net_config = NetConfig::dhcpv4(DhcpConfig::default());
        net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
            address: Ipv6Cidr::new(link_local, 64),
            gateway: None,
            dns_servers: Default::default(),
        });
        let resources = mk_static!(StackResources<4>, StackResources::new());
        let seed = {
            let mut bytes = [0u8; 8];
            Rng::new().read(&mut bytes);
            u64::from_le_bytes(bytes)
        };
        let (stack, runner) = embassy_net::new(interfaces.station, net_config, resources, seed);
        let discovery = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
            UdpSocket::new(
                stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let data = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
            UdpSocket::new(
                stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        spawner.spawn(net_task(runner).expect("net task fits"));
        spawner.spawn(wifi_connect_task(controller).expect("wifi connect task fits"));
        let wifi: AutoWifi<'static, WIFI_MEMBERS> =
            AutoWifi::new(stack, discovery, data, wifi_mac, &WIFI_SHARED);
        let data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_core::HARDWARE_MTU].leak();
        (wifi, data_buf)
    };

    #[cfg(feature = "wifi-bringup-c6")]
    let wifi_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
        static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
        static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
        let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        out_producer.set_outbound_wake(&WIFI_OUTBOUND_WAKE);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        Fleet::new(
            MemberWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &WIFI_OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };

    #[cfg(feature = "ble-bringup-c6")]
    let ble_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
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
            transport: Some(transport_id),
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
            interfaces: personal_rns::interfaces![],
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    #[cfg(feature = "wifi-bringup-c6")]
    node.activate_fleet(WIFI_FLEET_SLOT, WIFI_FLEET_ID);
    #[cfg(feature = "ble-bringup-c6")]
    node.activate_fleet(BLE_FLEET_SLOT, BLE_FLEET_ID);
    node.set_interface_store(&INTERFACE_COUNTS);

    println!("[mem] post-construction (engine columns inline in SRAM, no PSRAM)");
    println!("{}", esp_alloc::HEAP.stats());

    let heartbeat = async {
        let mut tick: u32 = 0;
        loop {
            Timer::after(HEARTBEAT_INTERVAL).await;
            tick += 1;
            println!("c6 hb tick={tick} free_heap={}", esp_alloc::HEAP.free());
        }
    };

    #[cfg(all(feature = "ble-bringup-c6", feature = "wifi-bringup-c6"))]
    {
        let ble_connector = esp_radio::ble::controller::BleConnector::new(
            p.BT,
            esp_radio::ble::Config::default().with_task_stack_size(4096),
        )
        .expect("ble connector");
        let mut wifi_sec: [u8; 0] = [];
        join4(
            node.run_reactor(),
            crate::ble::run(
                ble_connector,
                mac_octets,
                node_identity,
                ble_fleet,
                &BLE_SHARED,
            ),
            wifi.run(wifi_fleet, wifi_data_buf, &mut wifi_sec),
            heartbeat,
        )
        .await;
    }
    #[cfg(all(feature = "wifi-bringup-c6", not(feature = "ble-bringup-c6")))]
    {
        let mut wifi_sec: [u8; 0] = [];
        join3(
            node.run_reactor(),
            wifi.run(wifi_fleet, wifi_data_buf, &mut wifi_sec),
            heartbeat,
        )
        .await;
    }
    #[cfg(all(feature = "ble-bringup-c6", not(feature = "wifi-bringup-c6")))]
    {
        let ble_connector = esp_radio::ble::controller::BleConnector::new(
            p.BT,
            esp_radio::ble::Config::default().with_task_stack_size(4096),
        )
        .expect("ble connector");
        // Single-core: the reactor, the BLE supervisor (ble::run), and the heartbeat all run on the one
        // executor — where the dual-core S3 hands the reactor to core 1 and runs BLE on core 0.
        join3(
            node.run_reactor(),
            crate::ble::run(
                ble_connector,
                mac_octets,
                node_identity,
                ble_fleet,
                &BLE_SHARED,
            ),
            heartbeat,
        )
        .await;
    }
    #[cfg(not(any(feature = "ble-bringup-c6", feature = "wifi-bringup-c6")))]
    join(node.run_reactor(), heartbeat).await;
}
