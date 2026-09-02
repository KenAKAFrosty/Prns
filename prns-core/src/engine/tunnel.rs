use crate::crypto::{sha256, Ed25519SecretKey, Ed25519Signature};
use crate::engine::{Directive, EngineReaction, EngineState, WakeSchedule, WakeSchedules};
use crate::identity::{IdentityHash, IdentitySigner};
use crate::interfaces::InterfaceId;
use crate::routing::tunnel::{
    assemble_synthesize_payload, synthesize_signed_region, write_synthesize_wire_packet,
    PersistedTunnelRow, SeedTunnelOutcome, TunnelSynthesizeVerification,
    TunnelSynthesizeVerifyOwed, TunnelTransition, PUBLIC_KEY_LEN, RANDOM_HASH_LEN,
    SIGNED_REGION_LEN, TUNNEL_TIMEOUT_MS,
};
use crate::storage::StorageLayout;
use crate::wire::WireError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTunnelSynthesizeError {
    NoTransportId,
    TransportIdentityVanished,
    BufferTooShort,
}

/// A tunnel synthesis whose policy inputs are complete and whose signature may be produced by
/// the surrounding runtime.
#[repr(C)]
pub struct TunnelSynthesizeSignOwed {
    pub interface: InterfaceId,
    pub transport_identity: IdentityHash,
    pub public_key: [u8; PUBLIC_KEY_LEN],
    pub signed_region: [u8; SIGNED_REGION_LEN],
    pub signing_secret: Ed25519SecretKey,
}

#[repr(C)]
pub struct TunnelSynthesizeSignCompleted {
    pub owed: TunnelSynthesizeSignOwed,
    pub signature: Ed25519Signature,
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
        let transport_identity = IdentityHash::new(*transport_id.as_bytes());
        let signer = self
            .held_identities
            .get(&transport_identity)
            .ok_or(WriteTunnelSynthesizeError::TransportIdentityVanished)?;
        let public_key = signer.public_key_bytes();
        let signed_region =
            synthesize_signed_region(&public_key, &sha256(interface.as_bytes()), random_hash);
        write_tunnel_synthesize_from_signature(&signed_region, &signer.sign(&signed_region), buf)
    }

    pub fn prepare_tunnel_synthesize_sign(
        &self,
        interface: InterfaceId,
        random_hash: [u8; RANDOM_HASH_LEN],
    ) -> Result<TunnelSynthesizeSignOwed, WriteTunnelSynthesizeError> {
        let transport_id = self
            .network_transport_enabled()
            .then(|| self.transport_id())
            .flatten()
            .ok_or(WriteTunnelSynthesizeError::NoTransportId)?;
        let transport_identity = IdentityHash::new(*transport_id.as_bytes());
        let signer = self
            .held_identities
            .get(&transport_identity)
            .ok_or(WriteTunnelSynthesizeError::TransportIdentityVanished)?;
        let public_key = signer.public_key_bytes();
        Ok(TunnelSynthesizeSignOwed {
            interface,
            transport_identity,
            public_key,
            signed_region: synthesize_signed_region(
                &public_key,
                &sha256(interface.as_bytes()),
                &random_hash,
            ),
            signing_secret: signer.signing_secret_clone(),
        })
    }

    pub fn resume_tunnel_synthesize_sign(
        &self,
        completed: TunnelSynthesizeSignCompleted,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<(), WriteTunnelSynthesizeError> {
        let TunnelSynthesizeSignCompleted { owed, signature } = completed;
        let transport_id = self
            .network_transport_enabled()
            .then(|| self.transport_id())
            .flatten()
            .ok_or(WriteTunnelSynthesizeError::NoTransportId)?;
        if IdentityHash::new(*transport_id.as_bytes()) != owed.transport_identity {
            return Err(WriteTunnelSynthesizeError::TransportIdentityVanished);
        }
        let signer = self
            .held_identities
            .get(&owed.transport_identity)
            .ok_or(WriteTunnelSynthesizeError::TransportIdentityVanished)?;
        if signer.public_key_bytes() != owed.public_key {
            return Err(WriteTunnelSynthesizeError::TransportIdentityVanished);
        }
        let mut frame = [0u8; crate::wire::BROADCAST_MTU];
        let wire_bytes =
            write_tunnel_synthesize_from_signature(&owed.signed_region, &signature, &mut frame)?;
        sink(EngineReaction::Directive(Directive::Send {
            target: owed.interface,
            bytes: &frame[..wire_bytes],
        }));
        Ok(())
    }

    /// Applies a verified tunnel synthesis after the runtime completes its signature check.
    pub fn resume_tunnel_synthesize_verify(
        &mut self,
        owed: TunnelSynthesizeVerifyOwed,
        verification: TunnelSynthesizeVerification,
    ) -> WakeSchedules {
        if verification == TunnelSynthesizeVerification::Invalid {
            return WakeSchedules::UNCHANGED;
        }
        let expires =
            crate::engine::InstantMillis(owed.arrived_at.0.saturating_add(TUNNEL_TIMEOUT_MS));
        match self
            .tunnels
            .observe_synthesize(owed.tunnel_id, owed.source_interface, expires)
        {
            TunnelTransition::Established | TunnelTransition::Refreshed => {}
            TunnelTransition::Reappeared { previous_interface } => {
                self.routing_table.repoint_routes(
                    previous_interface,
                    owed.source_interface,
                    owed.arrived_at,
                );
                self.mark_interface_dirty(previous_interface);
                self.mark_interface_dirty(owed.source_interface);
            }
        }
        self.routing_table.invalidate_route_expiries();
        WakeSchedules {
            expired_routes: WakeSchedule::AtMost(expires),
            ..WakeSchedules::UNCHANGED
        }
    }
}

