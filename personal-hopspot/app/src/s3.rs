use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::{Rtc, RwdtStage};
use esp_hal::system::Stack as CpuStack;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

use core::sync::atomic::{AtomicUsize, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_net::{Ipv6Cidr, Runner, Stack, StackResources, StaticConfigV6};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{Config, Interface, WifiController};
use heapless::Vec as HVec;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use static_cell::StaticCell;

use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::RatchetPolicy;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, InstantMillis,
    IssuedCommand, ReannounceSchedule, SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::rns_parity::auto_interface::{self, link_local_from_mac};
use personal_rns::interfaces::impls::usb_auto::core::{device_descriptor, NodeTag, MAX_DATA_BYTES};
use personal_rns::interfaces::impls::usb_auto::serve;
use personal_rns::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
use personal_rns::interfaces::substrate::{
    new_wake_signal, EmbassyHostSubstrate, EmbassyInterfaceChannels, EmbassyInterfaceHandle,
    EmbassyInterfaceSeam, EmbassyTimebase, WakeSignal,
};
use personal_rns::interfaces::{
    InterfaceId, InterfaceWorkerContext, MacAddress, SelfDrivenInterface, StartedInterface,
};
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::storage::FixedInline;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::channels::embassy::RuntimeSnapshotWatch;
use personal_rns::runtime::host::impls::EmbassyContractHost;
use personal_rns::runtime::{Prns, PrnsEvent, Recipe, RuntimeSnapshot, StartingDestinationConfig};
use personal_rns::wire::TransportId;

use personal_hopspot_ui as screen;

esp_app_desc!();

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"prsnl-hopspot-s3");
const WIFI_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"prsnl-hopspot-wf");
const PER_INTERFACE_MAX_BUFFERED_PACKETS: usize = 8;

/// WiFi credentials, injected at compile time and kept out of the repo. Export
/// `HOPSPOT_WIFI_SSID` / `HOPSPOT_WIFI_PASSWORD` before building; absent ones leave
/// the station unconfigured (the connection task just keeps retrying).
const WIFI_SSID: &str = match option_env!("HOPSPOT_WIFI_SSID") {
    Some(value) => value,
    None => "",
};
const WIFI_PASSWORD: &str = match option_env!("HOPSPOT_WIFI_PASSWORD") {
    Some(value) => value,
    None => "",
};

/// Place a value in a `'static` `StaticCell` and hand back the `'static` reference —
/// the embassy idiom for giving the radio controller and net stack 'static lifetimes
/// without a heap allocation.
macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}
const ENGINE_STORAGE: FixedInline<24, 32, 1024, 4, 128, 4, 4, 4, 4, 32, 8, 8, 8> = FixedInline;

/// The engine's own stack on core 1 — sized from the painted watermark: the
/// measured peak is 69.1KB, and it is the boot spawn (embassy constructs the
/// ~68KB engine_task future here before moving it into the task pool), not
/// crypto — every dalek sign/verify ran beneath it. Re-measure via BUDGETS
/// whenever the engine grows.
const CORE1_STACK_BYTES: usize = 76 * 1024;

const STACK_PAINT_WORD: u32 = 0x57AC_C0DE;
const STACK_GUARD_SKIP_BYTES: usize = 64;
const CORE1_ENTRY_BLOB_SKIP_BYTES: usize = 2048;
const STACK_PAINT_SP_MARGIN_BYTES: usize = 2048;
const BUDGET_REPORT_EVERY: Duration = Duration::from_secs(20);

struct PaintedStack {
    floor: AtomicUsize,
    top: AtomicUsize,
}

static CORE0_STACK: PaintedStack = PaintedStack::unpainted();
static CORE1_STACK: PaintedStack = PaintedStack::unpainted();

impl PaintedStack {
    const fn unpainted() -> Self {
        Self {
            floor: AtomicUsize::new(0),
            top: AtomicUsize::new(0),
        }
    }

    unsafe fn paint(&self, floor: usize, paint_top: usize, true_top: usize) {
        critical_section::with(|_| {
            let mut addr = floor;
            while addr + 4 <= paint_top {
                (addr as *mut u32).write_volatile(STACK_PAINT_WORD);
                addr += 4;
            }
        });
        self.floor.store(floor, Ordering::Release);
        self.top.store(true_top, Ordering::Release);
    }

