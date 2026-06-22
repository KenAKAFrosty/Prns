#![no_std]
#![no_main]

use core::convert::Infallible;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, Ordering};

use panic_halt as _;

use embassy_executor::Spawner;
use embassy_futures::join::{join, join3, join5};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config as UsbConfig};
use heapless::{String, Vec as HVec};
use static_cell::{ConstStaticCell, StaticCell};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;

use personal_hopspot_ui as hopspot;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, IssuedCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::lora::core::{channel_tag, DEFAULT_915_PROFILE};
use personal_rns::interfaces::rns_parity::lora::impls::embassy::{LoRaControl, LoRaInterface};
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, Membership,
};
use personal_rns::reactor::grant::FrameSlot;
use personal_rns::reactor::impls::embassy_reactor::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbassyPrnsHandle, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::subghz_rf::{BoardConfig, Sx126x, TcxoVoltage};
use personal_rns::wire::TransportId;

mod ssd1681;
mod storage;
use ssd1681::Ssd1681;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

const IFACES: usize = 1;
const MAX_IFACES: usize = 4;
const NOTIFY_CAP: usize = 8;
const COMMANDS_CAP: usize = 8;
const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;
const LANE_DEPTH: usize = 1;
const STORE_CAP: usize = 8;

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x17Personal Hopspot T-Echo\xc0";

const PANEL_SIZE: i32 = 200;
const SCREEN_WIDTH: i32 = 64;
const SCREEN_HEIGHT: i32 = 128;
const SCALE_NUM: i32 = 3;
const SCALE_DEN: i32 = 2;
const SCALED_SHORT: i32 = SCREEN_WIDTH * SCALE_NUM / SCALE_DEN;
const SCALED_LONG: i32 = SCREEN_HEIGHT * SCALE_NUM / SCALE_DEN;
const SCALED_ORIGIN_X: i32 = (PANEL_SIZE - SCALED_LONG) / 2;
const SCALED_ORIGIN_Y: i32 = (PANEL_SIZE - SCALED_SHORT) / 2;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);
const FULL_REFRESH_INTERVAL: u32 = 20;
const FRONTLIGHT_HOLD: Duration = Duration::from_secs(8);
const STATS_POLL: Duration = Duration::from_millis(1000);

type Mtx = CriticalSectionRawMutex;
type EngineStorageType = storage::TechoStorage;
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

static LORA_CONTROL: LoRaControl = LoRaControl::new();
static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_COUNTS: EmbassyInterfaceStore<Mtx, STORE_CAP> = EmbassyInterfaceStore::new();
static BUTTON_EVENTS: Channel<Mtx, hopspot::InputEvent, 4> = Channel::new();
static BUTTON_COUNT: AtomicU32 = AtomicU32::new(0);
static FRONTLIGHT_WAKE: Signal<Mtx, ()> = Signal::new();
static ENTROPY_STATE: AtomicU32 = AtomicU32::new(0x9e37_79b9);

fn seeded_entropy(bytes: &mut [u8]) {
    let mut state = ENTROPY_STATE.load(Ordering::Relaxed);
    for byte in bytes {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = (state >> 24) as u8;
    }
    ENTROPY_STATE.store(state, Ordering::Relaxed);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

fn frame_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

async fn write_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
    bytes: &[u8],
) -> Result<(), embassy_usb::driver::EndpointError> {
    for chunk in bytes.chunks(60) {
        class.write_packet(chunk).await?;
    }
    Ok(())
}

fn techo_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    key[..32].fill(0x33);
    key[32..].fill(0x44);
    key
}

struct EinkScreen<'a> {
    panel: &'a mut Display1in54,
}

impl OriginDimensions for EinkScreen<'_> {
    fn size(&self) -> Size {
        Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)
    }
}

impl DrawTarget for EinkScreen<'_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let panel_color = match color {
                BinaryColor::On => EpdColor::Black,
                BinaryColor::Off => EpdColor::White,
            };
            let sx0 = point.x * SCALE_NUM / SCALE_DEN;
            let sx1 = (point.x + 1) * SCALE_NUM / SCALE_DEN;
            let sy0 = point.y * SCALE_NUM / SCALE_DEN;
            let sy1 = (point.y + 1) * SCALE_NUM / SCALE_DEN;
            let top_left = Point::new(
                SCALED_ORIGIN_X + sy0,
                SCALED_ORIGIN_Y + (SCALED_SHORT - sx1),
            );
            let size = Size::new((sy1 - sy0) as u32, (sx1 - sx0) as u32);
            let _ = self
                .panel
                .fill_solid(&Rectangle::new(top_left, size), panel_color);
        }
        Ok(())
    }
}

