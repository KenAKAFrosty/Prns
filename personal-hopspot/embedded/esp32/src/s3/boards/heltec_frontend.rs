use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use esp_hal::gpio::{Flex, InputConfig, Level, Output, OutputConfig, OutputPin, Pin};
use esp_hal::time::Duration;
use personal_rns::radios::sx126x::FrontendControl;

/// Heltec's front-end family discriminator needs one bounded settling interval after its rail is
/// enabled. GPIO2 is sampled exactly once after this interval; it is not a readiness signal and
/// there is no polling state machine behind it.
const FRONTEND_FAMILY_DETECTION_SETTLE: Duration = Duration::from_millis(5);

// Both front-end variants amplify the receive path before the SX1262. These typical gains are
// removed from RSSI reports so channel access and diagnostics remain antenna-referred.
const GC1109_RX_GAIN_DB: u8 = 17;
const KCT8103L_RX_GAIN_DB: u8 = 23;

#[derive(Debug, Clone, Copy)]
pub(super) enum HeltecFrontendKind {
    Gc1109,
    Kct8103l,
}

impl HeltecFrontendKind {
    pub(super) const fn rx_gain_db(self) -> u8 {
        match self {
            Self::Gc1109 => GC1109_RX_GAIN_DB,
            Self::Kct8103l => KCT8103L_RX_GAIN_DB,
        }
    }

    pub(super) const fn control(self) -> FrontendControl {
        match self {
            Self::Gc1109 => FrontendControl::NoDynamicControl,
            Self::Kct8103l => FrontendControl::TxRx {
                enter_transmit,
                enter_receive,
            },
        }
    }
}

enum ModeControl {
    /// GC1109 CPS is board-driven and remains high; CTX is driven by SX1262 DIO2.
    Gc1109 { _cps: Output<'static> },
    /// KCT8103L CTX is low for RX and high for TX; CPS is driven by SX1262 DIO2.
    Kct8103l { ctx: Output<'static> },
}

/// Retain the GPIO owners for as long as callbacks can switch the front-end. Keeping this state
/// outside the board future also avoids carrying it across unrelated async suspension points.
struct HeldFrontend {
    _power: Output<'static>,
    _chip_enable: Flex<'static>,
    mode: ModeControl,
}

static HELD_FRONTEND: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldFrontend>>> =
    Mutex::new(RefCell::new(None));

pub(super) fn initialize(
    power_pin: impl OutputPin + 'static,
    chip_enable_pin: impl Pin + 'static,
    gc1109_cps_pin: impl OutputPin + 'static,
    kct8103l_ctx_pin: impl OutputPin + 'static,
) -> HeltecFrontendKind {
    // Match the reference detection sequence: configure CSD as an input, power the front-end,
    // allow its family strap to settle, then sample once before repurposing CSD as chip enable.
    let mut chip_enable = Flex::new(chip_enable_pin);
    chip_enable.apply_input_config(&InputConfig::default());
    chip_enable.set_input_enable(true);
    let power = Output::new(power_pin, Level::High, OutputConfig::default());
    esp_hal::delay::Delay::new().delay(FRONTEND_FAMILY_DETECTION_SETTLE);

    let kind = if chip_enable.is_high() {
        HeltecFrontendKind::Kct8103l
    } else {
        HeltecFrontendKind::Gc1109
    };

    // Set the latch before enabling the output driver so CSD cannot glitch low during the input →
    // output transition.
    chip_enable.set_high();
    chip_enable.set_input_enable(false);
    chip_enable.set_output_enable(true);

    let mode = match kind {
        HeltecFrontendKind::Gc1109 => {
            log::info!("LoRa FEM detected: GC1109 (GPIO46 CPS held high)");
            ModeControl::Gc1109 {
                _cps: Output::new(gc1109_cps_pin, Level::High, OutputConfig::default()),
            }
        }
        HeltecFrontendKind::Kct8103l => {
            log::info!("LoRa FEM detected: KCT8103L (GPIO5 CTX TX/RX switching enabled)");
            ModeControl::Kct8103l {
                ctx: Output::new(kct8103l_ctx_pin, Level::Low, OutputConfig::default()),
            }
        }
    };

    HELD_FRONTEND.lock(|held| {
        *held.borrow_mut() = Some(HeldFrontend {
            _power: power,
            _chip_enable: chip_enable,
            mode,
        });
    });

    kind
}

fn enter_transmit() {
    HELD_FRONTEND.lock(|held| {
        if let Some(HeldFrontend {
            mode: ModeControl::Kct8103l { ctx },
            ..
        }) = held.borrow_mut().as_mut()
        {
            ctx.set_high();
        }
    });
}

fn enter_receive() {
    HELD_FRONTEND.lock(|held| {
        if let Some(HeldFrontend {
            mode: ModeControl::Kct8103l { ctx },
            ..
        }) = held.borrow_mut().as_mut()
        {
            ctx.set_low();
        }
    });
}