    fn peak_bytes(&self) -> usize {
        let floor = self.floor.load(Ordering::Acquire);
        let top = self.top.load(Ordering::Acquire);
        if floor == 0 {
            return 0;
        }
        let mut addr = floor;
        while addr + 4 <= top {
            if unsafe { (addr as *const u32).read_volatile() } != STACK_PAINT_WORD {
                return top - addr;
            }
            addr += 4;
        }
        0
    }

    fn span_bytes(&self) -> usize {
        self.top.load(Ordering::Acquire) - self.floor.load(Ordering::Acquire)
    }
}

fn core0_stack_bounds() -> (usize, usize) {
    extern "C" {
        static _stack_end: u32;
        static _stack_start: u32;
    }
    (
        core::ptr::addr_of!(_stack_end) as usize,
        core::ptr::addr_of!(_stack_start) as usize,
    )
}

type EngineInterfaces = FixedInterfaceSet<
    StartedInterface<EmbassyInterfaceHandle<MAX_DATA_BYTES>, core::convert::Infallible>,
    2,
>;

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot S3")` ‖ `nil` — the shape LXMF apps parse
/// (`\x13` = 19 = the name's length), so they surface the display name.
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot S3\xc0";

/// Heltec V4 VBAT sense: the on-board divider is ~4.9x ((390k+100k)/100k), so
/// VBAT(mV) = pin(mV) * 49 / 10.
const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;

/// LiPo range for the bar fill (datasheet: 3.3 V empty … 4.2 V full).
const VBAT_EMPTY_MV: u32 = 3300;
const VBAT_FULL_MV: u32 = 4200;
/// Below this no connected LiPo is plausible (USB with no battery reads ~0), so show
/// `Unknown` rather than misleading bars.
const VBAT_ABSENT_MV: u32 = 3000;

/// Blank the OLED after this long with no Reticulum activity; it wakes the instant
/// traffic resumes. `None` keeps the panel always on, so a dark screen can only
/// mean a reset, reflash, or crash — never idleness. (Battery/field builds want
/// `Some(secs)` back.)
const OLED_IDLE_BLANK_SECS: Option<u64> = None;

/// Hold the user button at least this long for a long press (open/close a menu);
/// anything shorter is a tap that advances focus. Matches the desktop face's threshold.
const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
/// Settle time after each press, so the contact's release bounce isn't a fresh press.
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

/// The worker-side seam this board's responder task runs against.
type UsbAutoContext = InterfaceWorkerContext<
    EmbassyHostSubstrate<MAX_DATA_BYTES, PER_INTERFACE_MAX_BUFFERED_PACKETS>,
>;

/// The interface's four channels live in one board `static` (the embassy idiom);
/// `attach` splits the worker + runtime ends out of it — no heap.
static CHANNELS: EmbassyInterfaceChannels<MAX_DATA_BYTES, PER_INTERFACE_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
static WIFI_CHANNELS: EmbassyInterfaceChannels<MAX_DATA_BYTES, PER_INTERFACE_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();

type WifiAutoContext = InterfaceWorkerContext<
    EmbassyHostSubstrate<MAX_DATA_BYTES, PER_INTERFACE_MAX_BUFFERED_PACKETS>,
>;
/// The host's one wake — the seam ends signal it, the contract host awaits it.
static WAKE: WakeSignal = new_wake_signal();
/// Each cycle's runtime snapshot, fired by the engine and subscribed by the OLED
/// render loop — latest-wins, so a burst of cycles coalesces to the newest view.
static SNAPSHOT_WATCH: RuntimeSnapshotWatch = RuntimeSnapshotWatch::new();
/// The user button's short/long-press events, from `button_task` to the render loop.
static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, screen::InputEvent, 4> = Channel::new();
/// The render loop's issued commands, drained by the engine one per cycle.
static COMMANDS: Channel<CriticalSectionRawMutex, IssuedCommand, 4> = Channel::new();

/// Ping command ids live in their own id space (top bit set), so the settle
/// handler never confuses them with the button's announce commands.
const PING_COMMAND_ID_BIT: u64 = 1 << 63;

/// One ping at the indirect neighbor: settlement-gated, so the stream is paced
/// by the real round trip through the relay - every tx/rx tick on the OLED is
/// a full sealed-forwarded-delivered-proven circle.
fn queue_ping(peer: personal_rns::wire::DestinationHash, seq: u64) {
    let mut payload_bytes = [0u8; 16];
    payload_bytes[..8].copy_from_slice(b"s3-ping:");
    payload_bytes[8..].copy_from_slice(&seq.to_le_bytes());
    let Ok(payload) = SendSinglePayload::from_slice(&payload_bytes) else {
        return;
    };
    let queued = COMMANDS.try_send(IssuedCommand {
        id: CommandId(PING_COMMAND_ID_BIT | seq),
        command: EngineCommand::SendSingle(SendSingle {
            destination: peer,
            payload,
        }),
    });
    if queued.is_ok() {
        WAKE.signal(());
    }
}

