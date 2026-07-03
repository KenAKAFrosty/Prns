use crate::crypto::{token_seal, TokenKey};
use crate::engine::commands::{CommandId, CommandOutcome, SendGroup};
use crate::engine::EngineState;
use crate::identity::ENCRYPTION_IV_LEN;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteSendGroupError {
    NoGroupKey,
    Seal,
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_group(&self, id: CommandId, send: SendGroup) -> CommandOutcome {
        if self.group_keys.key_for(&send.destination).is_some() {
            CommandOutcome::OwesSendGroup { id, send }
        } else {
            CommandOutcome::SendGroupRejected { id }
        }
    }

    pub fn write_commanded_send_group(
        &self,
        send: &SendGroup,
        iv: &[u8; ENCRYPTION_IV_LEN],
        buf: &mut [u8],
    ) -> Result<usize, WriteSendGroupError> {
        let key_bytes = self
            .group_keys
            .key_for(&send.destination)
            .ok_or(WriteSendGroupError::NoGroupKey)?;
        let key = TokenKey::from_derived(key_bytes).map_err(|_| WriteSendGroupError::Seal)?;

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: send.destination,
            context: WireContext::None,
        };
        let header_len = header
            .write(buf)
            .map_err(|_| WriteSendGroupError::Serialize)?;
        let sealed = token_seal(&key, iv, &send.payload, &mut buf[header_len..])
            .map_err(|_| WriteSendGroupError::Seal)?;
        Ok(header_len + sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::commands::{
        CommandId, EngineCommand, IssuedCommand, SendGroup, SendGroupPayload,
    };
    use crate::engine::test_support::*;
    use crate::engine::IngestPacketOutcome;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentitySigner;
    use crate::routing::delivery::Delivery;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex"))
            .collect()
    }

    fn group_send(destination: DestinationHash, plaintext: &[u8]) -> IssuedCommand {
        let mut payload = SendGroupPayload::new();
        payload.extend_from_slice(plaintext).unwrap();
        IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::SendGroup(SendGroup {
                destination,
                payload,
            }),
        }
    }

    #[test]
    fn a_send_to_a_registered_group_owes_the_send_else_is_rejected() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let group = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &hx(GROUP_KEY),
            )
            .unwrap();

        let CommandOutcome::OwesSendGroup {
            id: CommandId(7), ..
        } = state.ingest_command(group_send(group, b"hi"), &[])
        else {
            panic!("a registered group owes its send");
        };

        assert_eq!(
            state.ingest_command(group_send(DestinationHash::new([0x99; 16]), b"hi"), &[]),
            CommandOutcome::SendGroupRejected { id: CommandId(7) },
        );
    }

    #[test]
    fn a_commanded_group_send_seals_byte_identically_to_rns_1_3_5_and_we_open_it() {
        // Vector minted live against Python RNS 1.3.5: the same GROUP as the
        // delivery test, sealing b"group-send-hi" under a pinned IV.
        const TOKEN: &str = "44444444444444444444444444444444ce215bf3e6687202ac7d97a8deaee7c392356d2cfc86276758362f19ccb937d989e1391c477ae92487a0011dbe786123";

        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &hx(GROUP_KEY),
            )
            .unwrap();

        let mut payload = SendGroupPayload::new();
        payload.extend_from_slice(b"group-send-hi").unwrap();
        let send = SendGroup {
            destination,
            payload,
        };

        let mut buf = [0u8; BROADCAST_MTU];
        let iv = [0x44u8; ENCRYPTION_IV_LEN];
        let len = state
            .write_commanded_send_group(&send, &iv, &mut buf)
            .unwrap();
        assert!(
            buf[..len].ends_with(&hx(TOKEN)),
            "our sealed token is byte-identical to RNS Token.encrypt",
        );

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Group(group),
            ..
        } = state.ingest_packet(
            plain_data_packet(&mut buf[..len]),
            TEST_ENTROPY,
            &transporting_view(),
        )
        else {
            panic!("our own GROUP send round-trips back through delivery");
        };
        assert_eq!(group.plaintext, b"group-send-hi");
        assert_eq!(group.destination, destination);
    }
}
