use personal_rns::radios::lr1110::{
    BoardConfig, Lr11xxPart, ReceiveGain, ReferenceClock, RegulatorMode, RfSwitchConfig,
    RfSwitchPins, TcxoStartupTime, TcxoVoltage, TransmitRampTime,
    SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE,
};

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

pub(super) fn board_config() -> BoardConfig {
    BoardConfig {
        part: Lr11xxPart::Lr1110,
        reference_clock: ReferenceClock::Tcxo {
            voltage: TcxoVoltage::V1_6,
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
        power_amplifier: SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE,
        transmit_ramp_time: TransmitRampTime::Us48,
        external_receive_gain_db: EXTERNAL_RECEIVE_GAIN_DB,
    }
}