/// Platform bring-up, then the Hopspot screen loop. Never returns — its frame holds the
/// panel-power gate, the OLED, and the battery ADC alive while the spawned tasks (node,
/// responder, button) do the work.
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // esp-rtos needs a heap + a timer + a software interrupt to boot the scheduler and
    // the embassy-time driver. WiFi and the IP stack push the heap well past the
    // USB-only footprint, so size it for the radio.
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let (core0_floor, core0_top) = core0_stack_bounds();
    let sp: usize;
    unsafe { core::arch::asm!("mov {0}, a1", out(reg) sp) };
    unsafe {
        CORE0_STACK.paint(
            core0_floor + STACK_GUARD_SKIP_BYTES,
            sp - STACK_PAINT_SP_MARGIN_BYTES,
            core0_top,
        )
    };

    esp_println::logger::init_logger_from_env();
    let rtc = Rtc::new(p.LPWR);
    let announce_timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    // This first banner is the only thing on the usb-serial-jtag before frames flow, so
    // the desktop's decoder skips it as pre-frame noise.
    println!("HOPSPOT_S3 boot — USB-auto + WiFi");

    // WiFi radio + IPv6 link-local stack for the LAN AutoInterface. The brain hashes its
    // peering token over the EUI-64 link-local, so pin the stack to that exact address
    // (from the same MAC the worker gets) — peers see it as our datagram source.
    let wifi_mac = MacAddress::new(
        base_mac_address()
            .as_bytes()
            .try_into()
            .expect("6-byte base MAC"),
    );
    let (wifi_controller, wifi_interfaces) =
        esp_radio::wifi::new(p.WIFI, Default::default()).expect("wifi new");
    let net_config = embassy_net::Config::ipv6_static(StaticConfigV6 {
        address: Ipv6Cidr::new(link_local_from_mac(wifi_mac), 64),
        gateway: None,
        dns_servers: Default::default(),
    });
    let wifi_rng = Rng::new();
    let net_seed = ((wifi_rng.random() as u64) << 32) | wifi_rng.random() as u64;
    let (wifi_stack, wifi_runner) = embassy_net::new(
        wifi_interfaces.station,
        net_config,
        mk_static!(StackResources<4>, StackResources::new()),
        net_seed,
    );
    spawner.spawn(wifi_connection_task(wifi_controller).expect("wifi connection task"));
    spawner.spawn(wifi_net_task(wifi_runner).expect("wifi net task"));

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

    // Async USB serial, split — the responder task owns the halves.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();
    let node_tag = node_tag_from_mac();

    // The device responder is self-driven: its launch spawns the board's concrete
    // worker task (the USB halves + tag captured here, beside the macro); `start` fires it.
    let interface = SelfDrivenInterface::new(
        device_descriptor(USB_INTERFACE_ID),
        move |context: UsbAutoContext| {
            spawner.spawn(
                responder_task(usb_rx, usb_tx, context, node_tag)
                    .expect("responder task fits the pool"),
            );
        },
    );

    // The WiFi AutoInterface is self-driven the same way: its launch spawns the shared
    // embassy `serve` loop over the IP stack + this board's MAC (so the worker's token
    // matches the stack's link-local).
    let wifi_interface = SelfDrivenInterface::new(
        auto_interface::descriptor(WIFI_INTERFACE_ID),
        move |context: WifiAutoContext| {
            spawner.spawn(
                wifi_auto_task(wifi_stack, wifi_mac, context)
                    .expect("wifi auto task fits the pool"),
            );
        },
    );

    // The seams split here on core 0, so both worker tasks spawn onto THIS core's
    // executor beside the radio; only the runtime ends of the channels cross to
    // core 1 with the engine.
    let mut interfaces: EngineInterfaces = FixedInterfaceSet::new();
    let _ = interfaces.push(
        EmbassyInterfaceSeam::split_with_timebase(
            USB_INTERFACE_ID,
            &CHANNELS,
            &WAKE,
            announce_timebase,
        )
        .start_interface(interface),
    );
    // WiFi interface deliberately unregistered for now: the working rig is
    // USB-only, so every routed byte is attributable to the cable. Re-enable by
    // restoring this push when the rig needs WiFi again.
    let _ = &wifi_interface;
    // let _ = interfaces.push(
    //     EmbassyInterfaceSeam::split(WIFI_INTERFACE_ID, &WIFI_CHANNELS, &WAKE)
    //         .start_interface(wifi_interface),
    // );

    // The engine gets core 1 to itself: its own scheduler, its own explicitly
    // sized stack, and no radio task ever preempts a cycle. Radio, render, and
    // input stay here on core 0 — true parallelism across the seam.
    let secret_key = fixture_identity_secret_key();
    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    let core1_floor = core1_stack as *const CpuStack<CORE1_STACK_BYTES> as usize;
    let core1_top = core1_floor + core::mem::size_of::<CpuStack<CORE1_STACK_BYTES>>();
    unsafe {
        CORE1_STACK.paint(
            core1_floor + CORE1_ENTRY_BLOB_SKIP_BYTES,
            core1_top,
            core1_top,
        )
    };
    esp_rtos::start_second_core(
        p.CPU_CTRL,
        sw_int.software_interrupt1,
        core1_stack,
        move || {
            static CORE1_EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
            CORE1_EXECUTOR
                .init(esp_rtos::embassy::Executor::new())
                .run(|engine_spawner| {
                    engine_spawner.spawn(
                        engine_task(interfaces, secret_key, announce_timebase)
                            .expect("engine task fits the pool"),
                    );
                })
        },
    );

    // User button (GPIO0, the PRG/BOOT button; the internal pull-up holds it high on
    // release). Its task posts short/long presses the render loop turns into focus/menu.
    let button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits the pool"));

    // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by ADC_Ctrl on
    // GPIO37 — driven HIGH to connect the divider (the V4 flips the V3 convention). ADC1
    // is free because no TRNG holds it: the identity is a bring-up fixture (see node_task).
    let mut adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
    adc_ctrl.set_high();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(p.ADC1, adc_cfg);

    // The destination the announce button names: derived from the same fixture
    // identity the engine answers as, so the command and the registration agree.
    let self_destination = {
        let secret_key = fixture_identity_secret_key();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = expand_name("lxmf", &["delivery"]).expect("the self-announce name is valid");
        derive_destination_hash(&identity.identity_hash(), &name)
    };
    let mut next_command_id = 0u64;

    // The Hopspot screen, event-driven: redraw when engine state moves, the battery
    // cadence elapses, or the button is pressed — whichever first (so it parks between).
    let mut snapshots = SNAPSHOT_WATCH.receiver().expect("one snapshot receiver");
    let mut snapshot: Option<RuntimeSnapshot> = None;
    let mut ui_state = screen::UiState::new();
    // Smoothed VBAT (0 = uninitialized) so the bar doesn't jitter on ADC noise.
    let mut vbat_ema_mv: u32 = 0;
    // The interfaces' (rx, tx, dests) as of the last activity, to drive the idle blank.
    let mut last_activity_key: HVec<(u64, u64, u32), 8> = HVec::new();
    let mut last_active = Instant::now();
    let mut panel_on = true;
    let mut battery_tick = Ticker::every(Duration::from_secs(2));
    let mut last_budget_report = Instant::now();
    loop {
        // Battery level from the smoothed pin voltage; an implausibly low reading means
        // no LiPo (USB-only) → Unknown.
        let mut pin_mv = 0u16;
        for _ in 0..1000 {
            if let Ok(v) = vbat_adc.read_oneshot(&mut vbat_pin) {
                pin_mv = v;
                break;
            }
        }
        let vbat_mv = pin_mv as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
        let battery = if vbat_mv < VBAT_ABSENT_MV {
            screen::BatteryState::Unknown
        } else {
            vbat_ema_mv = if vbat_ema_mv == 0 {
                vbat_mv
            } else {
                (vbat_ema_mv * 7 + vbat_mv) / 8
            };
            let span = VBAT_FULL_MV - VBAT_EMPTY_MV;
            let pct = (vbat_ema_mv.saturating_sub(VBAT_EMPTY_MV) * 100 / span).min(100) as u8;
            screen::BatteryState::Level(pct)
        };

        // One card per interface in the latest snapshot — the host maps each id to its
        // icon/label. Rebuilt each iteration (cheap) so the button math has the live count.
        let cards: HVec<screen::Card, 8> = match &snapshot {
            Some(snap) => screen::snapshot_to_cards(snap, |id| {
                if id == WIFI_INTERFACE_ID {
                    Some((screen::CardKind::Wifi, "WiFi"))
                } else {
                    Some((screen::CardKind::Usb, "USB"))
                }
            }),
            None => HVec::new(),
        };
        let card_count = cards.len();
        ui_state.sync_card_count(card_count);

        // Reticulum activity = the per-interface traffic/destinations changed (battery
        // drift alone doesn't count, so a quiet node still blanks its panel).
        if let Some(snap) = &snapshot {
            let key: HVec<(u64, u64, u32), 8> = snap
                .interfaces
                .iter()
                .map(|v| {
                    (
                        v.reticulum_rx_byte_count,
                        v.reticulum_tx_byte_count,
                        v.tracked_destinations,
                    )
                })
                .collect();
            if key != last_activity_key {
                last_active = Instant::now();
                last_activity_key = key;
            }
        }
        let idle = OLED_IDLE_BLANK_SECS
            .is_some_and(|secs| last_active.elapsed() >= Duration::from_secs(secs));
        if idle && panel_on {
            let _ = display.set_display_on(false);
            panel_on = false;
        } else if !idle && !panel_on {
            let _ = display.set_display_on(true);
            panel_on = true;
        }

        if oled_ok && panel_on {
            screen::draw_with_state(&mut display, &cards, battery, &ui_state);
            let _ = display.flush();
        }

        match select3(
            snapshots.changed(),
            battery_tick.next(),
            BUTTON_EVENTS.receive(),
        )
        .await
        {
            Either3::First(new_snapshot) => {
                snapshot = Some(new_snapshot);
                // A short floor coalesces an announce burst into at most one render per ~100ms.
                Timer::after(Duration::from_millis(100)).await;
            }
            Either3::Second(()) => {}
            Either3::Third(event) => {
                // A button press is user activity, so it also un-blanks an idle panel.
                let action = ui_state.handle_input(event, card_count);
                if matches!(action, screen::UiAction::Announce) {
                    let id = CommandId(next_command_id);
                    next_command_id = next_command_id.wrapping_add(1);
                    let queued = COMMANDS.try_send(IssuedCommand {
                        id,
                        command: EngineCommand::AnnounceNow(AnnounceNow {
                            destination: self_destination,
                            target: AnnounceTarget::AllInterfaces,
                            app_data: AnnounceAppData::Scheduled,
                        }),
                    });
                    if queued.is_ok() {
                        WAKE.signal(());
                    }
                }
                last_active = Instant::now();
            }
        }
    }
}

