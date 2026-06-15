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

use core::sync::atomic::{AtomicUsize, Ordering};

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::zerocopy_channel;
use embassy_time::{Duration, Ticker, Timer};
use heapless::Vec as HVec;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use static_cell::{ConstStaticCell, StaticCell};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::reactor::grant::{AnyGrantConsumer, AnyGrantProducer, FrameSlot};
use personal_rns::reactor::impls::embassy_reactor::{
    embassy_grant_lane, run as run_reactor, EmbassyEgress, EmbassyGrantConsumer,
    EmbassyGrantProducer, EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus,
};
use personal_rns::reactor::interface_seam::{Interface, MAX_WIRE_FRAME_LEN};
use personal_rns::interfaces::usb_auto::core::device_descriptor;
use personal_rns::interfaces::usb_auto::impls::embassy::UsbAutoDevice;
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;

use crate::engine_storage::EngineStorageType;

use personal_hopspot_ui as screen;

esp_app_desc!();

/// This board's USB-auto interface id (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"prsnl-hopspot-s3");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot S3")` ‖ `nil` — the shape LXMF apps parse
/// (`\x13` = 19 = the name's length), so they surface the display name.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot S3\xc0";
/// How often the board re-announces itself: the engine does not originate announces, so the app
/// owns the cadence (the button fires the same announce on demand).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// The reactor's three cross-core channels, sized for an announce burst.
const INBOUND_CAP: usize = 8;
const OUTBOUND_CAP: usize = 8;
const COMMANDS_CAP: usize = 4;

/// The engine's own stack on core 1 — sized from the painted watermark. Re-measure via the
/// painted stacks whenever the engine grows.
const CORE1_STACK_BYTES: usize = 76 * 1024;

const STACK_PAINT_WORD: u32 = 0x57AC_C0DE;
const STACK_GUARD_SKIP_BYTES: usize = 64;
const CORE1_ENTRY_BLOB_SKIP_BYTES: usize = 2048;
const STACK_PAINT_SP_MARGIN_BYTES: usize = 2048;

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

/// UI repaint cadence — re-reads the live status (Dormant/Live, bytes) each tick.
const RENDER_INTERVAL: Duration = Duration::from_millis(500);
/// Re-read the battery every Nth render tick (≈ 2 s at the render cadence).
const RENDER_TICKS_PER_BATTERY: u8 = 4;

/// Hold the user button at least this long for a long press (open/close a menu);
/// anything shorter is a tap that advances focus. Matches the desktop face's threshold.
const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
/// Settle time after each press, so the contact's release bounce isn't a fresh press.
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

/// Place a value in a `'static` `StaticCell` and hand back the `'static` reference — the embassy
/// idiom for giving the core-1 stack a 'static lifetime without a heap allocation.
macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}

/// The USB interface's live state, written by the device task and read by the OLED render loop —
/// both on core 0, so it is shared by reference (not crossing the seam). A `const`-constructed
/// `static`, no `StaticCell` needed.
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);

/// One lane slot carries the engine's whole wire ceiling — the USB hardware MTU is larger,
/// but a thin (non-fat-links) engine never negotiates past this, so bigger slots would hold
/// bytes the engine refuses.
const USB_LANE_SLOT: usize = MAX_WIRE_FRAME_LEN;

const EMPTY_SLOT: FrameSlot<USB_LANE_SLOT> = FrameSlot::empty();

type UsbLaneRing =
    zerocopy_channel::Channel<'static, CriticalSectionRawMutex, FrameSlot<USB_LANE_SLOT>>;
type UsbSeam = EmbassyInterfaceSeam<'static, CriticalSectionRawMutex, INBOUND_CAP, USB_LANE_SLOT>;

/// The reactor's inputs, crossing the core 0 ↔ core 1 seam: the device fills inbound slots in
/// place and announces each commit on `NOTIFY`, the egress write-grants outbound slots the
/// device drains, and the render loop / announce timer issue commands. The frame bytes live in
/// these link-time buffers and never move.
static USB_IN_SLOTS: ConstStaticCell<[FrameSlot<USB_LANE_SLOT>; INBOUND_CAP]> =
    ConstStaticCell::new([EMPTY_SLOT; INBOUND_CAP]);
static USB_IN_RING: StaticCell<UsbLaneRing> = StaticCell::new();
static USB_OUT_SLOTS: ConstStaticCell<[FrameSlot<USB_LANE_SLOT>; OUTBOUND_CAP]> =
    ConstStaticCell::new([EMPTY_SLOT; OUTBOUND_CAP]);
static USB_OUT_RING: StaticCell<UsbLaneRing> = StaticCell::new();
static NOTIFY: Channel<CriticalSectionRawMutex, InterfaceId, INBOUND_CAP> = Channel::new();
static COMMANDS: Channel<CriticalSectionRawMutex, IssuedCommand, COMMANDS_CAP> = Channel::new();
/// The user button's short/long-press events, from `button_task` to the render loop.
static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, screen::InputEvent, 4> = Channel::new();

