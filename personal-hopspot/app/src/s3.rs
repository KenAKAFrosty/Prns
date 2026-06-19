use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::system::Stack as CpuStack;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select, Either};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::zerocopy_channel;
use embassy_time::{Duration, Ticker, Timer};
use heapless::Vec as HVec;
use portable_atomic::{AtomicU64, Ordering};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use static_cell::{ConstStaticCell, StaticCell};

use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, WifiController,
};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, InstantMillis, InterfaceCounts,
    RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::tcp::impls::embassy::TcpClient;
use personal_rns::interfaces::rns_parity::wifi_auto::core as wifi_core;
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiShared, AutoWifiStatus};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::usb_auto::core::device_descriptor;
use personal_rns::interfaces::usb_auto::impls::embassy::UsbAutoDevice;
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, MacAddress};
use personal_rns::reactor::grant::FrameSlot;
use personal_rns::reactor::impls::embassy_reactor::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    CompletionPool, EmbassyPrnsHandle, Fleet, MemberWire, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::wire::TransportId;

use crate::engine_storage::EngineStorageType;

use personal_hopspot_ui as screen;

esp_app_desc!();

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"hopsp-s3");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot S3")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot S3\xc0";

/// The WiFi network the board joins (station mode), read at build time. Export them (e.g.
/// `source .wifi-env`) before `cargo s3`; an unset SSID leaves WiFi down and the board runs USB-only.
const WIFI_SSID: &str = match option_env!("HOPSPOT_WIFI_SSID") {
    Some(ssid) => ssid,
    None => "",
};
const WIFI_PASSWORD: &str = match option_env!("HOPSPOT_WIFI_PASSWORD") {
    Some(password) => password,
    None => "",
};

/// The LAN Reticulum TCP node the board dials (`ip:port`, e.g. `192.168.1.50:4242`), read at build
/// time like the WiFi creds. Empty (or unparseable) leaves the TCP interface down. No DNS — a
/// resolved address only. Rides the WiFi stack, so it needs WiFi up.
const HOPSPOT_TCP_TARGET: &str = match option_env!("HOPSPOT_TCP_TARGET") {
    Some(target) => target,
    None => "",
};
/// The board's claim about its pipe to the LAN node: it sets the declared MTU tier, which the
/// reactor then clamps to the embedded ceiling. A 2.4 GHz station's honest order of magnitude.
const TCP_BITRATE_BPS: u32 = 65_000_000;
/// One TCP socket's smoltcp rx/tx buffer — sized for the board's frames, DRAM-frugal over throughput.
const TCP_SOCKET_BUF: usize = 1_024;

/// One lane per top-level driver: USB (slot 0), the TCP client (slot 1), and the WiFi supervisor's
/// one shared fleet lane (slot 2). WiFi members do NOT each take a lane — they share slot 2 — so the
/// expensive MTU buffers number three, not three-plus-every-peer.
const IFACES: usize = 3;
/// The WiFi fleet's member budget: how many peers the supervisor carries at once. Each costs only a
/// descriptor + a status slot, never a lane buffer, so it is sized generously.
const MEMBERS: usize = 24;
/// The engine-interface (descriptor + pacer) pool: the two fixed interfaces (USB, TCP) plus the WiFi
/// members. Distinct from the lane count `IFACES` — decoupling them is the whole point of the shared
/// lane, so a generous member budget costs descriptors, not buffers.
const MAX_IFACES: usize = 2 + MEMBERS;
/// The WiFi supervisor's fleet lane (slot 2) key: an `AutoWifi`-kind id, so every `WifiPeer` child
/// routes to this one lane by the kind byte (`lane_serves`). Also the WiFi card's aggregate id.
const WIFI_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
/// The fleet lane's pool slot, after USB (0) and TCP (1).
const WIFI_FLEET_SLOT: usize = 2;
const LANE_DEPTH: usize = 1;
/// Slot 1: the always-on TCP client wire (parallel to USB at slot 0), so the WiFi members never
/// claim it.
const TCP_SLOT: usize = 1;
const NOTIFY_CAP: usize = 16;
const COMMANDS_CAP: usize = 8;
const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