// A bring-up fixture identity. The battery sense owns ADC1, so no TRNG can — and the
// bare RNG without RF isn't trustworthy for a keypair — so the identity is fixed
// (the oracle vectors' X25519 0x22 ‖ Ed25519 0x11). NEVER ship: every fixture node
// shares one identity. A real one needs RF-backed entropy (the WiFi pass).
fn fixture_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    // The board MAC mixed into both halves makes every flashed board a distinct
    // node (multi-board rigs need distinct identities to route at all). Still a
    // fixture: predictable from the MAC, so the NEVER-ship bar above stands.
    let mac = base_mac_address();
    let mac_bytes = mac.as_bytes();
    for (i, byte) in mac_bytes.iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

/// The node: build the embassy host and hand the recipe to `Prns::run` — the
/// announcing engine + runtime, driven forever on core 1's executor. Lives in
/// one task so the (unnameable) runtime future stays a local.
#[embassy_executor::task]
async fn engine_task(
    interfaces: EngineInterfaces,
    secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    announce_timebase: EmbassyTimebase,
) {
    // The embassy contract host owns the shared wake and draws each cycle's announce
    // jitter from the RNG (timing only — its quality is non-critical). No heap: the
    // board owns the `static` CHANNELS.
    let host =
        EmbassyContractHost::new_with_timebase(&WAKE, announce_timebase, |bytes: &mut [u8]| {
            Rng::new().read(bytes);
        });

    // Each cycle's snapshot goes to the OLED render loop, never the shared
    // usb-serial-jtag (a mid-frame log byte would corrupt the link).
    let snapshot_tx = SNAPSHOT_WATCH.sender();
    let transport_id = TransportId::new(
        *InMemoryNodeIdentity::from_secret_key_bytes(&secret_key)
            .identity_hash()
            .as_bytes(),
    );
    Prns::run(
        Recipe {
            engine_storage: ENGINE_STORAGE,
            transport_id: Some(transport_id),
            starting_destinations: [StartingDestinationConfig::Single {
                app_name: "lxmf",
                aspects: &["delivery"],
                identity_secret_key: secret_key,
                proof_strategy: ProofStrategy::ProveAll,
                ratchet_policy: RatchetPolicy::Ratcheted,
                announce: Some(AnnounceConfig {
                    app_data: SELF_ANNOUNCE_APP_DATA,
                    schedule: ReannounceSchedule::every(60_000),
                }),
            }],
            interfaces,
            host,
        },
        {
            let mut ping_peer = None;
            let mut ping_seq: u64 = 0;
            move |event: PrnsEvent<'_>| match event {
                PrnsEvent::SnapshotUpdated(snapshot) => snapshot_tx.send(snapshot.clone()),
                PrnsEvent::Delivered(_) => {}
                PrnsEvent::AnnounceHeard {
                    destination, hops, ..
                } => {
                    // A peer two or more hops out is an indirect neighbor behind
                    // a relay - exactly the path worth exercising continuously.
                    if hops >= 2 && ping_peer.is_none() {
                        ping_peer = Some(destination);
                        queue_ping(destination, ping_seq);
                    }
                }
                PrnsEvent::CommandSettled {
                    id,
                    settlement: Settlement::SendSingle(_),
                } if id.0 & PING_COMMAND_ID_BIT != 0 => {
                    if let Some(peer) = ping_peer {
                        ping_seq = ping_seq.wrapping_add(1);
                        queue_ping(peer, ping_seq);
                    }
                }
                PrnsEvent::CommandSettled { .. } => {}
            }
        },
        || COMMANDS.try_receive().ok(),
    )
    .await
}

