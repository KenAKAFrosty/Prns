pub mod registry;

pub use registry::{FixedTunnelTable, TunnelTable, TunnelTransition, Tunnels, TUNNEL_TIMEOUT_MS};
#[cfg(feature = "alloc")]
pub use registry::{HeapTunnelTable, DEFAULT_MAX_TUNNELS};

use crate::crypto::{ed25519_verify, sha256, Ed25519PublicKey, Ed25519Signature};
use crate::engine::EgressSerializeError;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, HEADER_MIN_LEN,
};

pub const TUNNEL_SYNTHESIZE_DESTINATION: DestinationHash = DestinationHash::new([
    0x91, 0xbf, 0x09, 0x10, 0x26, 0x7b, 0x59, 0xb0, 0xe8, 0x64, 0xe0, 0xd4, 0xc9, 0x16, 0x02, 0xca,
]);

pub const PUBLIC_KEY_LEN: usize = crate::identity::IDENTITY_PUBLIC_KEY_LEN;
const ED25519_PUBLIC_OFFSET: usize = 32;
pub const INTERFACE_HASH_LEN: usize = 32;
pub const RANDOM_HASH_LEN: usize = 16;
pub const SIGNATURE_BYTE_LEN: usize = 64;

pub const SIGNED_REGION_LEN: usize = PUBLIC_KEY_LEN + INTERFACE_HASH_LEN + RANDOM_HASH_LEN;
pub const SYNTHESIZE_PAYLOAD_LEN: usize = SIGNED_REGION_LEN + SIGNATURE_BYTE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TunnelId([u8; 32]);

impl TunnelId {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[must_use]
pub fn compute_tunnel_id(
    public_key: &[u8; PUBLIC_KEY_LEN],
    interface_hash: &[u8; INTERFACE_HASH_LEN],
) -> TunnelId {
    let mut preimage = [0u8; PUBLIC_KEY_LEN + INTERFACE_HASH_LEN];
    preimage[..PUBLIC_KEY_LEN].copy_from_slice(public_key);
    preimage[PUBLIC_KEY_LEN..].copy_from_slice(interface_hash);
    TunnelId(sha256(&preimage))
}

#[must_use]
pub fn synthesize_signed_region(
    public_key: &[u8; PUBLIC_KEY_LEN],
    interface_hash: &[u8; INTERFACE_HASH_LEN],
    random_hash: &[u8; RANDOM_HASH_LEN],
) -> [u8; SIGNED_REGION_LEN] {
    let mut region = [0u8; SIGNED_REGION_LEN];
    region[..PUBLIC_KEY_LEN].copy_from_slice(public_key);
    region[PUBLIC_KEY_LEN..PUBLIC_KEY_LEN + INTERFACE_HASH_LEN].copy_from_slice(interface_hash);
    region[PUBLIC_KEY_LEN + INTERFACE_HASH_LEN..].copy_from_slice(random_hash);
    region
}

#[must_use]
pub fn assemble_synthesize_payload(
    signed_region: &[u8; SIGNED_REGION_LEN],
    signature: &Ed25519Signature,
) -> [u8; SYNTHESIZE_PAYLOAD_LEN] {
    let mut payload = [0u8; SYNTHESIZE_PAYLOAD_LEN];
    payload[..SIGNED_REGION_LEN].copy_from_slice(signed_region);
    payload[SIGNED_REGION_LEN..].copy_from_slice(&signature.0);
    payload
}

pub fn write_synthesize_wire_packet(
    payload: &[u8; SYNTHESIZE_PAYLOAD_LEN],
    buf: &mut [u8],
) -> Result<usize, EgressSerializeError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: TUNNEL_SYNTHESIZE_DESTINATION.to_address(),
        context: WireContext::None,
    };
    let total_len = HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN;
    if buf.len() < total_len {
        return Err(EgressSerializeError::BufferTooShort);
    }
    header
        .write(&mut buf[..HEADER_MIN_LEN])
        .map_err(|_| EgressSerializeError::BufferTooShort)?;
    buf[HEADER_MIN_LEN..total_len].copy_from_slice(payload);
    Ok(total_len)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSynthesize {
    pub tunnel_id: TunnelId,
    pub interface_hash: [u8; INTERFACE_HASH_LEN],
}

