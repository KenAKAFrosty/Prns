use crate::crypto::sha256;
use crate::engine::EngineState;
use crate::identity::{IdentityHash, IdentitySigner};
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::{
    assemble_synthesize_payload, synthesize_signed_region, write_synthesize_wire_packet,
    PersistedTunnelRow, SeedTunnelOutcome, RANDOM_HASH_LEN,
};
use crate::storage::StorageLayout;
use crate::wire::WireError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTunnelSynthesizeError {
    NoTransportId,
    TransportIdentityVanished,
    BufferTooShort,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn persisted_tunnel_rows(&self) -> impl Iterator<Item = PersistedTunnelRow> + '_ {
        self.tunnels.persisted_rows()
    }

    /// Unlike [`seed_route`](Self::seed_route) there is nothing to re-verify: a tunnel row carries no keys, so the worst a hostile store plants is warmth on a dead interface, bounded by the row's own expiry.
    pub fn seed_tunnel(&mut self, row: PersistedTunnelRow) -> SeedTunnelOutcome {
        let outcome = self.tunnels.seed_tunnel(row);
        if outcome == SeedTunnelOutcome::Seeded {
            self.routing_table.invalidate_route_expiries();
        }
        outcome
    }

    pub fn write_tunnel_synthesize(
        &self,
        interface: InterfaceId,
        random_hash: &[u8; RANDOM_HASH_LEN],
        buf: &mut [u8],
    ) -> Result<usize, WriteTunnelSynthesizeError> {
        let transport_id = self
            .network_transport_enabled()
            .then(|| self.transport_id())
            .flatten()
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
        write_synthesize_wire_packet(&payload, buf)
            .map_err(|WireError::BufferTooShort| WriteTunnelSynthesizeError::BufferTooShort)
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

    fn synthesize_wire(seed: u8) -> std::vec::Vec<u8> {
        use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};
        use crate::routing::tunnel::{
            assemble_synthesize_payload, synthesize_signed_region, write_synthesize_wire_packet,
            PUBLIC_KEY_LEN,
        };

        let secret = Ed25519SecretKey::new([seed; 32]);
        let signing_public = ed25519_public_key(&secret);
        let mut public_key = [0u8; PUBLIC_KEY_LEN];
        public_key[..32].copy_from_slice(&[seed ^ 0x5A; 32]);
        public_key[32..].copy_from_slice(&signing_public.0);
        let interface_hash = [0x9Du8; INTERFACE_HASH_LEN];
        let random = [0x42u8; RANDOM_HASH_LEN];
        let region = synthesize_signed_region(&public_key, &interface_hash, &random);
        let signature = ed25519_sign(&secret, &region);
        let payload = assemble_synthesize_payload(&region, &signature);
        let mut buf = std::vec![0u8; HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN];
        let n = write_synthesize_wire_packet(&payload, &mut buf).expect("frames");
        buf.truncate(n);
        buf
    }

    #[test]
    fn a_seeded_tunnel_repoints_seeded_routes_when_its_peer_reappears() {
        use crate::engine::test_support::{
            bytes_from_hex, routable_descriptor, transporting_node, RNS_1_4_2_ANNOUNCE,
        };
        use crate::engine::{InstantMillis, RouteSeedOutcome};
        use crate::interfaces::{AttachedInterfaces, InboundPacket};
        use crate::routing::tunnel::SeedTunnelOutcome;
        use crate::wire::DestinationHash;

        let mut before_reboot = transporting_node();
        let dest = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        let first_conn = InterfaceId::new([0xC1; 8]);
        let first_view = [routable_descriptor(first_conn)];
        let mut announce = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let _ = before_reboot.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: first_conn,
                bytes: &mut announce,
            },
            AttachedInterfaces::new(&first_view),
        );
        let mut synth = synthesize_wire(0xAB);
        let _ = before_reboot.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: first_conn,
                bytes: &mut synth,
            },
            AttachedInterfaces::new(&first_view),
        );

        let mut rebooted = transporting_node();
        for row in before_reboot.persisted_route_rows() {
            assert_eq!(
                rebooted.seed_route(&row, InstantMillis(0)),
                RouteSeedOutcome::Seeded,
            );
        }
        for row in before_reboot.persisted_tunnel_rows() {
            assert_eq!(rebooted.seed_tunnel(row), SeedTunnelOutcome::Seeded);
        }
        assert_eq!(
            rebooted
                .routing_table
                .path_row(&dest)
                .expect("the seeded route landed")
                .receiving_interface,
            first_conn,
        );

        let second_conn = InterfaceId::new([0xC2; 8]);
        let second_view = [routable_descriptor(second_conn)];
        let mut synth_again = synthesize_wire(0xAB);
        let _ = rebooted.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(3_000),
                source_interface: second_conn,
                bytes: &mut synth_again,
            },
            AttachedInterfaces::new(&second_view),
        );
        assert_eq!(
            rebooted
                .routing_table
                .path_row(&dest)
                .expect("the route survived the reboot")
                .receiving_interface,
            second_conn,
            "the peer's first synthesize after our reboot reads as a reappearance and repoints the seeded route",
        );
    }
}