/// Core 1 runs *only* the engine reactor: its future (with the constructed engine) lives in the
/// task pool, so this is just the per-poll execution stack — the run-time ingest crypto's frames.
/// The one-time engine *construction* (the big, dalek-heavy transient) happens on core 0's
/// guarded main-task stack instead, so core 1 stays small.
const CORE1_STACK_BYTES: usize = 32 * 1024;

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const VBAT_EMPTY_MV: u32 = 3300;
const VBAT_FULL_MV: u32 = 4200;
const VBAT_ABSENT_MV: u32 = 3000;

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const RENDER_TICKS_PER_BATTERY: u8 = 4;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

type Mtx = CriticalSectionRawMutex;
type LaneBuf = [FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>; LANE_DEPTH];
type LaneChannel = zerocopy_channel::Channel<'static, Mtx, FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>>;
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
type Handle = EmbassyPrnsHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
/// The fully-spelled node type, so it can ride to core 1 as a concrete `#[task]` argument — which
/// is why `on_event` is a fn pointer and the host's entropy is a fn pointer, not closures.
type S3Node = Prns<
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
/// The free-slot id a pool slot carries until an interface occupies it (never a real medium id).
const FREE_SLOT: InterfaceId = InterfaceId::new([0xff; 8]);

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}

/// The USB interface's live state, written by the device task (core 0) and read by the render loop
/// (core 0) — the engine on core 1 reaches it through the lanes, this `static` is a face-side view.
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);

/// The WiFi supervisor's shared aggregate + per-peer status (written + read on core 0).
static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);

/// The reactor's pool: one inbound + one outbound grant ring per slot, split at boot into the
/// reactor side (core 1's plumbing) and the interface side (core 0's USB seam / fleet wires).
static IN_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
    [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
static IN_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];
static OUT_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
    [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
static OUT_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];

/// The reactor↔interface channels (cross-core via `CriticalSectionRawMutex`).
static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, personal_rns::engine::IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static BUTTON_EVENTS: Channel<Mtx, screen::InputEvent, 4> = Channel::new();

/// The engine's entropy: the hardware TRNG blocks until WiFi RF is live (wifi::new enables it, but
/// the radio is not associated when the engine starts), so entropy is a board-unique software PRNG
/// over this `static` state. Acceptable ONLY because this whole identity is a NEVER-ship bring-up
/// fixture; the long-term fix is to gate the TRNG on RF-up. A fn (not a closure) so the host type
/// stays nameable for the cross-core move.
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

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

/// The recipe's event sink — a fn (not a closure) so the node type stays nameable.
fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

/// Print the allocator's per-region high-water footprint over the boot log: the `External` region's
/// size is the PSRAM the chip mapped (2 MiB vs 8 MiB), its `used` is the live cost of the engine's
/// boxed columns, the `Internal` region is the 56 KiB SRAM heap, and `Max usage` is the high-water
/// across both since boot. Safe only before the USB interface claims the USB-serial-JTAG, so it is a
/// construction-time probe, never a run-loop one.
fn log_heap_footprint(label: &str) {
    println!("[mem] {label}");
    println!("{}", esp_alloc::HEAP.stats());
}

