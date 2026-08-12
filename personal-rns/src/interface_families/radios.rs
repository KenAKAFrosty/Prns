pub mod sx126x {
    pub use prns_interfaces_embassy::radios::sx126x::{
        Bandwidth, BoardConfig, CodingRate, Error, LoraPacket, Modulation, RadioConfig,
        ReceivedAirFrame, SpreadingFactor, Sx126x, TcxoVoltage,
    };
}

pub mod lr1110 {
    pub use prns_interfaces_embassy::radios::lr1110::{
        Bandwidth, BoardConfig, CodingRate, Error, LoraPacket, Lr1110, Modulation, RadioConfig,
        RadioEvent, ReceivedAirFrame, SpreadingFactor, TcxoVoltage,
    };
}