#[must_use]
pub fn parse_synthesize_payload(payload: &[u8]) -> Option<VerifiedSynthesize> {
    if payload.len() != SYNTHESIZE_PAYLOAD_LEN {
        return None;
    }
    let public_key: [u8; PUBLIC_KEY_LEN] = payload[..PUBLIC_KEY_LEN].try_into().ok()?;
    let interface_hash: [u8; INTERFACE_HASH_LEN] = payload
        [PUBLIC_KEY_LEN..PUBLIC_KEY_LEN + INTERFACE_HASH_LEN]
        .try_into()
        .ok()?;
    let signed_region = &payload[..SIGNED_REGION_LEN];
    let signature_bytes: [u8; SIGNATURE_BYTE_LEN] = payload[SIGNED_REGION_LEN..].try_into().ok()?;
    let signature = Ed25519Signature(signature_bytes);

    let signing_key: [u8; 32] = public_key[ED25519_PUBLIC_OFFSET..].try_into().ok()?;
    ed25519_verify(&Ed25519PublicKey(signing_key), signed_region, &signature).ok()?;

    Some(VerifiedSynthesize {
        tunnel_id: compute_tunnel_id(&public_key, &interface_hash),
        interface_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};
    use crate::interfaces::AttachedInterfaces;
    use crate::routing::announce::{derive_plain_destination_hash, expand_name};
    use crate::routing::warmth::RouteWarmth;

    fn signed_payload(
        seed: u8,
        interface_hash: &[u8; INTERFACE_HASH_LEN],
        random_hash: &[u8; RANDOM_HASH_LEN],
    ) -> ([u8; SYNTHESIZE_PAYLOAD_LEN], [u8; PUBLIC_KEY_LEN]) {
        let secret = Ed25519SecretKey::new([seed; 32]);
        let signing_public = ed25519_public_key(&secret);

        let mut public_key = [0u8; PUBLIC_KEY_LEN];
        public_key[..32].copy_from_slice(&[seed ^ 0xA5; 32]);
        public_key[32..].copy_from_slice(&signing_public.0);

        let region = synthesize_signed_region(&public_key, interface_hash, random_hash);
        let signature = ed25519_sign(&secret, &region);
        (assemble_synthesize_payload(&region, &signature), public_key)
    }

    #[test]
    fn the_destination_matches_the_plain_name_derivation() {
        let name = expand_name("rnstransport", &["tunnel", "synthesize"]).unwrap();
        assert_eq!(
            derive_plain_destination_hash(&name),
            TUNNEL_SYNTHESIZE_DESTINATION
        );
    }

    #[test]
    fn the_payload_layout_sums_to_the_rns_sizes() {
        assert_eq!(SIGNED_REGION_LEN, 112);
        assert_eq!(SYNTHESIZE_PAYLOAD_LEN, 176);
    }

    #[test]
    fn a_validly_signed_payload_round_trips_to_its_tunnel_id() {
        let interface_hash = [0x33u8; INTERFACE_HASH_LEN];
        let random_hash = [0x77u8; RANDOM_HASH_LEN];
        let (payload, public_key) = signed_payload(0x11, &interface_hash, &random_hash);

        let verified = parse_synthesize_payload(&payload).expect("a signed payload verifies");
        assert_eq!(verified.interface_hash, interface_hash);
        assert_eq!(
            verified.tunnel_id,
            compute_tunnel_id(&public_key, &interface_hash)
        );
    }

    #[test]
    fn the_tunnel_id_ignores_the_random_hash_so_it_is_stable_across_reconnects() {
        let interface_hash = [0x44u8; INTERFACE_HASH_LEN];
        let (first, _) = signed_payload(0x22, &interface_hash, &[0x01; RANDOM_HASH_LEN]);
        let (second, _) = signed_payload(0x22, &interface_hash, &[0xFE; RANDOM_HASH_LEN]);

        let a = parse_synthesize_payload(&first).expect("first verifies");
        let b = parse_synthesize_payload(&second).expect("second verifies");
        assert_ne!(first, second);
        assert_eq!(a.tunnel_id, b.tunnel_id);
    }

    #[test]
    fn a_tampered_signature_does_not_verify() {
        let interface_hash = [0x55u8; INTERFACE_HASH_LEN];
        let (mut payload, _) = signed_payload(0x33, &interface_hash, &[0x09; RANDOM_HASH_LEN]);
        payload[SIGNED_REGION_LEN] ^= 0x01;
        assert!(parse_synthesize_payload(&payload).is_none());
    }

    #[test]
    fn a_tampered_signed_region_does_not_verify() {
        let interface_hash = [0x66u8; INTERFACE_HASH_LEN];
        let (mut payload, _) = signed_payload(0x44, &interface_hash, &[0x0A; RANDOM_HASH_LEN]);
        payload[PUBLIC_KEY_LEN] ^= 0x01;
        assert!(parse_synthesize_payload(&payload).is_none());
    }

    #[test]
    fn a_wrong_length_payload_is_rejected() {
        assert!(parse_synthesize_payload(&[0u8; SYNTHESIZE_PAYLOAD_LEN - 1]).is_none());
        assert!(parse_synthesize_payload(&[0u8; SYNTHESIZE_PAYLOAD_LEN + 1]).is_none());
        assert!(parse_synthesize_payload(&[]).is_none());
    }

    #[test]
    fn the_wire_packet_carries_the_payload_after_a_plain_data_header() {
        let payload = [0xC4u8; SYNTHESIZE_PAYLOAD_LEN];
        let mut buf = [0u8; HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN];
        let n = write_synthesize_wire_packet(&payload, &mut buf).expect("frames into a sized buf");
        assert_eq!(n, HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN);

        let (header, body) = WirePacketHeader::parse(&buf[..n]).expect("the header parses back");
        assert_eq!(
            DestinationHash::from_address(header.address),
            TUNNEL_SYNTHESIZE_DESTINATION
        );
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert!(header.transport_id.is_none());
        assert_eq!(body, &payload[..]);
    }

    #[test]
    fn the_wire_packet_rejects_a_short_buffer() {
        let payload = [0u8; SYNTHESIZE_PAYLOAD_LEN];
        let mut buf = [0u8; HEADER_MIN_LEN + SYNTHESIZE_PAYLOAD_LEN - 1];
        assert_eq!(
            write_synthesize_wire_packet(&payload, &mut buf),
            Err(EgressSerializeError::BufferTooShort)
        );
    }

    fn synthesize_wire(seed: u8) -> std::vec::Vec<u8> {
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
    fn a_tunnel_keeps_routes_warm_through_a_disconnect_and_repoints_them_on_reconnect() {
        use crate::engine::test_support::{
            bytes_from_hex, routable_descriptor, transporting_node, RNS_1_3_5_ANNOUNCE,
        };
        use crate::engine::InstantMillis;
        use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};

        let mut relay = transporting_node();
        let dest = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        let first_conn = InterfaceId::new([0xC1; 8]);
        let interfaces = [routable_descriptor(first_conn)];

        let mut announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: first_conn,
                bytes: &mut announce,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(
            relay
                .routing_table
                .path_row(&dest)
                .expect("the announce taught a route")
                .receiving_interface,
            first_conn,
        );

        let mut synth = synthesize_wire(0xAB);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: first_conn,
                bytes: &mut synth,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert!(relay.tunnels.warm_until(first_conn).is_some());

        let no_interfaces: [InterfaceDescriptor; 0] = [];
        let _ = relay.cull_expired_routes(
            InstantMillis(3_000),
            AttachedInterfaces::new(&no_interfaces),
            &mut |_| {},
        );
        assert!(
            relay.routing_table.path_row(&dest).is_some(),
            "the route stays warm while the tunnel is dormant",
        );

        let second_conn = InterfaceId::new([0xC2; 8]);
        let second_view = [routable_descriptor(second_conn)];
        let mut synth_again = synthesize_wire(0xAB);
        let _ = relay.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(4_000),
                source_interface: second_conn,
                bytes: &mut synth_again,
            },
            &mut |_| {},
            AttachedInterfaces::new(&second_view),
            &mut |_| {},
            None,
        );
        assert_eq!(
            relay
                .routing_table
                .path_row(&dest)
                .expect("the route survived the gap")
                .receiving_interface,
            second_conn,
            "the warm route re-points onto the reconnected interface",
        );

        let past = InstantMillis(4_000 + TUNNEL_TIMEOUT_MS + 1);
        let _ =
            relay.cull_expired_routes(past, AttachedInterfaces::new(&no_interfaces), &mut |_| {});
        assert!(
            relay.routing_table.path_row(&dest).is_none(),
            "once the tunnel times out the route finally falls due",
        );
        assert!(relay.tunnels.is_empty());
    }
}
