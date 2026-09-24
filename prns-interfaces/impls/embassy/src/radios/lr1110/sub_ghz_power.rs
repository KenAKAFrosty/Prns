use super::config::{
    HighPowerSelection, PowerAmplifierConfig, PowerAmplifierDutyCycle, PowerAmplifierSelection,
    PowerAmplifierSupply, PowerAmplifierTable,
};

const MINIMUM_OUTPUT_POWER_DBM: i8 = -17;
const HIGH_POWER_CHIP_OUTPUT_DBM: i8 = 22;

const CONFIGURATIONS: [PowerAmplifierConfig; 40] = [
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
    high_power(3, 7),
    high_power(4, 6),
    high_power(4, 7),
];

pub const SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE: PowerAmplifierTable =
    PowerAmplifierTable::new(MINIMUM_OUTPUT_POWER_DBM, &CONFIGURATIONS);

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
        chip_output_power_dbm: HIGH_POWER_CHIP_OUTPUT_DBM,
        selection: PowerAmplifierSelection::HighPower,
        supply: PowerAmplifierSupply::Battery,
        duty_cycle: PowerAmplifierDutyCycle::new(duty_cycle),
        high_power_selection: HighPowerSelection::new(high_power_selection),
    }
}