/// Platform bring-up on core 0, where the heavy lifting lives: the OLED, the identity crypto, the
/// whole engine *construction* (its dalek-heavy transient wants the guarded main-task stack), and
/// all the I/O (USB-auto + WiFi-auto). The built node then rides to core 1, which runs only the
/// reactor on a small stack — true parallelism (engine ⊥ I/O) over the cross-core lane channels.
/// Never returns: this frame is core 0's I/O + screen drive.
#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: 56 * 1024);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    // The engine construction allocates + zeroes PSRAM-backed columns synchronously; PSRAM is slow,
    // so it can overrun the RTC watchdog's ~2s timeout. Disable RWDT/SWD over the boot build.
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    println!("HOPSPOT_S3 boot — recipe runtime, engine core 1 + I/O core 0");

    // OLED (Heltec V4: Vext active-low gates panel power; pulse RST; I2C0 on 17/18).
    let mut _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
    let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    rst.set_low();
    Timer::after(Duration::from_millis(20)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(20)).await;
    let i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0")
    .with_sda(p.GPIO17)
    .with_scl(p.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    let oled_ok = display.init().is_ok();
    if oled_ok {
        screen::splash(&mut display, "Personal Hopspot");
        let _ = display.flush();
    }

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    let secret_key = fixture_identity_secret_key(&mac);

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
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );

    let mut inbound: ReactorInbound = HVec::new();
    let mut egress_lanes: ReactorEgressLanes = HVec::new();
    let mut iface_halves: [Option<(
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    )>; IFACES] = [const { None }; IFACES];
    for slot in 0..IFACES {
        let in_ch = IN_CH[slot].init(zerocopy_channel::Channel::new(IN_BUF[slot].take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH[slot].init(zerocopy_channel::Channel::new(OUT_BUF[slot].take()));
        let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

    // The WiFi stack carries both the WiFi-auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    let wifi_built = build_wifi(&spawner, p.WIFI, mac_octets);
    let stack = wifi_built.as_ref().map(|(_, stack)| *stack);
    let wifi = wifi_built.map(|(wifi, _)| wifi);
    let tcp_built = stack.and_then(build_tcp);
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let handle: Handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, seeded_entropy as fn(&mut [u8]));
    static NODE: StaticCell<S3Node> = StaticCell::new();
    let node: &'static mut S3Node = NODE.init(Prns::new(
        PrnsRecipe {
            transport: Some(transport_id),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: secret_key,
                announce_app_data: ANNOUNCE_APP_DATA,
                proof: personal_rns::routing::ProofStrategy::ProveAll,
                ratchet: RatchetPolicy::Ratcheted,
            }],
            app_state: (),
            storage: EngineStorageType::default(),
            routes: personal_rns::routes![],
            interfaces: personal_rns::interfaces![],
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    node.activate(0, device_descriptor(USB_INTERFACE_ID));
    if let Some((tcp, _, _)) = &tcp_built {
        node.activate(TCP_SLOT, tcp.descriptor());
    }
    // The WiFi supervisor's one shared lane: keyed by its AutoWifi id, every WifiPeer member routes
    // to it by kind. Registered before the node moves to core 1; members add their descriptors at
    // runtime through the fleet, never another lane.
    if wifi.is_some() {
        node.activate_fleet(WIFI_FLEET_SLOT, WIFI_FLEET_ID);
    }

    log_heap_footprint("post-construction (engine columns boxed into PSRAM)");

    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(
        p.CPU_CTRL,
        sw_int.software_interrupt1,
        core1_stack,
        move || {
            static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
            EXECUTOR
                .init(esp_rtos::embassy::Executor::new())
                .run(|spawner| {
                    spawner.spawn(reactor_core(node).expect("reactor task fits"));
                })
        },
    );

    let (usb_in_producer, usb_out_consumer) = iface_halves[0].take().expect("slot 0 half");
    let usb_seam = EmbassyInterfaceSeam::new(
        USB_INTERFACE_ID,
        usb_in_producer,
        NOTIFY.sender(),
        usb_out_consumer,
    );
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();
    let usb_device = usb_device(usb_rx, usb_tx);

    let tcp = tcp_built.map(|(tcp, _, _)| {
        let (in_producer, out_consumer) = iface_halves[TCP_SLOT].take().expect("tcp slot half");
        let seam = EmbassyInterfaceSeam::new(tcp.id(), in_producer, NOTIFY.sender(), out_consumer);
        (tcp, seam)
    });

    // The whole WiFi fleet shares slot 2's one lane: the supervisor funnels every peer's frames
    // through it, tagged by the peer's id, and the reactor demuxes by kind. Members are descriptors,
    // not lanes — so no per-peer wire is taken here.
    let (wifi_in_producer, wifi_out_consumer) = iface_halves[WIFI_FLEET_SLOT]
        .take()
        .expect("wifi fleet half");
    let fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = Fleet::new(
        MemberWire {
            inbound: wifi_in_producer,
            outbound: wifi_out_consumer,
            notify: NOTIFY.sender(),
        },
        LIFECYCLE.sender(),
    );

    let button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits"));

    // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by ADC_Ctrl on GPIO37.
    let mut adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
    adc_ctrl.set_high();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(p.ADC1, adc_cfg);

    let wifi_status = wifi.as_ref().map(AutoWifi::status);
    let wifi_id = wifi_status.as_ref().map(|status| {
        use personal_rns::interfaces::InterfaceStatus;
        status.id()
    });

    let render = async move {
        let mut ui_state = screen::UiState::new();
        let mut vbat_ema_mv: u32 = 0;
        let mut battery_state = screen::BatteryState::Unknown;
        let mut ticks_to_battery: u8 = 0;
        let mut render_tick = Ticker::every(RENDER_INTERVAL);
        loop {
            if ticks_to_battery == 0 {
                let mut pin_mv = 0u16;
                for _ in 0..1000 {
                    if let Ok(value) = vbat_adc.read_oneshot(&mut vbat_pin) {
                        pin_mv = value;
                        break;
                    }
                }
                let vbat_mv = pin_mv as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
                battery_state = if vbat_mv < VBAT_ABSENT_MV {
                    screen::BatteryState::Unknown
                } else {
                    vbat_ema_mv = if vbat_ema_mv == 0 {
                        vbat_mv
                    } else {
                        (vbat_ema_mv * 7 + vbat_mv) / 8
                    };
                    let span = VBAT_FULL_MV - VBAT_EMPTY_MV;
                    let pct =
                        (vbat_ema_mv.saturating_sub(VBAT_EMPTY_MV) * 100 / span).min(100) as u8;
                    screen::BatteryState::Level(pct)
                };
                ticks_to_battery = RENDER_TICKS_PER_BATTERY;
            }
            ticks_to_battery -= 1;

            let cards = build_cards(
                &handle,
                &USB_STATUS,
                wifi_status.as_ref(),
                wifi_id,
                tcp_status,
                tcp_id,
            )
            .await;
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);
            if oled_ok {
                screen::draw_with_state(&mut display, &cards, battery_state, &ui_state);
                let _ = display.flush();
            }

            match select(render_tick.next(), BUTTON_EVENTS.receive()).await {
                Either::First(()) => {}
                Either::Second(event) => match ui_state.handle_input(event, card_count) {
                    screen::UiAction::Announce => {
                        let _ = handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                            destination: self_destination,
                            target: AnnounceTarget::AllInterfaces,
                            app_data: AnnounceAppData::Registered,
                        }));
                    }
                    screen::UiAction::ToggleSelectedInterface => {
                        if let Some(card) = ui_state
                            .selected_card(card_count)
                            .and_then(|index| cards.get(index))
                        {
                            if card.id == USB_INTERFACE_ID {
                                USB_STATUS.set_enabled(!USB_STATUS.is_enabled());
                            } else if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id) {
                                if card.id == tcp_id {
                                    tcp.set_enabled(!tcp.is_enabled());
                                }
                            }
                        }
                    }
                    screen::UiAction::None => {}
                },
            }
        }
    };

    match (wifi, tcp) {
        (Some(wifi), Some((tcp, tcp_seam))) => {
            join(
                join(
                    join(usb_device.run(usb_seam), wifi.run(fleet)),
                    tcp.run(tcp_seam),
                ),
                render,
            )
            .await;
        }
        (Some(wifi), None) => {
            join(join(usb_device.run(usb_seam), wifi.run(fleet)), render).await;
        }
        (None, _) => {
            join(usb_device.run(usb_seam), render).await;
        }
    }
}