fn write_tunnel_synthesize_from_signature(
    signed_region: &[u8; SIGNED_REGION_LEN],
    signature: &Ed25519Signature,
    buf: &mut [u8],
) -> Result<usize, WriteTunnelSynthesizeError> {
    let payload = assemble_synthesize_payload(signed_region, signature);
    write_synthesize_wire_packet(&payload, buf)
        .map_err(|WireError::BufferTooShort| WriteTunnelSynthesizeError::BufferTooShort)
}

#[cfg(test)]
mod tests {
    use super::{TunnelSynthesizeSignCompleted, WriteTunnelSynthesizeError};
    use crate::crypto::sha256;
    use crate::engine::test_support::{
        fixed_secret_key, pin_transport_id, TestStorageLayout, TEST_TRANSPORT_ID,
    };
    use crate::engine::{Directive, EngineReaction, EngineState};
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
    fn continued_tunnel_signing_is_byte_identical_to_the_inline_writer() {
        let mut inline = EngineState::<TestStorageLayout>::default();
        let inline_held = inline.hold_identity(fixed_secret_key()).unwrap();
        inline.set_transport_identity(&inline_held).unwrap();
        let mut continued = EngineState::<TestStorageLayout>::default();
        let continued_held = continued.hold_identity(fixed_secret_key()).unwrap();
        continued.set_transport_identity(&continued_held).unwrap();
        let interface = InterfaceId::new([0xD2; 8]);
        let random = [0x39; RANDOM_HASH_LEN];
        let mut inline_wire = [0u8; 256];
        let inline_bytes = inline
            .write_tunnel_synthesize(interface, &random, &mut inline_wire)
            .unwrap();
        let completed = continued
            .prepare_tunnel_synthesize_sign(interface, random)
            .unwrap();
        let signature =
            crate::crypto::ed25519_sign(&completed.signing_secret, &completed.signed_region);
        let mut continued_wire = std::vec::Vec::new();
        continued
            .resume_tunnel_synthesize_sign(
                TunnelSynthesizeSignCompleted {
                    owed: completed,
                    signature,
                },
                &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                        continued_wire.extend_from_slice(bytes);
                    }
                },
            )
            .unwrap();

        assert_eq!(continued_wire, inline_wire[..inline_bytes]);
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

    fn ingest_tunnel(
        state: &mut EngineState<TestStorageLayout>,
        frame: &mut [u8],
        source: InterfaceId,
        now: crate::engine::InstantMillis,
        interfaces: crate::interfaces::AttachedInterfaces<'_>,
    ) {
        let crate::engine::IngestPacketOutcome::OwesTunnelSynthesizeVerify(owed) = state
            .ingest_for_test(
                crate::interfaces::InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes: frame,
                },
                interfaces,
            )
        else {
            panic!("tunnel synthesis should request signature verification");
        };
        let verification = if crate::crypto::ed25519_verify(
            &owed.signing_key,
            &owed.signed_region,
            &owed.signature,
        )
        .is_ok()
        {
            crate::engine::TunnelSynthesizeVerification::Valid
        } else {
            crate::engine::TunnelSynthesizeVerification::Invalid
        };
        state.resume_tunnel_synthesize_verify(owed, verification);
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
        let _ = before_reboot.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: first_conn,
                bytes: &mut announce,
            },
            AttachedInterfaces::new(&first_view),
        );
        let mut synth = synthesize_wire(0xAB);
        ingest_tunnel(
            &mut before_reboot,
            &mut synth,
            first_conn,
            InstantMillis(2_000),
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
        ingest_tunnel(
            &mut rebooted,
            &mut synth_again,
            second_conn,
            InstantMillis(3_000),
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
