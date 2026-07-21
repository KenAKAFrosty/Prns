use embassy_executor::Spawner;
use embassy_futures::join::{join3, join5};
use embassy_futures::select::{select3, Either3};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::saadc::{self, ChannelConfig, Config as SaadcConfig, Gain, Reference, Saadc};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::zerocopy_channel;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;

use nrf_softdevice::ble::l2cap;
use nrf_softdevice::Softdevice;

use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::{BluetoothAuto, BluetoothAutoStatus};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::interfaces::bluetooth_auto::{Endpoint, LinkCapabilities, Nrf52Host, BLE_HW_MTU};
use personal_rns::interfaces::lora::{channel_tag, DEFAULT_915_PROFILE};
use personal_rns::interfaces::usb_auto::{WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};
use personal_rns::reactor::embassy::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, PooledEgress,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    Fleet, FleetWire, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, ReactorPlumbing, RequestHandlerRegistration,
};
use personal_rns::storage::StorageLayout;
use personal_rns::usb_auto::UsbAutoDevice;
use personal_rns::usb_auto::{WebUsbAutoClass, WebUsbAutoState, WEBUSB_AUTO_PACKET_SIZE};

use super::bluetooth_auto::{
    acceptor, scanner, serve_slot, softdevice_config, softdevice_task, usb_vbus_present,
    L2capPacket, NrfBleBackend, Server, BLE_SHARED, FLEET_ID, HUB, MEMBERS, OUTBOUND_WAKE, POOL,
};
use super::display::{build_cards, build_snapshots, frame_hash, EinkScreen};
use super::input;
use super::node::*;

const FULL_REFRESH_INTERVAL: u32 = 20;
const STATS_POLL: Duration = Duration::from_millis(1000);
const NOTICE_MS: u64 = 900;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    SAADC => saadc::InterruptHandler;
});

