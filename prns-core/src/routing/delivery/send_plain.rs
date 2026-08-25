use crate::engine::{CommandId, CommandOutcome, EngineState, SendPlainPacket};
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPlainPacketWriteError {
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_plain_packet(&self, id: CommandId, send: SendPlainPacket) -> CommandOutcome {
        CommandOutcome::OwesSendPlainPacket { id, send }
    }

    pub fn write_commanded_send_plain_packet(
        &self,
        send: &SendPlainPacket,
        buf: &mut [u8],
    ) -> Result<usize, SendPlainPacketWriteError> {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: send.destination.to_address(),
            context: WireContext::None,
        };
        let header_len = header
            .write(buf)
            .map_err(|_| SendPlainPacketWriteError::Serialize)?;
        let payload_end = header_len + send.payload.len();
        let Some(payload) = buf.get_mut(header_len..payload_end) else {
            return Err(SendPlainPacketWriteError::Serialize);
        };
        payload.copy_from_slice(&send.payload);
        Ok(payload_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{CommandId, IssuedCommand, PrnsCommand, SendPlainPacketPayload};
    use crate::interfaces::AttachedInterfaces;
    use crate::wire::{DestinationHash, WirePacketHeader, BROADCAST_MTU};

    const DESTINATION: DestinationHash = DestinationHash::new([0xA5; 16]);

    fn send(payload: &[u8]) -> SendPlainPacket {
        SendPlainPacket {
            destination: DESTINATION,
            payload: SendPlainPacketPayload::from_slice(payload).unwrap(),
        }
    }

    #[test]
    fn plain_send_is_accepted_without_a_route_or_identity() {
        let mut state = EngineState::<crate::engine::test_support::TestStorageLayout>::default();
        let command = send(b"plain");
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: PrnsCommand::SendPlainPacket(command.clone()),
                },
                AttachedInterfaces::new(&[]),
            ),
            CommandOutcome::OwesSendPlainPacket {
                id: CommandId(7),
                send: command,
            }
        );
    }

    #[test]
    fn plain_send_writes_an_unencrypted_rns_data_packet() {
        let state = EngineState::<crate::engine::test_support::TestStorageLayout>::default();
        let mut buf = [0u8; BROADCAST_MTU];
        let len = state
            .write_commanded_send_plain_packet(&send(b"plain-\0-\xff"), &mut buf)
            .unwrap();
        let (header, payload) = WirePacketHeader::parse(&buf[..len]).unwrap();
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.address, DESTINATION.to_address());
        assert_eq!(payload, b"plain-\0-\xff");
    }
}