/// Core 1: run only the engine reactor over the slot pool. The node was built on core 0 and lives in
/// a `static`; core 1 borrows it by `&'static mut`, so only a pointer crosses the core boundary (the
/// engine never moves) and this core needs just a small per-poll stack for the ingest crypto.
#[embassy_executor::task]
async fn reactor_core(node: &'static mut S3Node) {
    node.run_reactor().await
}

/// Build the card set: the USB host, the WiFi aggregate, and one card per confirmed peer —
/// classified into USB / WiFi / `Peer <hex>`, the same shape the desktop face renders.
async fn build_cards(
    handle: &Handle,
    usb: &EmbassyInterfaceStatus,
    wifi: Option<&AutoWifiStatus<MEMBERS>>,
    wifi_id: Option<InterfaceId>,
    tcp: Option<&EmbassyInterfaceStatus>,
    tcp_id: Option<InterfaceId>,
) -> HVec<screen::Card, 8> {
    use personal_rns::interfaces::InterfaceStatus;
    let classify = |id: InterfaceId| -> Option<(screen::CardKind, screen::CardLabel)> {
        if id == USB_INTERFACE_ID {
            Some((screen::CardKind::Usb, screen::card_label("USB")))
        } else if Some(id) == wifi_id {
            Some((screen::CardKind::Wifi, screen::card_label("WiFi")))
        } else if Some(id) == tcp_id {
            Some((
                screen::CardKind::Tcp,
                screen::tcp_card_label(HOPSPOT_TCP_TARGET),
            ))
        } else {
            let bytes = id.as_bytes();
            let mut label = screen::CardLabel::new();
            let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
            Some((screen::CardKind::Peer, label))
        }
    };
    let mut statuses: HVec<&dyn InterfaceStatus, 8> = HVec::new();
    let _ = statuses.push(usb);
    if let Some(tcp) = tcp {
        let _ = statuses.push(tcp);
    }
    if let Some(wifi) = wifi {
        let _ = statuses.push(wifi);
        for member in wifi.members() {
            let _ = statuses.push(member);
        }
    }
    let mut counts: HVec<(InterfaceId, InterfaceCounts), 8> = HVec::new();
    for status in &statuses {
        let id = status.id();
        let _ = counts.push((id, handle.interface_counts(id).await.unwrap_or_default()));
    }
    let lookup = |id: InterfaceId| {
        counts
            .iter()
            .find(|(cid, _)| *cid == id)
            .map_or_else(InterfaceCounts::default, |(_, c)| *c)
    };
    let wifi_counts = wifi.map(|wifi| {
        wifi.members()
            .fold(InterfaceCounts::default(), |total, member| {
                let member_counts = lookup(member.id());
                InterfaceCounts {
                    destinations: total.destinations + member_counts.destinations,
                    links: total.links + member_counts.links,
                }
            })
    });
    screen::statuses_to_cards(&statuses, classify, |id| {
        if Some(id) == wifi_id {
            wifi_counts.unwrap_or_default()
        } else {
            lookup(id)
        }
    })
}

