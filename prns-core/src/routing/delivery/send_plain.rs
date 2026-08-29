use crate::engine::{CommandId, CommandOutcome, EngineState, SendPlainPacket};
use crate::interfaces::AttachedInterfaces;
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
    pub fn ingest_send_plain_packet(
        &self,
        id: CommandId,
        send: SendPlainPacket,
        interfaces: AttachedInterfaces<'_>,
    ) -> CommandOutcome {
        if let Err(rejection) = send.target.admit(interfaces) {
            return CommandOutcome::SendPlainPacketRejected { id, rejection };
        }
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
    use crate::engine::test_support::routable_descriptor;
    use crate::engine::{
        CommandId, Directive, EgressTarget, EgressTargetRejection, EngineReaction, InstantMillis,
        IssuedCommand, PrnsCommand, SendPlainPacketPayload,
    };
    use crate::interfaces::{AttachedInterfaces, EgressCapability, InterfaceId};
    use crate::wire::{DestinationHash, WirePacketHeader, BROADCAST_MTU};

    const DESTINATION: DestinationHash = DestinationHash::new([0xA5; 16]);

    fn send(payload: &[u8]) -> SendPlainPacket {
        SendPlainPacket {
            destination: DESTINATION,
            target: EgressTarget::AllInterfaces,
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
    fn unavailable_selected_interfaces_are_rejected_before_egress() {
        let unknown = InterfaceId::new([0xB1; 8]);
        let mut unknown_command = send(b"plain");
        unknown_command.target = EgressTarget::Interface(unknown);
        let mut state = EngineState::<crate::engine::test_support::TestStorageLayout>::default();
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(9),
                    command: PrnsCommand::SendPlainPacket(unknown_command),
                },
                AttachedInterfaces::new(&[]),
            ),
            CommandOutcome::SendPlainPacketRejected {
                id: CommandId(9),
                rejection: EgressTargetRejection::UnknownInterface,
            },
        );

        let receive_only = InterfaceId::new([0xB2; 8]);
        let mut descriptor = routable_descriptor(receive_only);
        descriptor.capabilities.egress = EgressCapability::Disabled;
        let interfaces = [descriptor];
        let mut receive_only_command = send(b"plain");
        receive_only_command.target = EgressTarget::Interface(receive_only);
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(10),
                    command: PrnsCommand::SendPlainPacket(receive_only_command),
                },
                AttachedInterfaces::new(&interfaces),
            ),
            CommandOutcome::SendPlainPacketRejected {
                id: CommandId(10),
                rejection: EgressTargetRejection::InterfaceCannotTransmit,
            },
        );
    }

    #[test]
    fn plain_send_targets_only_the_selected_interface() {
        let first = InterfaceId::new([0xA1; 8]);
        let selected = InterfaceId::new([0xA2; 8]);
        let interfaces = [routable_descriptor(first), routable_descriptor(selected)];
        let mut command = send(b"plain");
        command.target = EgressTarget::Interface(selected);
        let mut emitted = std::vec::Vec::new();
        let mut state = EngineState::<crate::engine::test_support::TestStorageLayout>::default();

        let _ = state.ingest_command_into(
            IssuedCommand {
                id: CommandId(8),
                command: PrnsCommand::SendPlainPacket(command),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut |_| {},
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                    emitted.push(target);
                }
            },
        );

        assert_eq!(emitted, [selected]);
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
