use crate::crypto::token_seal;
use crate::engine::EngineState;
use crate::engine::{CommandId, CommandOutcome, SendGroup, SendGroupRejection};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};

pub const SEND_GROUP_ENTROPY_LEN: usize = ENCRYPTION_IV_LEN;

/// Move-only and never shown; consuming it seals exactly one packet, so one draw can never key two.
pub struct SendGroupEntropy([u8; SEND_GROUP_ENTROPY_LEN]);

impl SendGroupEntropy {
    pub const LEN: usize = SEND_GROUP_ENTROPY_LEN;

    pub const fn new(bytes: [u8; SEND_GROUP_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    fn into_iv(self) -> [u8; ENCRYPTION_IV_LEN] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGroupWriteError {
    NoGroupKey,
    Seal,
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_group(&self, id: CommandId, send: SendGroup) -> CommandOutcome {
        if self.group_keys.key_for(&send.destination).is_some() {
            CommandOutcome::OwesSendGroup { id, send }
        } else {
            CommandOutcome::SendGroupRejected {
                id,
                rejection: SendGroupRejection::NoGroupKey,
            }
        }
    }

    /// Intentional deviation from RNS 1.4.2 `Transport.outbound`, which excludes only PLAIN sends from its receipt gate: a GROUP destination carries no identity to prove with, so the reference's GROUP receipt can only ever time out, and we track none.
    pub fn write_commanded_send_group(
        &self,
        send: &SendGroup,
        entropy: SendGroupEntropy,
        buf: &mut [u8],
    ) -> Result<usize, SendGroupWriteError> {
        let key = self
            .group_keys
            .key_for(&send.destination)
            .ok_or(SendGroupWriteError::NoGroupKey)?
            .as_token_key();

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: send.destination.to_address(),
            context: WireContext::None,
        };
        let header_len = header
            .write(buf)
            .map_err(|_| SendGroupWriteError::Serialize)?;
        let iv = entropy.into_iv();
        let sealed = token_seal(&key, &iv, &send.payload, &mut buf[header_len..])
            .map_err(|_| SendGroupWriteError::Seal)?;
        Ok(header_len + sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::IngestPacketOutcome;
    use crate::engine::{CommandId, IssuedCommand, PrnsCommand, SendGroup, SendGroupPayload};
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentitySigner;
    use crate::interfaces::AttachedInterfaces;
    use crate::routing::delivery::Delivery;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex"))
            .collect()
    }

    fn group_send(destination: DestinationHash, plaintext: &[u8]) -> IssuedCommand {
        let mut payload = SendGroupPayload::new();
        payload.extend_from_slice(plaintext).unwrap();
        IssuedCommand {
            id: CommandId(7),
            command: PrnsCommand::SendGroup(SendGroup {
                destination,
                payload,
            }),
        }
    }

    #[test]
    fn a_send_to_a_registered_group_owes_the_send_else_is_rejected() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let group = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &bytes_from_hex(GROUP_KEY),
            )
            .unwrap();

        let CommandOutcome::OwesSendGroup {
            id: CommandId(7), ..
        } = state.ingest_command(group_send(group, b"hi"), AttachedInterfaces::new(&[]))
        else {
            panic!("a registered group owes its send");
        };

        assert_eq!(
            state.ingest_command(
                group_send(DestinationHash::new([0x99; 16]), b"hi"),
                AttachedInterfaces::new(&[])
            ),
            CommandOutcome::SendGroupRejected {
                id: CommandId(7),
                rejection: SendGroupRejection::NoGroupKey,
            },
        );
    }

    #[test]
    fn a_commanded_group_send_seals_byte_identically_to_rns_1_4_2_and_we_open_it() {
        // Vector minted live against Python RNS 1.3.5 and revalidated with 1.4.2: the same GROUP as the delivery test, sealing b"group-send-hi" under a pinned IV.
        const TOKEN: &str = "44444444444444444444444444444444ce215bf3e6687202ac7d97a8deaee7c392356d2cfc86276758362f19ccb937d989e1391c477ae92487a0011dbe786123";

        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &bytes_from_hex(GROUP_KEY),
            )
            .unwrap();

        let mut payload = SendGroupPayload::new();
        payload.extend_from_slice(b"group-send-hi").unwrap();
        let send = SendGroup {
            destination,
            payload,
        };

        let mut buf = [0u8; BROADCAST_MTU];
        let entropy = SendGroupEntropy::new([0x44u8; SendGroupEntropy::LEN]);
        let len = state
            .write_commanded_send_group(&send, entropy, &mut buf)
            .unwrap();
        assert!(
            buf[..len].ends_with(&bytes_from_hex(TOKEN)),
            "our sealed token is byte-identical to RNS Token.encrypt",
        );

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Group(group),
            ..
        } = state.ingest_packet_with(
            plain_data_packet(&mut buf[..len]),
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        )
        else {
            panic!("our own GROUP send round-trips back through delivery");
        };
        assert_eq!(group.plaintext, b"group-send-hi");
        assert_eq!(group.destination, destination);
    }
}
