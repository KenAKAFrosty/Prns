use personal_rns::radios::lr1110::{
    BoardConfig, Lr11xxPart, ReceiveGain, ReferenceClock, RegulatorMode, RfSwitchConfig,
    RfSwitchPins, TcxoStartupTime, TcxoVoltage, TransmitRampTime,
    SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE,
};

const TCXO_STARTUP_RTC_TICKS: u32 = 164;
const EXTERNAL_RECEIVE_GAIN_DB: u8 = 0;

const RECEIVE_SWITCH: RfSwitchPins = RfSwitchPins::RFSW0;
const TRANSMIT_SWITCH: RfSwitchPins = RfSwitchPins::RFSW1;
const ENABLED_SWITCHES: RfSwitchPins = RECEIVE_SWITCH.union(TRANSMIT_SWITCH);

pub(super) fn board_config() -> BoardConfig {
    BoardConfig {
        part: Lr11xxPart::Lr1121,
        reference_clock: ReferenceClock::Tcxo {
            voltage: TcxoVoltage::V3_0,
            startup_time: TcxoStartupTime::from_rtc_ticks(TCXO_STARTUP_RTC_TICKS),
        },
        regulator: RegulatorMode::Dcdc,
        receive_gain: ReceiveGain::Boosted,
        rf_switch: RfSwitchConfig {
            enabled: ENABLED_SWITCHES,
            standby: RfSwitchPins::NONE,
            receive: RECEIVE_SWITCH,
            transmit: TRANSMIT_SWITCH,
            transmit_high_power: TRANSMIT_SWITCH,
            transmit_high_frequency: RfSwitchPins::NONE,
            gnss: RfSwitchPins::NONE,
            wifi: RfSwitchPins::NONE,
        },
        power_amplifier: SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE,
        transmit_ramp_time: TransmitRampTime::Us48,
        external_receive_gain_db: EXTERNAL_RECEIVE_GAIN_DB,
    }
}
