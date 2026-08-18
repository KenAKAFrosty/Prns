use personal_rns::radios::lr1110::{
    BoardConfig, HighPowerSelection, PowerAmplifierConfig, PowerAmplifierDutyCycle,
    PowerAmplifierSelection, PowerAmplifierSupply, PowerAmplifierTable, ReceiveGain,
    ReferenceClock, RegulatorMode, RfSwitchConfig, RfSwitchPins, TcxoStartupTime, TcxoVoltage,
    TransmitRampTime,
};

const MINIMUM_OUTPUT_POWER_DBM: i8 = -17;
const TCXO_STARTUP_RTC_TICKS: u32 = 164;
const EXTERNAL_RECEIVE_GAIN_DB: u8 = 0;

const RECEIVE_SWITCHES: RfSwitchPins = RfSwitchPins::RFSW0.union(RfSwitchPins::RFSW3);
const TRANSMIT_SWITCHES: RfSwitchPins = RfSwitchPins::RFSW0
    .union(RfSwitchPins::RFSW1)
    .union(RfSwitchPins::RFSW3);
const HIGH_POWER_TRANSMIT_SWITCHES: RfSwitchPins = RfSwitchPins::RFSW1.union(RfSwitchPins::RFSW3);
const ENABLED_SWITCHES: RfSwitchPins = RfSwitchPins::RFSW0
    .union(RfSwitchPins::RFSW1)
    .union(RfSwitchPins::RFSW2)
    .union(RfSwitchPins::RFSW3);

const POWER_AMPLIFIER_CONFIGURATIONS: [PowerAmplifierConfig; 40] = [
    low_power(-15, 0),
    low_power(-14, 0),
    low_power(-13, 0),
    low_power(-12, 0),
    low_power(-11, 0),
    low_power(-9, 0),
    low_power(-8, 0),
    low_power(-7, 0),
    low_power(-6, 0),
    low_power(-5, 0),
    low_power(-4, 0),
    low_power(-3, 0),
    low_power(-2, 0),
    low_power(-1, 0),
    low_power(0, 0),
    low_power(1, 0),
    low_power(2, 0),
    low_power(3, 0),
    low_power(3, 1),
    low_power(4, 1),
    low_power(7, 0),
    low_power(8, 0),
    low_power(9, 0),
    low_power(10, 0),
    low_power(12, 0),
    low_power(13, 0),
    low_power(14, 0),
    low_power(13, 1),
    low_power(13, 2),
    low_power(14, 2),
    low_power(14, 3),
    low_power(14, 4),
    low_power(14, 7),
    high_power(1, 4),
    high_power(2, 4),
    high_power(1, 6),
    high_power(3, 5),
    high_power(4, 7),
    high_power(5, 7),
    high_power(6, 7),
];

const fn low_power(chip_output_power_dbm: i8, duty_cycle: u8) -> PowerAmplifierConfig {
    PowerAmplifierConfig {
        chip_output_power_dbm,
        selection: PowerAmplifierSelection::LowPower,
        supply: PowerAmplifierSupply::Regulator,
        duty_cycle: PowerAmplifierDutyCycle::new(duty_cycle),
        high_power_selection: HighPowerSelection::new(0),
    }
}

const fn high_power(duty_cycle: u8, high_power_selection: u8) -> PowerAmplifierConfig {
    PowerAmplifierConfig {
        chip_output_power_dbm: 22,
        selection: PowerAmplifierSelection::HighPower,
        supply: PowerAmplifierSupply::Battery,
        duty_cycle: PowerAmplifierDutyCycle::new(duty_cycle),
        high_power_selection: HighPowerSelection::new(high_power_selection),
    }
}

pub(super) fn board_config() -> BoardConfig {
    BoardConfig {
        reference_clock: ReferenceClock::Tcxo {
            voltage: TcxoVoltage::V3_0,
            startup_time: TcxoStartupTime::from_rtc_ticks(TCXO_STARTUP_RTC_TICKS),
        },
        regulator: RegulatorMode::Dcdc,
        receive_gain: ReceiveGain::Boosted,
        rf_switch: RfSwitchConfig {
            enabled: ENABLED_SWITCHES,
            standby: RfSwitchPins::NONE,
            receive: RECEIVE_SWITCHES,
            transmit: TRANSMIT_SWITCHES,
            transmit_high_power: HIGH_POWER_TRANSMIT_SWITCHES,
            transmit_high_frequency: RfSwitchPins::NONE,
            gnss: RfSwitchPins::RFSW2,
            wifi: RfSwitchPins::NONE,
        },
        power_amplifier: PowerAmplifierTable::new(
            MINIMUM_OUTPUT_POWER_DBM,
            &POWER_AMPLIFIER_CONFIGURATIONS,
        ),
        transmit_ramp_time: TransmitRampTime::Us48,
        external_receive_gain_db: EXTERNAL_RECEIVE_GAIN_DB,
    }
}