/// The board's concrete USB-auto device over the serial-jtag halves, reporting into [`USB_STATUS`].
fn usb_device(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
) -> UsbAutoDevice<
    'static,
    UsbSerialJtagRx<'static, Async>,
    UsbSerialJtagTx<'static, Async>,
    impl FnMut() -> bool,
> {
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
    UsbAutoDevice::new(USB_INTERFACE_ID, rx, tx, &USB_STATUS, host_present)
}

/// Stand the TCP client up from [`HOPSPOT_TCP_TARGET`] over the WiFi `stack`: parse its `ip:port`
/// (unset or unparseable leaves it down), mint the interface id and its status under the same key,
/// and lease the socket's smoltcp buffers from `static`s. Hands back the interface, its status
/// handle (the render reads it for the card), and its id (the classifier names it).
fn build_tcp(
    stack: Stack<'static>,
) -> Option<(
    TcpClient<'static>,
    &'static EmbassyInterfaceStatus,
    InterfaceId,
)> {
    let addr = HOPSPOT_TCP_TARGET.parse::<::core::net::SocketAddr>().ok()?;
    let target = IpEndpoint::new(addr.ip().into(), addr.port());
    let tag = HOPSPOT_TCP_TARGET.as_bytes();
    let id = TcpClient::interface_id(tag);
    let status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(id, ConnectionState::Initializing)
    );
    let rx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
    let tx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
    let tcp = TcpClient::new(
        stack,
        target,
        tag,
        TCP_BITRATE_BPS,
        Duration::from_secs(5),
        rx_buffer,
        tx_buffer,
        status,
    );
    Some((tcp, status, id))
}

