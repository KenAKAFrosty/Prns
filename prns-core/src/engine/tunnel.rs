use crate::crypto::sha256;
use crate::engine::{EgressSerializeError, EngineState};
use crate::identity::{IdentityHash, IdentitySigner};
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::{
    assemble_synthesize_payload, synthesize_signed_region, write_synthesize_wire_packet,
    RANDOM_HASH_LEN,
};
use crate::storage::StorageLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTunnelSynthesizeError {
    NoTransportId,
    TransportIdentityVanished,
    BufferTooShort,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn write_tunnel_synthesize(
        &self,
        interface: InterfaceId,
        random_hash: &[u8; RANDOM_HASH_LEN],
        buf: &mut [u8],
    ) -> Result<usize, WriteTunnelSynthesizeError> {
        let transport_id = self
            .transport_id
            .ok_or(WriteTunnelSynthesizeError::NoTransportId)?;
        let signer = self
            .held_identities
            .get(&IdentityHash::new(*transport_id.as_bytes()))
            .ok_or(WriteTunnelSynthesizeError::TransportIdentityVanished)?;

        let public_key = signer.public_key_bytes();
        let interface_hash = sha256(interface.as_bytes());
        let region = synthesize_signed_region(&public_key, &interface_hash, random_hash);
        let signature = signer.sign(&region);
        let payload = assemble_synthesize_payload(&region, &signature);
        write_synthesize_wire_packet(&payload, buf).map_err(
            |EgressSerializeError::BufferTooShort| WriteTunnelSynthesizeError::BufferTooShort,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::WriteTunnelSynthesizeError;
    use crate::crypto::sha256;
    use crate::engine::test_support::{
        fixed_secret_key, pin_transport_id, TestStorageLayout, TEST_TRANSPORT_ID,
    };
    use crate::engine::EngineState;
    use crate::interfaces::InterfaceId;
    use crate::routing::tunnel::{
        parse_synthesize_payload, INTERFACE_HASH_LEN, RANDOM_HASH_LEN, SYNTHESIZE_PAYLOAD_LEN,
    };
    use crate::wire::HEADER_MIN_LEN;

    #[test]
    fn a_transport_identity_signs_a_synthesize_that_verifies_against_its_own_key() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        state.set_transport_identity(&held).unwrap();

        let interface = InterfaceId::new([0xC1; 8]);
        let random = [0x11u8; RANDOM_HASH_LEN];
        let mut buf = [0u8; 256];
        let n = state
            .write_tunnel_synthesize(interface, &random, &mut buf)
            .expect("a held transport identity can synthesize");
        assert_eq!(n, HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN);

        let verified = parse_synthesize_payload(&buf[HEADER_MIN_LEN..n])
            .expect("the packet we signed verifies against the key it carries");
        let mut interface_hash = [0u8; INTERFACE_HASH_LEN];
        interface_hash.copy_from_slice(&sha256(interface.as_bytes()));
        assert_eq!(verified.interface_hash, interface_hash);
    }

    #[test]
    fn a_transport_id_whose_identity_is_not_held_cannot_synthesize() {
        let mut state = EngineState::<TestStorageLayout>::default();
        pin_transport_id(&mut state, TEST_TRANSPORT_ID);
        let mut buf = [0u8; 256];
        assert_eq!(
            state.write_tunnel_synthesize(
                InterfaceId::new([0x01; 8]),
                &[0u8; RANDOM_HASH_LEN],
                &mut buf
            ),
            Err(WriteTunnelSynthesizeError::TransportIdentityVanished),
        );
    }

    #[test]
    fn a_node_with_no_transport_role_cannot_synthesize() {
        let state = EngineState::<TestStorageLayout>::default();
        let mut buf = [0u8; 256];
        assert_eq!(
            state.write_tunnel_synthesize(
                InterfaceId::new([0x01; 8]),
                &[0u8; RANDOM_HASH_LEN],
                &mut buf
            ),
            Err(WriteTunnelSynthesizeError::NoTransportId),
        );
    }
}
