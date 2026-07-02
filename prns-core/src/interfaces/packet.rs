use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

#[derive(Debug, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub bytes: &'a mut [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPacket<'a> {
    pub bytes: &'a [u8],
}

impl<'a> OutboundPacket<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}