static CORE0_STACK: PaintedStack = PaintedStack::unpainted();
static CORE1_STACK: PaintedStack = PaintedStack::unpainted();

struct PaintedStack {
    floor: AtomicUsize,
    top: AtomicUsize,
}

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

    #[allow(dead_code)]
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

/// Platform bring-up, then the Hopspot screen loop. Never returns — its frame holds the
/// panel-power gate, the OLED, and the battery ADC alive while the spawned tasks (engine on
/// core 1; device, button, announce on core 0) do the work.
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // esp-rtos needs a heap + a timer + a software interrupt to boot the scheduler and the
    // embassy-time driver. USB-only (no radio/IP stack) keeps this small.
    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
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

    let rtc = Rtc::new(p.LPWR);
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    // The only thing on the usb-serial-jtag before frames flow — the desktop's decoder skips it
    // as pre-frame noise. Nothing else may print after this: the wire IS the link.
    println!("HOPSPOT_S3 boot — USB-auto on the reactor");
    #[cfg(feature = "footprint")]
    {
        Timer::after(Duration::from_millis(2000)).await;
        println!(
            "FOOTPRINT engine_state={} bytes (Esp32S3 layout)",
            core::mem::size_of::<EngineState<EngineStorageType>>()
        );
        println!("FOOTPRINT heap_free={} bytes", esp_alloc::HEAP.free());
    }

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

    // Async USB serial, split — the device task owns the halves on core 0.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();
    let (usb_in_tx, usb_in_rx) =
        embassy_grant_lane(USB_IN_RING.init(zerocopy_channel::Channel::new(USB_IN_SLOTS.take())));
    let (usb_out_tx, usb_out_rx) =
        embassy_grant_lane(USB_OUT_RING.init(zerocopy_channel::Channel::new(USB_OUT_SLOTS.take())));
    let seam = EmbassyInterfaceSeam::new(USB_INTERFACE_ID, usb_in_tx, NOTIFY.sender(), usb_out_rx);
    spawner.spawn(usb_device_task(usb_rx, usb_tx, seam).expect("device task fits the pool"));

    // The destination the announces name: derived from the same fixture identity the engine
    // answers as (`register_single_destination` derives the same hash), so command and
    // registration agree.
    let self_destination = {
        let secret_key = fixture_identity_secret_key();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = expand_name("lxmf", &["delivery"]).expect("the announce name is valid");
        derive_destination_hash(&identity.identity_hash(), &name)
    };
    spawner.spawn(announce_task(self_destination).expect("announce task fits"));

    // The engine gets core 1 to itself: its own scheduler, its own explicitly sized stack, and no
    // I/O task ever preempts a cycle. Device, render, and input stay here on core 0 — true
    // parallelism across the seam.
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
                        engine_task(secret_key, timebase, usb_in_rx, usb_out_tx)
                            .expect("engine task fits"),
                    );
                })
        },
    );

    // User button (GPIO0, the PRG/BOOT button; the internal pull-up holds it high on release).
    let button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits the pool"));

    // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by ADC_Ctrl on GPIO37 —
    // driven HIGH to connect the divider. ADC1 is free because no TRNG holds it: the identity is
    // a bring-up fixture.
    let mut adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
    adc_ctrl.set_high();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(p.ADC1, adc_cfg);

    // The Hopspot screen: re-read the interface's live status each render tick (so Dormant flips
    // to Live the moment a host links), and redraw on every button press.
    let mut ui_state = screen::UiState::new();
    let mut vbat_ema_mv: u32 = 0;
    let mut battery = screen::BatteryState::Unknown;
    let mut ticks_to_battery: u8 = 0;
    let mut next_command_id = 0u64;
    let mut render_tick = Ticker::every(RENDER_INTERVAL);
    loop {
        if ticks_to_battery == 0 {
            let mut pin_mv = 0u16;
            for _ in 0..1000 {
                if let Ok(v) = vbat_adc.read_oneshot(&mut vbat_pin) {
                    pin_mv = v;
                    break;
                }
            }
            let vbat_mv = pin_mv as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
            battery = if vbat_mv < VBAT_ABSENT_MV {
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
            ticks_to_battery = RENDER_TICKS_PER_BATTERY;
        }
        ticks_to_battery -= 1;

        let statuses = [&USB_STATUS];
        let cards: HVec<screen::Card, 8> = screen::statuses_to_cards(&statuses, |id| {
            if id == USB_INTERFACE_ID {
                Some((screen::CardKind::Usb, "USB"))
            } else {
                None
            }
        });
        let card_count = cards.len();
        ui_state.sync_card_count(card_count);
        if oled_ok {
            screen::draw_with_state(&mut display, &cards, battery, &ui_state);
            let _ = display.flush();
        }

        match select(render_tick.next(), BUTTON_EVENTS.receive()).await {
            Either::First(()) => {}
            Either::Second(event) => {
                if matches!(
                    ui_state.handle_input(event, card_count),
                    screen::UiAction::Announce
                ) {
                    next_command_id += 1;
                    let _ = COMMANDS.try_send(IssuedCommand {
                        id: CommandId(next_command_id),
                        command: EngineCommand::AnnounceNow(AnnounceNow {
                            destination: self_destination,
                            target: AnnounceTarget::AllInterfaces,
                            app_data: AnnounceAppData::Registered,
                        }),
                    });
                }
            }
        }
    }
}