/// The board's concrete responder task: the one monomorphization the launch closure
/// spawns. Just runs the crate's [`serve`] loop over the USB halves.
#[embassy_executor::task]
async fn responder_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    context: UsbAutoContext,
    node_tag: NodeTag,
) {
    serve(rx, tx, context, node_tag).await
}

/// The board's concrete WiFi AutoInterface worker: the one monomorphization the launch
/// closure spawns. Runs the shared embassy `serve` loop over the IP stack.
#[embassy_executor::task]
async fn wifi_auto_task(stack: Stack<'static>, mac: MacAddress, context: WifiAutoContext) {
    auto_interface::embassy::serve(stack, mac, context).await
}

/// Drives the WiFi station: applies the credentials (which `set_config` (re)starts the
/// controller for), connects, and reconnects whenever the link drops.
#[embassy_executor::task]
async fn wifi_connection_task(mut controller: WifiController<'static>) {
    let config = Config::Station(
        StationConfig::default()
            .with_ssid(WIFI_SSID)
            .with_password(WIFI_PASSWORD.into()),
    );
    if let Err(e) = controller.set_config(&config) {
        log::warn!("WIFI config failed: {e:?}");
    }
    log::info!("WIFI joining \"{WIFI_SSID}\"");
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                log::info!("WIFI connected");
                let _ = controller.wait_for_disconnect_async().await;
                log::warn!("WIFI disconnected, reconnecting");
            }
            Err(e) => {
                log::warn!("WIFI connect failed: {e:?}");
                Timer::after(Duration::from_millis(3_000)).await;
            }
        }
    }
}

/// Runs the embassy-net stack's background poll loop.
#[embassy_executor::task]
async fn wifi_net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

/// The user button worker: turn raw active-low edges on GPIO0 into the same
/// [`InputEvent`](screen::InputEvent)s the desktop face produces. A tap (release before
/// [`BUTTON_LONG_PRESS`]) is a `ShortPress`; crossing the hold threshold fires a
/// `LongPress` the instant it's reached — so the menu opens without waiting for release
/// — and the eventual release is swallowed.
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        // Active-low: a press pulls GPIO0 to ground (falling), the pull-up holds it high
        // on release (rising).
        button.wait_for_falling_edge().await;
        match select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            Either::First(()) => BUTTON_EVENTS.send(screen::InputEvent::ShortPress).await,
            Either::Second(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

/// This board's opaque link tag: the 6-byte eFuse MAC, padded to the 8-byte width.
fn node_tag_from_mac() -> NodeTag {
    let mac = base_mac_address();
    let bytes = mac.as_bytes();
    let mut tag = [0u8; 8];
    tag[..bytes.len()].copy_from_slice(bytes);
    NodeTag(tag)
}
