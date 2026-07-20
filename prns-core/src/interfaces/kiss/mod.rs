mod protocol;
#[cfg(feature = "alloc")]
mod transmission;

pub use protocol::{
    configured_policy, descriptor, Decoder, TncConfig, DEFAULTS, DEFAULT_PERSISTENCE,
    DEFAULT_PREAMBLE_MS, DEFAULT_SLOTTIME_MS, DEFAULT_TXTAIL_MS, FRAMED_LEN, KISS_BITRATE_BPS,
    KISS_FRAME_LEN, KISS_HW_MTU, READ_BUF_LEN,
};
#[cfg(feature = "alloc")]
pub use transmission::{
    EmptyStationIdentification, KissTransmissionControl, ReadyCommandFlowControl, ReadyTimeout,
    StationIdInterval, StationIdWireFormat, StationIdentification, Transmission,
};
