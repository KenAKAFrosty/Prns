pub mod sx126x {
    pub use prns_interfaces_embassy::radios::sx126x::{
        Bandwidth, BoardConfig, CodingRate, Error, LoraPacket, Modulation, RadioConfig,
        ReceivedAirFrame, SpreadingFactor, Sx126x, TcxoVoltage,
    };
}