/// A bring-up fixture identity. The battery sense owns ADC1, so no TRNG can — and the bare RNG
/// without RF isn't trustworthy for a keypair — so the identity is fixed (the oracle vectors'
/// X25519 0x22 ‖ Ed25519 0x11), with the board MAC mixed in so every flashed board is a distinct
/// node. NEVER ship: predictable from the MAC. A real one needs RF-backed entropy.
fn fixture_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    let mac = base_mac_address();
    let mac_bytes = mac.as_bytes();
    for (i, byte) in mac_bytes.iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

/// The engine on core 1: build it on the fixed inline storage, take the transport role, register
/// the `lxmf.delivery` destination, then drive the embassy reactor forever — racing the inbound
/// funnel, the command lane, and its own deadlines, carrying every directive out through the
/// egress. Lives in one task so the (unnameable) reactor future stays a local.
#[embassy_executor::task]
async fn engine_task(
    secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    timebase: EmbassyTimebase,
    usb_in_rx: EmbassyGrantConsumer<'static, CriticalSectionRawMutex, USB_LANE_SLOT>,
    usb_out_tx: EmbassyGrantProducer<'static, CriticalSectionRawMutex, USB_LANE_SLOT>,
) {
    let mut engine = EngineState::<EngineStorageType>::new(secret_key);
    let node = engine.held_identity_hashes()[0];
    engine
        .set_transport_identity(&node)
        .expect("the held identity takes the transport role");
    let _ = engine
        .register_single_destination(
            &node,
            "lxmf",
            &["delivery"],
            ANNOUNCE_APP_DATA,
            ProofStrategy::ProveAll,
            RatchetPolicy::Ratcheted,
        )
        .expect("registers the lxmf.delivery destination");

    // The host owns the clock and draws each cycle's announce jitter from the RNG (timing only —
    // its quality is non-critical). No heap: the board owns the `static` channels.
    let host = EmbassyHost::new_with_timebase(timebase, |bytes: &mut [u8]| {
        Rng::new().read(bytes);
    });

    let mut usb_in_rx = usb_in_rx;
    let mut usb_out_tx = usb_out_tx;
    let interfaces = [device_descriptor(USB_INTERFACE_ID)];
    let mut inbound_lanes: [(InterfaceId, &mut dyn AnyGrantConsumer); 1] =
        [(USB_INTERFACE_ID, &mut usb_in_rx)];
    let mut egress_lanes: [(InterfaceId, &mut dyn AnyGrantProducer); 1] =
        [(USB_INTERFACE_ID, &mut usb_out_tx)];
    let egress = EmbassyEgress::new(&mut egress_lanes);

    // The reactor swallows every directive into the egress itself; the app sees only `Journaled`,
    // and the board has nowhere to log it (the usb-serial-jtag is the wire) — so it is dropped.
    run_reactor(
        engine,
        &interfaces,
        &[],
        host,
        NOTIFY.receiver(),
        &mut inbound_lanes,
        COMMANDS.receiver(),
        egress,
        |_journaled: Journaled<'_>| {},
    )
    .await
}

/// The board's concrete device link: the one monomorphization the static task pool holds. Runs
/// the USB-auto device interface over the serial-jtag halves, funneling into the reactor and
/// writing the broadcasts it drains.
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

/// The board's announce cadence: the engine does not originate announces, so this fires a scheduled
/// `lxmf.delivery` announce on its own timer (the button fires the same on demand).
#[embassy_executor::task]
async fn announce_task(destination: DestinationHash) {
    let mut ticker = Ticker::every(ANNOUNCE_INTERVAL);
    let mut next_id = 0u64;
    loop {
        next_id += 1;
        let _ = COMMANDS.try_send(IssuedCommand {
            id: CommandId(next_id),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        });
        ticker.next().await;
    }
}

/// The user button worker: turn raw active-low edges on GPIO0 into the same
/// [`InputEvent`](screen::InputEvent)s the desktop face produces. A tap (release before
/// [`BUTTON_LONG_PRESS`]) is a `ShortPress`; crossing the hold threshold fires a `LongPress` the
/// instant it's reached — so the menu opens without waiting for release — and the eventual
/// release is swallowed.
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
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