#[allow(clippy::too_many_lines)]
pub(crate) async fn run(spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.gpiote_interrupt_priority = Priority::P2;
    nrf_config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(nrf_config);

    let _eink_rail = Output::new(p.P0_12, Level::High, OutputDrive::Standard);
    let mut led = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    // The SoftDevice reserves P0/P1/P4; keep every app interrupt off those. USB at P2 (matches the
    // validated bring-up); SPI and SAADC at P3 so a BLE radio event can preempt them.
    interrupt::USBD.set_priority(Priority::P2);
    interrupt::SPI2.set_priority(Priority::P3);
    interrupt::TWISPI0.set_priority(Priority::P3);
    interrupt::SAADC.set_priority(Priority::P3);

    static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus = SOFTWARE_VBUS.init(SoftwareVbusDetect::new(true, true));

    let usb_driver = Driver::new(p.USBD, Irqs, &*vbus);
    let mut usb_config = UsbConfig::new(WEBUSB_VENDOR_ID, WEBUSB_PRODUCT_ID);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-HOP");
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    builder.msos_descriptor(0x0603_0000, 0x20);
    static USB_STATE: StaticCell<WebUsbAutoState> = StaticCell::new();
    let class = WebUsbAutoClass::new(
        &mut builder,
        USB_STATE.init(WebUsbAutoState::new()),
        WEBUSB_AUTO_PACKET_SIZE,
    );
    let mut usb = builder.build();

    // Battery sense: VBAT on a 2:1 divider into AIN2 (P0.04), sampled by the SAADC against the 3.0 V
    // internal reference, so VBAT_mV = raw * 6000 / 4096.
    let mut bat_channel = ChannelConfig::single_ended(p.P0_04);
    bat_channel.reference = Reference::INTERNAL;
    bat_channel.gain = Gain::GAIN1_5;
    let saadc = Saadc::new(p.SAADC, Irqs, SaadcConfig::default(), [bat_channel]);

    // The SoftDevice owns the radio + CLOCK/POWER, and feeds the USB vbus detector over its SoC
    // events; bring it up here (before the dalek-heavy engine construction) so its boot matches the
    // validated first-light ordering. Constructing the engine afterward is fine — the SD's own
    // high-priority interrupts keep the radio alive across the synchronous build.
    let sd = Softdevice::enable(&softdevice_config());
    static SERVER: StaticCell<Server> = StaticCell::new();
    let server: &'static Server = SERVER.init(Server::new(sd).unwrap());
    static L2CAP: StaticCell<l2cap::L2cap<L2capPacket>> = StaticCell::new();
    let l2cap: &'static l2cap::L2cap<L2capPacket> = L2CAP.init(l2cap::L2cap::init(sd));
    let sd: &'static Softdevice = sd;
    spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));
    let ble_identity = super::ble_identity::load_or_create(sd).await.ok();
    if let Some(identity) = ble_identity {
        super::bluetooth_auto::set_columba_identity(server, identity);
    }
    // The connection-slot pool: one worker per slot, parked until handed a connection. Pre-fill
    // the free list so the acceptor can advertise; seed the single central-radio permit.
    if ble_identity.is_some() {
        let _ = HUB.central_token.try_send(());
        for idx in 0..POOL {
            let _ = HUB.free.try_send(idx);
            spawner.spawn(serve_slot(idx, sd, l2cap, server, &HUB).expect("serve slot fits"));
        }
    }

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
    let eink = crate::ssd1681::Ssd1681::new(eink_spi, eink_busy, eink_dc, eink_rst, Delay).ok();

    // Self-identity: the same fixture keypair the LoRa-only build uses, so the board keeps one
    // destination across builds.
    let secret_key = techo_secret_key();
    let transport_secret = secret_key.clone();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name)
    };
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) | 1,
        core::sync::atomic::Ordering::Relaxed,
    );

    // The reactor's slot pool: LoRa on slot 0, the BLE fleet's one shared lane on slot 1. The fleet
    // slot's egress producer carries the outbound wake so a committed frame rouses the supervisor.
    static IN_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
        [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
    static IN_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];
    static OUT_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
        [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
    static OUT_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];

    let mut inbound: ReactorInbound = heapless::Vec::new();
    let mut egress_lanes: ReactorEgressLanes = heapless::Vec::new();
    let mut iface_halves: [Option<(
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    )>; IFACES] = [const { None }; IFACES];
    for slot in 0..IFACES {
        let in_ch = IN_CH[slot].init(zerocopy_channel::Channel::new(IN_BUF[slot].take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH[slot].init(zerocopy_channel::Channel::new(OUT_BUF[slot].take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        if slot == BLE_FLEET_SLOT {
            out_producer.set_outbound_wake(&OUTBOUND_WAKE);
        }
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

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

    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
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
    let node: &'static mut Node = NODE.init_with(|| {
        PrnsNode::new(
            PrnsNodeRecipe {
                transport_identity: Some(transport_secret),
                pre_configured_destinations: [PreConfiguredDestination::Single {
                    resource_strategy:
                        personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                    app_name: "lxmf",
                    aspects: &["delivery"],
                    identity: secret_key,
                    announce_app_data: ANNOUNCE_APP_DATA,
                    proof: personal_rns::routing::ProofStrategy::ProveAll,
                    link_requests: personal_rns::routing::LinkRequestPolicy::AcceptAll,
                    ratchet: RatchetPolicy::Ratcheted,
                    request_handlers: RequestHandlerRegistration::None,
                }],
                app_state: (),
                storage: crate::storage::TechoStorage,
                routes: personal_rns::routes![],
                interfaces: personal_rns::runtime::Manual,
                on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
            },
            plumbing,
            host,
            heapless::Vec::new(),
        )
    });
    node.activate(LORA_SLOT, lora.descriptor());
    if ble_identity.is_some() {
        node.activate_fleet(BLE_FLEET_SLOT, FLEET_ID);
    }
    let (lora_in_producer, lora_out_consumer) =
        iface_halves[LORA_SLOT].take().expect("lora slot half");
    let lora_seam = EmbassyInterfaceSeam::new(
        lora_id,
        lora_in_producer,
        NOTIFY.sender(),
        lora_out_consumer,
        seeded_entropy,
    );

    let (ble_in_producer, ble_out_consumer) =
        iface_halves[BLE_FLEET_SLOT].take().expect("ble fleet half");
    let fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = Fleet::new(
        FleetWire {
            inbound: ble_in_producer,
            outbound: ble_out_consumer,
            notify: NOTIFY.sender(),
            outbound_wake: &OUTBOUND_WAKE,
        },
        LIFECYCLE.sender(),
    );

    // The browser-facing USB-auto Reticulum interface is vendor-specific bulk, not CDC ACM. CDC is
    // claimed by OS serial drivers on desktop hosts; a vendor interface gives WebUSB a clean endpoint
    // pair to claim while keeping the Prns Hello/HelloAck wire exactly the same.
    let (usb_tx, usb_rx) = class.split();
    static USB_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let usb_status: &'static EmbassyInterfaceStatus = USB_STATUS.init(EmbassyInterfaceStatus::new(
        USB_INTERFACE_ID,
        ConnectionState::Initializing,
    ));
    let usb_dev = UsbAutoDevice::new(USB_INTERFACE_ID, usb_rx, usb_tx, usb_status, || true);
    node.activate(USB_SLOT, usb_dev.descriptor());
    let (usb_in_producer, usb_out_consumer) = iface_halves[USB_SLOT].take().expect("usb slot half");
    let usb_seam = EmbassyInterfaceSeam::new(
        USB_INTERFACE_ID,
        usb_in_producer,
        NOTIFY.sender(),
        usb_out_consumer,
        seeded_entropy,
    );

    let backend = NrfBleBackend::new(&HUB);
    let supervisor = ble_identity.map(|identity| {
        BluetoothAuto::new(
            backend,
            identity,
            Endpoint::Nrf52(Nrf52Host::Nrf52),
            LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            },
            &BLE_SHARED,
        )
    });

    let button = Input::new(p.P1_10, Pull::Up);
    let frontlight = Output::new(p.P1_11, Level::Low, OutputDrive::Standard);

    let usb_fut = usb.run();

    let heartbeat = async {
        let mut n = 0u32;
        loop {
            Timer::after(Duration::from_secs(1)).await;
            n = n.wrapping_add(1);
            if n & 1 == 0 {
                led.set_low();
            } else {
                led.set_high();
            }
        }
    };

    let ui_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let render = async move {
        let mut saadc = saadc;
        let mut epd = match eink {
            Some(epd) => epd,
            None => core::future::pending().await,
        };
        let mut ui_state = hopspot::UiState::new(hopspot::UiConfiguration {
            storage_limits: <crate::storage::TechoStorage as StorageLayout>::LIMITS,
            display_power_control: hopspot::DisplayPowerControl::Unavailable,
            access_point: hopspot::AccessPointState::Unsupported,
        });
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut since_full = 0u32;
        let mut displayed_hash = 0u64;
        let mut have_displayed = false;
        let mut activity = hopspot::CardActivityTracker::<{ MEMBERS + 4 }>::new();
        let mut notice_until_ms: Option<u64> = None;
        let mut battery_gauge = hopspot::BatteryGauge::lipo();
        loop {
            let mut adc = [0i16; 1];
            saadc.sample(&mut adc).await;
            let vbat_mv = (adc[0].max(0) as u32) * 6000 / 4096;
            let battery = battery_gauge.update(Some(vbat_mv), usb_vbus_present());

            let snapshots = build_snapshots(lora_status, usb_status);
            let mut cards = build_cards(&snapshots, lora_status.id(), usb_status.id());
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);
            if notice_until_ms.is_some_and(|until| now_ms >= until) {
                ui_state.clear_notice();
                notice_until_ms = None;
            }

            let _ = panel.clear(EpdColor::White);
            let interface_menu_details = hopspot::snapshots_to_interface_menu_details(
                ui_state.selected_card(&cards),
                &snapshots,
            );
            hopspot::render(
                &mut EinkScreen { panel: &mut panel },
                hopspot::RenderFrame {
                    cards: &cards,
                    battery,
                    state: &ui_state,
                    local_docs: None,
                    interface_menu_details: &interface_menu_details,
                    animation_ms: now_ms,
                },
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
                input::EVENTS.receive(),
                INTERFACE_STORE.changed(),
                Timer::after(STATS_POLL),
            )
            .await
            {
                Either3::First(event) => {
                    let action = ui_state.handle_input(event, &cards);
                    match action {
                        hopspot::UiAction::Sleep => {
                            ui_state.show_notice(hopspot::UiNotice::Sleeping);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            lora_status.disable();
                            usb_status.disable();
                            let status = BluetoothAutoStatus::new(&BLE_SHARED);
                            status.disable();
                        }
                        hopspot::UiAction::Wake => {
                            ui_state.show_notice(hopspot::UiNotice::Awake);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            lora_status.enable();
                            usb_status.enable();
                            let status = BluetoothAutoStatus::new(&BLE_SHARED);
                            status.enable();
                        }
                        hopspot::UiAction::Announce => {
                            ui_state.show_notice(hopspot::UiNotice::Announcing);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            let _ = ui_handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state.selected_card(&cards) {
                                if card.id() == lora_status.id() {
                                    ui_state.show_notice(if lora_status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                    lora_status.toggle_enabled();
                                } else if card.id() == usb_status.id() {
                                    ui_state.show_notice(if usb_status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                    usb_status.toggle_enabled();
                                } else if card.id() == FLEET_ID {
                                    let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                    ui_state.show_notice(if status.is_enabled() {
                                        hopspot::UiNotice::TurningOff
                                    } else {
                                        hopspot::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                    status.toggle_enabled();
                                }
                            }
                        }
                        hopspot::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        hopspot::UiAction::SetLoRaProfile(profile) => {
                            ui_state.show_notice(hopspot::UiNotice::Saved);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            working_lora_profile = profile;
                            LORA_CONTROL.signal(profile);
                        }
                        hopspot::UiAction::OpenDocs => {}
                        hopspot::UiAction::SwapRadioMode => {}
                        hopspot::UiAction::OledOff => {}
                        hopspot::UiAction::None => {}
                    }
                }
                Either3::Second(()) => {}
                Either3::Third(()) => {}
            }
        }
    };

    let io = join5(
        usb_fut,
        usb_dev.run(usb_seam),
        heartbeat,
        input::drive_button(button),
        input::drive_frontlight(frontlight),
    );
    let ble_plane = async move {
        match supervisor {
            Some(supervisor) => {
                join3(acceptor(sd, &HUB), scanner(sd, &HUB), supervisor.run(fleet)).await;
            }
            None => core::future::pending().await,
        }
    };
    let mesh = join3(
        node.run_reactor_with_interface_store(&INTERFACE_STORE),
        lora.run(lora_seam),
        render,
    );
    join3(io, ble_plane, mesh).await;
    core::future::pending().await
}