/// Bring the WiFi stack up in station mode and hand back the supervisor. `None` with no SSID (the
/// board then runs USB-only). Spawns the net runner + the connect/reconnect loop on core 0.
fn build_wifi(
    spawner: &Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    mac: [u8; 6],
) -> Option<(AutoWifi<'static, MEMBERS>, Stack<'static>)> {
    if WIFI_SSID.is_empty() {
        return None;
    }
    let (controller, interfaces) = esp_radio::wifi::new(wifi, ControllerConfig::default()).ok()?;

    let link_local = wifi_core::link_local_from_mac(MacAddress::new(mac));
    // Dual-stack: the v6 link-local carries WiFi-auto's discovery/data UDP (peer-to-peer on the
    // segment); v4 over DHCP gives the board a routable address to dial a Reticulum TCP node by
    // ip:port.
    let mut net_config = NetConfig::dhcpv4(DhcpConfig::default());
    net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
        address: Ipv6Cidr::new(link_local, 64),
        gateway: None,
        dns_servers: Default::default(),
    });
    let resources = mk_static!(StackResources<6>, StackResources::new());
    let seed = {
        let mut bytes = [0u8; 8];
        Rng::new().read(&mut bytes);
        u64::from_le_bytes(bytes)
    };
    let (stack, runner) = embassy_net::new(interfaces.station, net_config, resources, seed);

    let discovery = {
        static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
            ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
        static RX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
        static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
            ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
        static TX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
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
        static RX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
        static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
            ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
        static TX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
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
    Some((
        AutoWifi::new(stack, discovery, data, mac, &WIFI_SHARED),
        stack,
    ))
}

/// Drive the embassy-net stack forever (the link/neighbor/socket machinery), on core 0.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

/// Join the configured network in station mode and hold the association up, reconnecting on drop.
///
/// A mesh (e.g. eero) hands the same SSID out on many BSSIDs across its nodes and bands and bridges
/// multicast between them unreliably, so a station left to roam can land on a node that never
/// receives the discovery group. To avoid that, this scans first and pins to the strongest BSSID
/// for the SSID — landing the S3 on one node and holding it there, where the discovery multicast
/// reaches it.
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
                "wifi: pinned to BSSID {:02x?} channel {} (rssi {})",
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

/// A bring-up fixture identity (the oracle X25519 0x22 ‖ Ed25519 0x11 keypair with the board MAC
/// mixed in so every flashed board is distinct). NEVER ship: predictable from the MAC.
fn fixture_identity_secret_key(
    mac: &esp_hal::efuse::MacAddress,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    for (i, byte) in mac.as_bytes().iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

/// The user button worker (core 0): turn raw active-low edges on GPIO0 into the same
/// [`InputEvent`](screen::InputEvent)s the desktop face produces.
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        match embassy_futures::select::select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            embassy_futures::select::Either::First(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::ShortPress).await
            }
            embassy_futures::select::Either::Second(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}
