#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcxoVoltage {
    V1_6 = 0x00,
    V1_7 = 0x01,
    V1_8 = 0x02,
    V2_2 = 0x03,
    V2_4 = 0x04,
    V2_7 = 0x05,
    V3_0 = 0x06,
    V3_3 = 0x07,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcxoStartupTime(u32);

impl TcxoStartupTime {
    const MAXIMUM_RTC_TICKS: u32 = 0x00ff_ffff;

    pub const fn from_rtc_ticks(rtc_ticks: u32) -> Self {
        assert!(rtc_ticks <= Self::MAXIMUM_RTC_TICKS);
        Self(rtc_ticks)
    }

    pub const fn rtc_ticks(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceClock {
    Crystal,
    Tcxo {
        voltage: TcxoVoltage,
        startup_time: TcxoStartupTime,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RegulatorMode {
    Ldo = 0x00,
    Dcdc = 0x01,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReceiveGain {
    PowerSaving = 0x00,
    Boosted = 0x01,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransmitRampTime {
    Us16 = 0x00,
    Us32 = 0x01,
    Us48 = 0x02,
    Us64 = 0x03,
    Us80 = 0x04,
    Us96 = 0x05,
    Us112 = 0x06,
    Us128 = 0x07,
    Us144 = 0x08,
    Us160 = 0x09,
    Us176 = 0x0a,
    Us192 = 0x0b,
    Us208 = 0x0c,
    Us240 = 0x0d,
    Us272 = 0x0e,
    Us304 = 0x0f,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfSwitchPins(u8);

impl RfSwitchPins {
    pub const NONE: Self = Self(0);
    pub const RFSW0: Self = Self(1 << 0);
    pub const RFSW1: Self = Self(1 << 1);
    pub const RFSW2: Self = Self(1 << 2);
    pub const RFSW3: Self = Self(1 << 3);
    pub const RFSW4: Self = Self(1 << 4);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfSwitchConfig {
    pub enabled: RfSwitchPins,
    pub standby: RfSwitchPins,
    pub receive: RfSwitchPins,
    pub transmit: RfSwitchPins,
    pub transmit_high_power: RfSwitchPins,
    pub transmit_high_frequency: RfSwitchPins,
    pub gnss: RfSwitchPins,
    pub wifi: RfSwitchPins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerAmplifierSelection {
    LowPower = 0x00,
    HighPower = 0x01,
    HighFrequency = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerAmplifierSupply {
    Regulator = 0x00,
    Battery = 0x01,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerAmplifierDutyCycle(u8);

impl PowerAmplifierDutyCycle {
    const MAXIMUM: u8 = 0x07;

    pub const fn new(value: u8) -> Self {
        assert!(value <= Self::MAXIMUM);
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighPowerSelection(u8);

impl HighPowerSelection {
    const MAXIMUM: u8 = 0x07;

    pub const fn new(value: u8) -> Self {
        assert!(value <= Self::MAXIMUM);
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerAmplifierConfig {
    pub chip_output_power_dbm: i8,
    pub selection: PowerAmplifierSelection,
    pub supply: PowerAmplifierSupply,
    pub duty_cycle: PowerAmplifierDutyCycle,
    pub high_power_selection: HighPowerSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerAmplifierTable {
    minimum_output_power_dbm: i8,
    configurations: &'static [PowerAmplifierConfig],
}

impl PowerAmplifierTable {
    const MAXIMUM_CONFIGURATIONS: usize = u8::MAX as usize + 1;

    pub const fn new(
        minimum_output_power_dbm: i8,
        configurations: &'static [PowerAmplifierConfig],
    ) -> Self {
        assert!(!configurations.is_empty());
        assert!(configurations.len() <= Self::MAXIMUM_CONFIGURATIONS);
        assert!(
            minimum_output_power_dbm as i16 + configurations.len() as i16 - 1 <= i8::MAX as i16
        );
        Self {
            minimum_output_power_dbm,
            configurations,
        }
    }

    pub const fn minimum_output_power_dbm(self) -> i8 {
        self.minimum_output_power_dbm
    }

    pub const fn maximum_output_power_dbm(self) -> i8 {
        (self.minimum_output_power_dbm as i16 + self.configurations.len() as i16 - 1) as i8
    }

    pub(super) fn configuration(self, output_power_dbm: i8) -> Option<PowerAmplifierConfig> {
        let index = i16::from(output_power_dbm) - i16::from(self.minimum_output_power_dbm);
        let Ok(index) = usize::try_from(index) else {
            return None;
        };
        self.configurations.get(index).copied()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BoardConfig {
    pub reference_clock: ReferenceClock,
    pub regulator: RegulatorMode,
    pub receive_gain: ReceiveGain,
    pub rf_switch: RfSwitchConfig,
    pub power_amplifier: PowerAmplifierTable,
    pub transmit_ramp_time: TransmitRampTime,
    pub external_receive_gain_db: u8,
}
