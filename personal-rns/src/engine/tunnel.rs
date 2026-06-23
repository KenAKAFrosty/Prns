use crate::crypto::sha256;
use crate::engine::EngineState;
use crate::identity::{IdentityHash, IdentitySigner};
use crate::routing::tunnel::{
    assemble_synthesize_payload, synthesize_signed_region, write_synthesize_wire_packet,
    PUBLIC_KEY_LEN, RANDOM_HASH_LEN,
};
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    pub fn write_tunnel_synthesize(
        &self,
        channel_tag: &[u8],
        random_hash: &[u8; RANDOM_HASH_LEN],
        buf: &mut [u8],
    ) -> Option<usize> {
        let transport_id = self.transport_id?;
        let signer = self
            .held_identities
            .get(&IdentityHash::new(*transport_id.as_bytes()))?;

        let mut public_key = [0u8; PUBLIC_KEY_LEN];
        public_key[..32].copy_from_slice(signer.encryption_public_key().as_bytes());
        public_key[32..].copy_from_slice(signer.signing_public_key().as_bytes());

        let interface_hash = sha256(channel_tag);
        let region = synthesize_signed_region(&public_key, &interface_hash, random_hash);
        let signature = signer.sign(&region);
        let payload = assemble_synthesize_payload(&region, &signature);
        write_synthesize_wire_packet(&payload, buf).ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::crypto::sha256;
    use crate::engine::test_support::{fixed_secret_key, Cap};
    use crate::engine::EngineState;
    use crate::routing::tunnel::{
        parse_synthesize_payload, INTERFACE_HASH_LEN, RANDOM_HASH_LEN, SYNTHESIZE_PAYLOAD_LEN,
    };
    use crate::wire::HEADER_MIN_LEN;

    #[test]
    fn a_transport_identity_signs_a_synthesize_that_verifies_against_its_own_key() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        state.set_transport_identity(&held).unwrap();

        let channel_tag = b"hub.example.com:4965";
        let random = [0x11u8; RANDOM_HASH_LEN];
        let mut buf = [0u8; 256];
        let n = state
            .write_tunnel_synthesize(channel_tag, &random, &mut buf)
            .expect("a held transport identity can synthesize");
        assert_eq!(n, HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN);

        let verified = parse_synthesize_payload(&buf[HEADER_MIN_LEN..n])
            .expect("the packet we signed verifies against the key it carries");
        let mut interface_hash = [0u8; INTERFACE_HASH_LEN];
        interface_hash.copy_from_slice(&sha256(channel_tag));
        assert_eq!(verified.interface_hash, interface_hash);
    }

    #[test]
    fn a_bare_transport_id_with_no_held_key_cannot_synthesize() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state.set_transport_id(crate::engine::test_support::TEST_TRANSPORT_ID);
        let mut buf = [0u8; 256];
        assert!(state
            .write_tunnel_synthesize(b"hub:1", &[0u8; RANDOM_HASH_LEN], &mut buf)
            .is_none());
    }

    #[test]
    fn a_node_with_no_transport_role_cannot_synthesize() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        let mut buf = [0u8; 256];
        assert!(state
            .write_tunnel_synthesize(b"hub:1", &[0u8; RANDOM_HASH_LEN], &mut buf)
            .is_none());
    }
}
