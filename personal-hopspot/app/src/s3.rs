use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

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

use personal_rns::engine::{
    EngineCycleEntropySeed, ReannounceSchedule, ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::rns_parity::auto_interface::{self, link_local_from_mac};
use personal_rns::interfaces::impls::usb_auto::core::{device_descriptor, NodeTag, MAX_DATA_BYTES};
use personal_rns::interfaces::impls::usb_auto::serve;
use personal_rns::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
use personal_rns::interfaces::substrate::{
    new_wake_signal, EmbassyHostSubstrate, EmbassyInterfaceChannels, WakeSignal,
};
use personal_rns::interfaces::{
    InterfaceId, InterfaceWorkerContext, MacAddress, SelfDrivenInterface,
};
use personal_rns::routing::storage::FixedInline;
use personal_rns::runtime::channels::embassy::RuntimeSnapshotWatch;
use personal_rns::runtime::host::impls::EmbassyContractHost;
use personal_rns::runtime::{StartingDestinationConfig, Prns, PrnsEvent, Recipe, RuntimeSnapshot};

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
const ENGINE_STORAGE: FixedInline<24, 32, 1024, 4, 128, 4, 4, 4, 4, 32> = FixedInline;

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
/// traffic resumes. On a busy fabric, announces keep it effectively always on.
const OLED_IDLE_BLANK_SECS: u64 = 30;

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

type WifiAutoContext =
    InterfaceWorkerContext<EmbassyHostSubstrate<MAX_DATA_BYTES, PER_INTERFACE_MAX_BUFFERED_PACKETS>>;
/// The host's one wake — the seam ends signal it, the contract host awaits it.
static WAKE: WakeSignal = new_wake_signal();
/// Each cycle's runtime snapshot, fired by the engine and subscribed by the OLED
/// render loop — latest-wins, so a burst of cycles coalesces to the newest view.
static SNAPSHOT_WATCH: RuntimeSnapshotWatch = RuntimeSnapshotWatch::new();
/// The user button's short/long-press events, from `button_task` to the render loop.
static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, screen::InputEvent, 4> = Channel::new();

/// Platform bring-up, then the Hopspot screen loop. Never returns — its frame holds the
/// panel-power gate, the OLED, and the battery ADC alive while the spawned tasks (node,
/// responder, button) do the work.
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // esp-rtos needs a heap + a timer + a software interrupt to boot the scheduler and
    // the embassy-time driver. WiFi and the IP stack push the heap well past the
    // USB-only footprint, so size it for the radio.
    esp_alloc::heap_allocator!(size: 110 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    esp_println::logger::init_logger_from_env();

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
    spawner.spawn(
        node_task(spawner, usb_rx, usb_tx, node_tag, wifi_stack, wifi_mac)
            .expect("node task fits the pool"),
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
        let idle = last_active.elapsed() >= Duration::from_secs(OLED_IDLE_BLANK_SECS);
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
                ui_state.handle_input(event, card_count);
                last_active = Instant::now();
            }
        }
    }
}

/// The node: build the embassy host, pop the USB-auto responder in, and hand the
/// recipe to `Prns::run` — the announcing engine + runtime, driven forever. Lives in
/// one task so the (unnameable) runtime future stays a local.
#[embassy_executor::task]
async fn node_task(
    spawner: Spawner,
    usb_rx: UsbSerialJtagRx<'static, Async>,
    usb_tx: UsbSerialJtagTx<'static, Async>,
    node_tag: NodeTag,
    wifi_stack: Stack<'static>,
    wifi_mac: MacAddress,
) {
    // A bring-up fixture identity. The battery sense owns ADC1, so no TRNG can — and the
    // bare RNG without RF isn't trustworthy for a keypair — so the identity is fixed
    // (the oracle vectors' X25519 0x22 ‖ Ed25519 0x11). NEVER ship: every fixture node
    // shares one identity. A real one needs RF-backed entropy (the WiFi pass).
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);

    // The embassy contract host owns the shared wake and draws each cycle's announce
    // jitter from the RNG (timing only — its quality is non-critical). No heap: the
    // board owns the `static` CHANNELS.
    let host = EmbassyContractHost::new(&WAKE, || {
        let mut bytes = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        Rng::new().read(&mut bytes);
        EngineCycleEntropySeed::new(bytes)
    });

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

    // `attach` glues the seam from the board's `static` channels (keyed by the id the
    // interface carries), then starts it: the worker task holds the context, the runtime
    // keeps the handle.
    let mut interfaces = FixedInterfaceSet::<_, 2>::new();
    let _ = interfaces.push(host.attach(interface, &CHANNELS));
    let _ = interfaces.push(host.attach(wifi_interface, &WIFI_CHANNELS));

    // Each cycle's snapshot goes to the OLED render loop, never the shared
    // usb-serial-jtag (a mid-frame log byte would corrupt the link).
    let snapshot_tx = SNAPSHOT_WATCH.sender();
    Prns::run(
        Recipe {
            engine_storage: ENGINE_STORAGE,
            starting_destinations: [StartingDestinationConfig::Single {
                app_name: "lxmf",
                aspects: &["delivery"],
                identity_secret_key: secret_key,
                announce: Some(AnnounceConfig {
                    app_data: SELF_ANNOUNCE_APP_DATA,
                    // Fast re-announce so the desktop catches us promptly during bring-up.
                    schedule: ReannounceSchedule::every(10_000),
                }),
            }],
            interfaces,
            host,
        },
        move |event: PrnsEvent<'_>| match event {
            PrnsEvent::SnapshotUpdated(snapshot) => snapshot_tx.send(snapshot.clone()),
            PrnsEvent::Delivered(_) => {}
        },
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