async fn drive_button(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        FRONTLIGHT_WAKE.signal(());
        match select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            Either::First(()) => {
                BUTTON_COUNT.fetch_add(1, Ordering::Relaxed);
                BUTTON_EVENTS.send(hopspot::InputEvent::ShortPress).await;
            }
            Either::Second(()) => {
                BUTTON_COUNT.fetch_add(1, Ordering::Relaxed);
                BUTTON_EVENTS.send(hopspot::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

async fn drive_frontlight(mut frontlight: Output<'static>) -> ! {
    loop {
        FRONTLIGHT_WAKE.wait().await;
        frontlight.set_high();
        while let Either::First(()) =
            select(FRONTLIGHT_WAKE.wait(), Timer::after(FRONTLIGHT_HOLD)).await
        {}
        frontlight.set_low();
    }
}

fn build_cards(lora: &EmbassyInterfaceStatus, lora_id: InterfaceId) -> HVec<hopspot::Card, 4> {
    let classify = |id: InterfaceId| -> Option<(hopspot::CardKind, hopspot::CardLabel)> {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else {
            None
        }
    };
    let counts = INTERFACE_COUNTS.counts(lora_id);
    let mut snapshots: HVec<InterfaceSnapshot, 4> = HVec::new();
    let _ = snapshots.push(InterfaceSnapshot {
        id: lora.id(),
        connection: lora.connection(),
        rx_bytes: lora.rx_bytes(),
        tx_bytes: lora.tx_bytes(),
        transfer_rates: lora.transfer_rates(),
        destinations: counts.destinations,
        links: counts.links,
        transported_links: counts.transported_links,
        membership: Membership::Independent,
    });
    hopspot::snapshots_to_cards(&snapshots, classify)
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.hfclk_source = config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

    let _eink_rail = Output::new(p.P0_12, Level::High, OutputDrive::Standard);
    let mut led_green = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let mut usb_config = UsbConfig::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-001");
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static USB_STATE: StaticCell<State> = StaticCell::new();
    let mut builder = Builder::new(
        driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    let mut class = CdcAcmClass::new(&mut builder, USB_STATE.init(State::new()), 64);
    let mut usb = builder.build();

    let mut radio_spim_config = spim::Config::default();
    radio_spim_config.frequency = spim::Frequency::M4;
    let radio_bus = Spim::new(
        p.TWISPI0,
        Irqs,
        p.P0_19,
        p.P0_23,
        p.P0_22,
        radio_spim_config,
    );
    let radio_cs = Output::new(p.P0_24, Level::High, OutputDrive::Standard);
    let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
    let radio_busy = Input::new(p.P0_17, Pull::None);
    let radio_dio1 = Input::new(p.P0_20, Pull::None);
    let radio_reset = Output::new(p.P0_25, Level::High, OutputDrive::Standard);
    let radio = Sx126x::new(
        radio_spi,
        radio_busy,
        radio_dio1,
        radio_reset,
        Delay,
        BoardConfig {
            tcxo_voltage: Some(TcxoVoltage::V1_8),
            use_dcdc: true,
            rx_boost: true,
            dio2_as_rf_switch: true,
        },
    );

    let mut eink_spim_config = spim::Config::default();
    eink_spim_config.frequency = spim::Frequency::M4;
    let eink_bus = Spim::new(p.SPI2, Irqs, p.P0_31, p.P1_06, p.P0_29, eink_spim_config);
    let eink_cs = Output::new(p.P0_30, Level::High, OutputDrive::Standard);
    let eink_dc = Output::new(p.P0_28, Level::Low, OutputDrive::Standard);
    let eink_rst = Output::new(p.P0_02, Level::High, OutputDrive::Standard);
    let eink_busy = Input::new(p.P0_03, Pull::None);
    Timer::after(Duration::from_millis(150)).await;
    let eink_spi = ExclusiveDevice::new(eink_bus, eink_cs, Delay).unwrap();
    let mut panel = Display1in54::default();
    let eink = Ssd1681::new(eink_spi, eink_busy, eink_dc, eink_rst, Delay).ok();
    let eink_ok = eink.is_some();

    let secret_key = techo_secret_key();
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
        u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) | 1,
        Ordering::Relaxed,
    );

    static IN_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
    static IN_CH: StaticCell<LaneChannel> = StaticCell::new();
    static OUT_BUF: ConstStaticCell<LaneBuf> = ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]);
    static OUT_CH: StaticCell<LaneChannel> = StaticCell::new();
    let in_ch = IN_CH.init(zerocopy_channel::Channel::new(IN_BUF.take()));
    let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
    let out_ch = OUT_CH.init(zerocopy_channel::Channel::new(OUT_BUF.take()));
    let (out_producer, out_consumer) = embassy_grant_lane(out_ch);
    let mut inbound: ReactorInbound = HVec::new();
    let _ = inbound.push((FREE_SLOT, in_consumer));
    let mut egress_lanes: ReactorEgressLanes = HVec::new();
    let _ = egress_lanes.push((FREE_SLOT, out_producer));

    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&lora_profile));
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing),
    );
    let lora = LoRaInterface::new(
        radio,
        lora_profile,
        &LORA_CONTROL,
        lora_status,
        LIFECYCLE.dyn_sender(),
    );

    let handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new(seeded_entropy as fn(&mut [u8]));
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
            storage: storage::TechoStorage,
            routes: personal_rns::routes![],
            interfaces: personal_rns::interfaces![],
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    node.activate(0, lora.descriptor());
    node.set_interface_store(&INTERFACE_COUNTS);

    let lora_seam = EmbassyInterfaceSeam::new(lora_id, in_producer, NOTIFY.sender(), out_consumer);
    let ui_handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);

    let button = Input::new(p.P1_10, Pull::Up);
    let frontlight = Output::new(p.P1_11, Level::Low, OutputDrive::Standard);

    let usb_fut = usb.run();

    let heartbeat_fut = async {
        loop {
            class.wait_connection().await;
            loop {
                let counts = INTERFACE_COUNTS.counts(lora_id);
                let mut line: String<128> = String::new();
                let _ = write!(
                    line,
                    "techo eink={} btn={} lora={:?} rx={} tx={} dests={} links={}\r\n",
                    if eink_ok { "ok" } else { "err" },
                    BUTTON_COUNT.load(Ordering::Relaxed),
                    lora_status.connection(),
                    lora_status.rx_bytes(),
                    lora_status.tx_bytes(),
                    counts.destinations,
                    counts.links,
                );
                if write_serial(&mut class, line.as_bytes()).await.is_err() {
                    break;
                }
                Timer::after(Duration::from_millis(1000)).await;
            }
        }
    };

    let blink_fut = async {
        loop {
            led_green.set_low();
            Timer::after(Duration::from_millis(500)).await;
            led_green.set_high();
            Timer::after(Duration::from_millis(500)).await;
        }
    };

    let render = async move {
        let mut epd = match eink {
            Some(epd) => epd,
            None => core::future::pending().await,
        };
        let mut ui_state = hopspot::UiState::new();
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut since_full = 0u32;
        let mut displayed_hash = 0u64;
        let mut have_displayed = false;
        loop {
            let cards = build_cards(lora_status, lora_id);
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);

            let _ = panel.clear(EpdColor::White);
            hopspot::draw_with_state(
                &mut EinkScreen { panel: &mut panel },
                &cards,
                hopspot::BatteryState::Unknown,
                &ui_state,
            );
            let hash = frame_hash(panel.buffer());
            if !have_displayed || hash != displayed_hash {
                if !have_displayed || since_full >= FULL_REFRESH_INTERVAL {
                    let _ = epd.full_update(panel.buffer());
                    since_full = 0;
                } else {
                    let _ = epd.partial_update(panel.buffer());
                }
                since_full += 1;
                displayed_hash = hash;
                have_displayed = true;
            }

            match select3(
                BUTTON_EVENTS.receive(),
                INTERFACE_COUNTS.changed(),
                Timer::after(STATS_POLL),
            )
            .await
            {
                Either3::First(event) => {
                    let selected_kind = ui_state
                        .selected_card(card_count)
                        .and_then(|index| cards.get(index))
                        .map(|card| card.kind);
                    match ui_state.handle_input(event, card_count, selected_kind) {
                        hopspot::UiAction::Announce => {
                            let _ = ui_handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state
                                .selected_card(card_count)
                                .and_then(|index| cards.get(index))
                            {
                                if card.id == lora_id {
                                    lora_status.set_enabled(!lora_status.is_enabled());
                                }
                            }
                        }
                        hopspot::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        hopspot::UiAction::SetLoRaProfile(profile) => {
                            working_lora_profile = profile;
                            LORA_CONTROL.signal(profile);
                        }
                        hopspot::UiAction::None => {}
                    }
                }
                Either3::Second(()) => {}
                Either3::Third(()) => {}
            }
        }
    };

    let drivers = join5(
        usb_fut,
        heartbeat_fut,
        blink_fut,
        drive_button(button),
        drive_frontlight(frontlight),
    );
    let mesh = join3(node.run_reactor(), lora.run(lora_seam), render);
    join(drivers, mesh).await;
    loop {}
}
