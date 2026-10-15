use super::ui as s3fn8_ui;
use crate::display_runtime::{ImmediateBoardDisplay, S3BoardDisplay};
#[cfg(feature = "remote-control-pairing")]
use crate::immediate_display::ImmediatePresentation;
use crate::immediate_display::{ImmediateDisplayDevice, ImmediateDisplayRuntime};
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::{Input, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::time::{Duration as HalDuration, Rate};
#[cfg(feature = "remote-control-pairing")]
use hopspot::display::DisplayVisibility;
use hopspot::display::{
    BlankingCommand, BlankingOutcome, BlankingResult, BufferRetention, DisplayAutoOff,
    DisplayBlankReason, DisplayButtonOutcome, MonotonicMillis, PresentationOutcome,
};
use personal_hopspot_core as hopspot;
use personal_rns::interfaces::{subghz::SubGConfigurationState, InterfaceSnapshot};
use ssd1306::mode::BufferedGraphicsMode;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

pub(super) type Oled = Ssd1306<
    I2CInterface<I2c<'static, esp_hal::Blocking>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

pub(super) fn init(
    i2c: esp_hal::peripherals::I2C0<'static>,
    sda: esp_hal::peripherals::GPIO17<'static>,
    scl: esp_hal::peripherals::GPIO18<'static>,
    reset_pin: esp_hal::peripherals::GPIO21<'static>,
) -> Option<Oled> {
    let mut reset = Output::new(reset_pin, Level::High, OutputConfig::default());
    reset.set_low();
    esp_hal::delay::Delay::new().delay(HalDuration::from_millis(20));
    reset.set_high();
    esp_hal::delay::Delay::new().delay(HalDuration::from_millis(20));

    let i2c = I2c::new(
        i2c,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("Heltec V3 I2C0 configuration valid")
    .with_sda(sda)
    .with_scl(scl);
    let mut oled = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    match oled.init() {
        Ok(()) => Some(oled),
        Err(error) => {
            log::error!("Heltec V3 OLED initialization failed: {error:?}");
            None
        }
    }
}

struct Display(Oled);
impl ImmediateDisplayDevice for Display {
    fn present(&mut self, frame: &hopspot::face_64x128::Frame) -> PresentationOutcome {
        if crate::immediate_display::draw_canonical_frame(&mut self.0, frame).is_err()
            || self.0.flush().is_err()
        {
            PresentationOutcome::Failed
        } else {
            PresentationOutcome::Succeeded
        }
    }
    fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome {
        BlankingOutcome {
            result: if self
                .0
                .set_display_on(command == BlankingCommand::Restore)
                .is_ok()
            {
                BlankingResult::Succeeded
            } else {
                BlankingResult::Failed
            },
            buffer_retention: BufferRetention::Preserved,
        }
    }
}

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);
pub(super) static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, hopspot::InputEvent, 4> =
    Channel::new();

/// Wait on the user pin, the same gesture the V4 display task and the nRF boards use.
#[embassy_executor::task]
pub(super) async fn button_task(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        match select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            Either::First(()) => BUTTON_EVENTS.send(hopspot::InputEvent::ShortPress).await,
            Either::Second(()) => {
                BUTTON_EVENTS.send(hopspot::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const CHARGE_RISE_MV: u32 = 16;
const BATTERY_SAMPLE_MILLIS: u64 = 2000;
const RENDER_MILLIS: u64 = 500;
const NOTICE_MILLIS: u64 = 900;

struct Battery {
    adc: Adc<'static, esp_hal::peripherals::ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<
        esp_hal::peripherals::GPIO1<'static>,
        esp_hal::peripherals::ADC1<'static>,
        AdcCalCurve<esp_hal::peripherals::ADC1<'static>>,
    >,
    fast_ema_mv: u32,
    slow_ema_mv: u32,
}
impl hopspot::BatterySource for Battery {
    fn read_millivolts(&mut self) -> Option<u32> {
        for _ in 0..1000 {
            if let Ok(raw) = self.adc.read_oneshot(&mut self.pin) {
                let mv = raw as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
                if self.slow_ema_mv == 0 {
                    self.fast_ema_mv = mv;
                    self.slow_ema_mv = mv;
                } else {
                    self.fast_ema_mv = (self.fast_ema_mv * 3 + mv) / 4;
                    self.slow_ema_mv = (self.slow_ema_mv * 15 + mv) / 16;
                }
                return Some(mv);
            }
        }
        None
    }

    fn external_power(&mut self) -> hopspot::ExternalPowerState {
        if self.fast_ema_mv > self.slow_ema_mv.saturating_add(CHARGE_RISE_MV) {
            hopspot::ExternalPowerState::Present {
                charging: hopspot::ChargingState::Charging,
            }
        } else {
            hopspot::ExternalPowerState::Unknown
        }
    }
}

pub(super) struct Face {
    display: Option<ImmediateDisplayRuntime<Display>>,
    battery: Battery,
    gauge: hopspot::BatteryGauge,
    power: hopspot::PowerSnapshot,
    pub(super) state: hopspot::UiState,
    activity: hopspot::CardActivityTracker<{ super::INTERFACE_CAPACITY }>,
    next_render: u64,
    next_sample: u64,
    notice_until: u64,
    #[cfg(feature = "remote-control-pairing")]
    rendered_confirmation: Option<personal_rns::remote_control::RemoteControlPairingAttemptId>,
    #[cfg(feature = "remote-control-pairing")]
    pressed_confirmation: Option<personal_rns::remote_control::RemoteControlPairingAttemptId>,
}
fn now() -> MonotonicMillis {
    MonotonicMillis::new(Instant::now().as_millis())
}
impl Face {
    // Keep construction temporaries off the no-PSRAM firmware poll frame.
    #[inline(never)]
    pub(super) fn new(
        oled: Option<Oled>,
        adc: esp_hal::peripherals::ADC1<'static>,
        pin: esp_hal::peripherals::GPIO1<'static>,
    ) -> Self {
        let display =
            oled.map(|oled| ImmediateBoardDisplay::initialized(Display(oled)).into_runtime(now()));
        #[cfg(feature = "remote-control-pairing")]
        if display.is_none() {
            super::heltec_v3_pairing::display_failed();
        }
        let user_blanking = display
            .as_ref()
            .map_or(hopspot::UserBlanking::unavailable(), |display| {
                display.user_blanking()
            });
        let mut adc_config = AdcConfig::new();
        let pin = adc_config.enable_pin_with_cal::<_, AdcCalCurve<_>>(pin, Attenuation::_11dB);
        let adc = Adc::new(adc, adc_config);
        Self {
            display,
            battery: Battery {
                adc,
                pin,
                fast_ema_mv: 0,
                slow_ema_mv: 0,
            },
            gauge: hopspot::BatteryGauge::lipo(),
            power: hopspot::PowerSnapshot::UNKNOWN,
            state: s3fn8_ui::state(
                user_blanking,
                <super::InternalStorage as personal_rns::storage::StorageLayout>::LIMITS,
            ),
            activity: hopspot::CardActivityTracker::new(),
            next_render: 0,
            next_sample: 0,
            notice_until: 0,
            #[cfg(feature = "remote-control-pairing")]
            rendered_confirmation: None,
            #[cfg(feature = "remote-control-pairing")]
            pressed_confirmation: None,
        }
    }
    pub(super) fn notice(&mut self, notice: hopspot::UiNotice) {
        self.state.show_notice(notice);
        self.notice_until = Instant::now().as_millis() + NOTICE_MILLIS;
        self.next_render = 0;
    }
    pub(super) fn poll(
        &mut self,
        snapshots: &[InterfaceSnapshot],
        configuration: SubGConfigurationState,
        system_awake: bool,
        spectrum: &personal_rns::lora::LoRaSpectrumStatus,
        input: Option<hopspot::InputEvent>,
    ) -> hopspot::UiAction {
        let millis = Instant::now().as_millis();
        if millis >= self.next_sample {
            self.power = self.gauge.sample(&mut self.battery);
            hopspot::publish_power_snapshot(self.power);
            self.next_sample = millis + BATTERY_SAMPLE_MILLIS;
        }
        let mut cards = s3fn8_ui::cards::<{ super::INTERFACE_CAPACITY }>(
            snapshots,
            super::USB_INTERFACE_ID,
            configuration,
        );
        self.activity
            .update(&mut cards, (millis / 1000).min(u64::from(u32::MAX)) as u32);
        let content = hopspot::ScreenContent {
            cards: &cards,
            local_docs: None,
        };
        self.state.sync(content);
        #[cfg(feature = "remote-control-pairing")]
        self.state.sync_remote_control(
            super::heltec_v3_pairing::state(),
            super::heltec_v3_pairing::now(),
        );
        if self.notice_until != 0 && millis >= self.notice_until {
            self.state.clear_notice();
            self.notice_until = 0;
        }
        let mut action = hopspot::UiAction::None;
        if let Some(event) = input {
            let forward_to_ui = match self.display.as_mut() {
                Some(display) => matches!(
                    display.button_pressed(now(), now),
                    Ok(DisplayButtonOutcome::ForwardToUi)
                ),
                None => true,
            };
            #[cfg(feature = "remote-control-pairing")]
            {
                self.pressed_confirmation = self.rendered_confirmation;
            }
            if forward_to_ui {
                action = self.state.handle_input(event, content);
                if !system_awake {
                    action = hopspot::UiAction::Wake;
                }
                #[cfg(feature = "remote-control-pairing")]
                if let hopspot::UiAction::ApproveRemoteControlTargetPairing(attempt_id) = action {
                    if !s3fn8_ui::confirmation_matches_press(
                        attempt_id,
                        self.rendered_confirmation,
                        self.pressed_confirmation,
                    ) || !super::heltec_v3_pairing::enabled()
                    {
                        action = hopspot::UiAction::None;
                    }
                }
            }
            self.next_render = 0;
        }
        match action {
            hopspot::UiAction::BlankDisplay => {
                if let Some(display) = self.display.as_mut() {
                    let _ = display.schedule_blanking(now(), DisplayBlankReason::DisplayOnly);
                }
            }
            hopspot::UiAction::ToggleDisplayAutoOff => {
                if let Some(display) = self.display.as_mut() {
                    let next = match display.auto_off() {
                        Ok(DisplayAutoOff::Disabled) => DisplayAutoOff::Enabled,
                        _ => DisplayAutoOff::Disabled,
                    };
                    if display.set_auto_off(next, now()).is_ok() {
                        self.notice(if next == DisplayAutoOff::Disabled {
                            hopspot::UiNotice::DisplayAutoOffOff
                        } else {
                            hopspot::UiNotice::DisplayAutoOffOn
                        });
                    }
                }
            }
            _ => {}
        }
        if let Some(display) = self.display.as_mut() {
            let _ = display.poll_blanking(now(), now);
            #[cfg(feature = "remote-control-pairing")]
            if display.visibility() != DisplayVisibility::Visible {
                self.rendered_confirmation = None;
            }
            if millis >= self.next_render {
                let selected = self.state.selected_card(&cards);
                let mut details = if let Some(card) =
                    selected.filter(|card| card.id() == super::USB_INTERFACE_ID)
                {
                    hopspot::usb_interface_menu_details(card.connection())
                } else {
                    hopspot::snapshots_to_interface_menu_details(selected, snapshots)
                };
                if selected.is_some_and(|card| matches!(card.kind(), hopspot::CardKind::SubG(_))) {
                    let snapshot = spectrum.snapshot();
                    details.push_lora_spectrum(hopspot::LoRaSpectrumMenuDetails {
                        channel_busy_per_mille: snapshot.channel_busy_per_mille,
                        noise_floor_dbm: snapshot.noise_floor_dbm,
                        cca_threshold_dbm: snapshot.cca_threshold_dbm,
                        deferrals: snapshot.deferrals,
                        false_preambles: snapshot.false_preambles,
                        contention_timeouts: snapshot.contention_timeouts,
                        duty_holds: snapshot.duty_holds,
                        duty_timeouts: snapshot.duty_timeouts,
                        radio_recoveries: snapshot.radio_recoveries,
                    });
                }
                #[cfg_attr(not(feature = "remote-control-pairing"), allow(unused_variables))]
                let presented = display.render_and_present(
                    hopspot::face_64x128::RenderInput {
                        content,
                        battery: self.power,
                        gnss: None,
                        state: &self.state,
                        interface_menu_details: &details,
                    },
                    now(),
                    now,
                );
                #[cfg(feature = "remote-control-pairing")]
                match presented {
                    Ok(ImmediatePresentation::Presented) => {
                        let state = super::heltec_v3_pairing::state();
                        self.rendered_confirmation = if state.phase()
                            == hopspot::RemoteControlTargetPairingPhase::Confirmation
                        {
                            state.attempt_id()
                        } else {
                            None
                        };
                    }
                    Ok(ImmediatePresentation::Failed) | Err(_) => {
                        self.rendered_confirmation = None;
                        super::heltec_v3_pairing::display_failed();
                    }
                    _ => {}
                }
                self.next_render = millis + RENDER_MILLIS;
            }
        }
        action
    }
    pub(super) fn selected_interface(
        &self,
        snapshots: &[InterfaceSnapshot],
        configuration: SubGConfigurationState,
    ) -> Option<personal_rns::interfaces::InterfaceId> {
        let cards = s3fn8_ui::cards::<{ super::INTERFACE_CAPACITY }>(
            snapshots,
            super::USB_INTERFACE_ID,
            configuration,
        );
        self.state.selected_card(&cards).map(|card| card.id())
    }
}
