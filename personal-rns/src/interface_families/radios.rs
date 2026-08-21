pub use prns_interfaces_embassy::radios::{LoRaRadio, RadioEvent, RadioRecovery, ReceivedAirFrame};

pub mod lr1110 {
    pub use prns_interfaces_embassy::radios::lr1110::{
        BoardConfig, Error, HighPowerSelection, Lr1110, PowerAmplifierConfig,
        PowerAmplifierDutyCycle, PowerAmplifierSelection, PowerAmplifierSupply,
        PowerAmplifierTable, ReceiveGain, ReceivedAirFrame, ReferenceClock, RegulatorMode,
        RfSwitchConfig, RfSwitchPins, TcxoStartupTime, TcxoVoltage, TransmitRampTime,
    };
}

pub mod sx126x {
    pub use prns_interfaces_embassy::radios::sx126x::{
        Bandwidth, BoardConfig, CodingRate, Error, ExternalPowerAmplifier, LoraPacket, Modulation,
        RadioConfig, ReceivedAirFrame, SpreadingFactor, Sx126x, TcxoVoltage,
    };
}
