/// The trailing context byte: a sub-type tag the engine routes on. Exhaustive over the values RNS defines, plus `Unknown(u8)` so an unrecognised byte round-trips unchanged (RNS preserves unknown context bytes). Decoding only ever yields `Unknown` for bytes outside the named set, so a parsed value is always canonical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireContext {
    None,
    Resource,
    ResourceAdvertisement,
    ResourceRequest,
    ResourceHashUpdate,
    ResourceProof,
    ResourceInitiatorCancel,
    ResourceReceiverCancel,
    CacheRequest,
    Request,
    Response,
    PathResponse,
    Command,
    CommandStatus,
    Channel,
    KeepAlive,
    LinkIdentify,
    LinkClose,
    LinkProof,
    LinkRtt,
    LinkRequestProof,
    Unknown(u8),
}

impl WireContext {
    pub(super) const fn from_byte(byte: u8) -> Self {
        match byte {
            0x00 => Self::None,
            0x01 => Self::Resource,
            0x02 => Self::ResourceAdvertisement,
            0x03 => Self::ResourceRequest,
            0x04 => Self::ResourceHashUpdate,
            0x05 => Self::ResourceProof,
            0x06 => Self::ResourceInitiatorCancel,
            0x07 => Self::ResourceReceiverCancel,
            0x08 => Self::CacheRequest,
            0x09 => Self::Request,
            0x0A => Self::Response,
            0x0B => Self::PathResponse,
            0x0C => Self::Command,
            0x0D => Self::CommandStatus,
            0x0E => Self::Channel,
            0xFA => Self::KeepAlive,
            0xFB => Self::LinkIdentify,
            0xFC => Self::LinkClose,
            0xFD => Self::LinkProof,
            0xFE => Self::LinkRtt,
            0xFF => Self::LinkRequestProof,
            other => Self::Unknown(other),
        }
    }

    pub const fn to_byte(self) -> u8 {
        match self {
            Self::None => 0x00,
            Self::Resource => 0x01,
            Self::ResourceAdvertisement => 0x02,
            Self::ResourceRequest => 0x03,
            Self::ResourceHashUpdate => 0x04,
            Self::ResourceProof => 0x05,
            Self::ResourceInitiatorCancel => 0x06,
            Self::ResourceReceiverCancel => 0x07,
            Self::CacheRequest => 0x08,
            Self::Request => 0x09,
            Self::Response => 0x0A,
            Self::PathResponse => 0x0B,
            Self::Command => 0x0C,
            Self::CommandStatus => 0x0D,
            Self::Channel => 0x0E,
            Self::KeepAlive => 0xFA,
            Self::LinkIdentify => 0xFB,
            Self::LinkClose => 0xFC,
            Self::LinkProof => 0xFD,
            Self::LinkRtt => 0xFE,
            Self::LinkRequestProof => 0xFF,
            Self::Unknown(byte) => byte,
        }
    }
}
