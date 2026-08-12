#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Port {
    P0,
    P1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pin {
    pub port: Port,
    pub index: u8,
}

impl Pin {
    const fn p0(index: u8) -> Self {
        Self {
            port: Port::P0,
            index,
        }
    }

    const fn p1(index: u8) -> Self {
        Self {
            port: Port::P1,
            index,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HardwareProfile {
    pub clocks: ClockProfile,
    pub power: PowerProfile,
    pub radio: RadioProfile,
    pub display: DisplayProfile,
    pub gnss: GnssProfile,
    pub battery: BatteryProfile,
    pub controls: ControlsProfile,
    pub buses: BusProfile,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClockProfile {
    pub high_frequency_hz: u32,
    pub low_frequency_hz: u32,
    pub low_frequency_crystal: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PowerProfile {
    pub peripheral_enable: Pin,
    pub peripheral_enable_active_high: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SpiPins {
    pub sck: Pin,
    pub mosi: Pin,
    pub miso: Option<Pin>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RadioProfile {
    pub chip: &'static str,
    pub spi: SpiPins,
    pub chip_select: Pin,
    pub dio1: Pin,
    pub busy: Pin,
    pub reset: Pin,
    pub dio2_controls_rf_switch: bool,
    pub dio3_tcxo_millivolts: u16,
    pub maximum_antenna_dbm: i8,
    pub front_end: FrontEndProfile,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrontEndProfile {
    pub chip: &'static str,
    pub power_enable: Pin,
    pub chip_enable: Pin,
    pub ctx: Pin,
    pub lna_gain_db: u8,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DisplayProfile {
    pub controller: &'static str,
    pub spi: SpiPins,
    pub chip_select: Pin,
    pub data_command: Pin,
    pub reset: Pin,
    pub backlight: Pin,
    pub backlight_active_low: bool,
    pub width: u16,
    pub height: u16,
    pub column_offset: u16,
    pub row_offset: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GnssProfile {
    pub chip: &'static str,
    pub mcu_rx: Pin,
    pub mcu_tx: Pin,
    pub enable: Pin,
    pub enable_active_low: bool,
    pub reset: Pin,
    pub reset_active_low: bool,
    pub pulse_per_second: Pin,
    pub baud: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BatteryProfile {
    pub analog_input: Pin,
    pub divider_enable: Pin,
    pub divider_enable_active_high: bool,
    pub reference_millivolts: u16,
    pub sample_resolution_bits: u8,
    /// Divider multiplier times 1,000; upstream implementations use approximately 4.90–4.916.
    pub divider_multiplier_milli: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlsProfile {
    pub user_button: Pin,
    pub status_led: Pin,
    pub status_led_active_high: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct I2cPins {
    pub sda: Pin,
    pub scl: Pin,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BusProfile {
    pub onboard_i2c: I2cPins,
    pub header_i2c: I2cPins,
    pub header_uart_rx: Pin,
    pub header_uart_tx: Pin,
}

pub(crate) const HARDWARE: HardwareProfile = HardwareProfile {
    clocks: ClockProfile {
        high_frequency_hz: 64_000_000,
        low_frequency_hz: 32_768,
        low_frequency_crystal: true,
    },
    power: PowerProfile {
        peripheral_enable: Pin::p0(26),
        peripheral_enable_active_high: true,
    },
    radio: RadioProfile {
        chip: "SX1262",
        spi: SpiPins {
            sck: Pin::p1(8),
            mosi: Pin::p0(11),
            miso: Some(Pin::p0(14)),
        },
        chip_select: Pin::p0(5),
        dio1: Pin::p0(21),
        busy: Pin::p0(19),
        reset: Pin::p0(16),
        dio2_controls_rf_switch: true,
        dio3_tcxo_millivolts: 1_800,
        maximum_antenna_dbm: 28,
        front_end: FrontEndProfile {
            chip: "KCT8103L",
            power_enable: Pin::p0(30),
            chip_enable: Pin::p0(12),
            ctx: Pin::p1(9),
            lna_gain_db: 21,
        },
    },
    display: DisplayProfile {
        controller: "ST7735S",
        spi: SpiPins {
            sck: Pin::p0(20),
            mosi: Pin::p0(17),
            miso: None,
        },
        chip_select: Pin::p0(22),
        data_command: Pin::p0(15),
        reset: Pin::p0(13),
        backlight: Pin::p1(12),
        backlight_active_low: true,
        width: 80,
        height: 160,
        column_offset: 24,
        row_offset: 0,
    },
    gnss: GnssProfile {
        chip: "UC6580",
        mcu_rx: Pin::p0(25),
        mcu_tx: Pin::p0(23),
        enable: Pin::p0(6),
        enable_active_low: true,
        reset: Pin::p1(14),
        reset_active_low: true,
        pulse_per_second: Pin::p1(11),
        baud: 115_200,
    },
    battery: BatteryProfile {
        analog_input: Pin::p0(3),
        divider_enable: Pin::p1(15),
        divider_enable_active_high: true,
        reference_millivolts: 3_000,
        sample_resolution_bits: 12,
        divider_multiplier_milli: 4_900,
    },
    controls: ControlsProfile {
        user_button: Pin::p1(10),
        status_led: Pin::p0(28),
        status_led_active_high: true,
    },
    buses: BusProfile {
        onboard_i2c: I2cPins {
            sda: Pin::p0(7),
            scl: Pin::p0(8),
        },
        header_i2c: I2cPins {
            sda: Pin::p0(4),
            scl: Pin::p0(27),
        },
        header_uart_rx: Pin::p0(9),
        header_uart_tx: Pin::p0(10),
    },
};
