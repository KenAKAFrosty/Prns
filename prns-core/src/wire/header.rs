use super::address::{TransportId, WireAddress};
use super::context::WireContext;
use super::flags::{ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType};
use super::limits::TRUNCATED_HASH_BYTE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    BufferTooShort,
}

/// Note that `transport_id.is_some()` *is* the Type-1/Type-2 distinction, so the two can never disagree.
///
/// ```text
/// [flags:1][hops:1] ( [transport_id:16] )? [destination:16][context:1] [payload…]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WirePacketHeader {
    pub ifac_flag: IfacFlag,
    pub context_flag: ContextFlag,
    pub propagation: PropagationType,
    pub destination_type: DestinationType,
    pub packet_type: PacketType,
    pub hops: u8,
    pub transport_id: Option<TransportId>,
    pub address: WireAddress,
    pub context: WireContext,
}

impl WirePacketHeader {
    pub fn parse(bytes: &[u8]) -> Result<(WirePacketHeader, &[u8]), WireError> {
        let (&meta, rest) = bytes.split_first().ok_or(WireError::BufferTooShort)?;
        let (&hops, rest) = rest.split_first().ok_or(WireError::BufferTooShort)?;

        let is_type_2 = (meta >> 6) & 0b1 == 0b1;
        let ifac_flag = IfacFlag::from_bits(meta >> 7);
        let context_flag = ContextFlag::from_bits(meta >> 5);
        let propagation = PropagationType::from_bits(meta >> 4);
        let destination_type = DestinationType::from_bits(meta >> 2);
        let packet_type = PacketType::from_bits(meta);

        let (transport_id, rest) = if is_type_2 {
            let (id, rest) = rest.split_first_chunk().ok_or(WireError::BufferTooShort)?;
            (Some(TransportId::new(*id)), rest)
        } else {
            (None, rest)
        };

        let (address, rest) = rest.split_first_chunk().ok_or(WireError::BufferTooShort)?;
        let (&context, rest) = rest.split_first().ok_or(WireError::BufferTooShort)?;

        let header = WirePacketHeader {
            ifac_flag,
            context_flag,
            propagation,
            destination_type,
            packet_type,
            hops,
            transport_id,
            address: WireAddress::new(*address),
            context: WireContext::from_byte(context),
        };
        Ok((header, rest))
    }

    pub fn write(&self, buf: &mut [u8]) -> Result<usize, WireError> {
        let transport_len = if self.transport_id.is_some() {
            TRUNCATED_HASH_BYTE_LEN
        } else {
            0
        };
        let header_len = 2 + transport_len + TRUNCATED_HASH_BYTE_LEN + 1;
        if buf.len() < header_len {
            return Err(WireError::BufferTooShort);
        }

        let header_type_bit = u8::from(self.transport_id.is_some());
        buf[0] = ((self.ifac_flag as u8) << 7)
            | (header_type_bit << 6)
            | ((self.context_flag as u8) << 5)
            | ((self.propagation as u8) << 4)
            | ((self.destination_type as u8) << 2)
            | (self.packet_type as u8);
        buf[1] = self.hops;

        let mut offset = 2;
        if let Some(transport_id) = &self.transport_id {
            buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(transport_id.as_bytes());
            offset += TRUNCATED_HASH_BYTE_LEN;
        }
        buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(self.address.as_bytes());
        offset += TRUNCATED_HASH_BYTE_LEN;
        buf[offset] = self.context.to_byte();
        offset += 1;

        Ok(offset)
    }
}
