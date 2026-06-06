//! Shared builders for the engine's test modules: the capacity spell, the
//! fixture identities, the `personal.node` announcer recipes, and the wire
//! fixtures more than one domain pins against.

use super::*;
use crate::engine::self_announce::AnnounceConfig;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::IdentitySigner;
use crate::interfaces::InboundPacket;
use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceMode,
    MediumKind, TransportCapability,
};
use crate::routing::storage::FixedInline;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, MTU,
};

pub(crate) type Cap = FixedInline<64, 64, 4096, 4, 512, 64, 8, 8, 8, 128, 8>;

pub(crate) const TEST_ENTROPY: JitterSeed = JitterSeed(0xCAFE_F00D_DEAD_BEEF);
pub(crate) const TEST_NONCE: SelfAnnounceEntropy =
    SelfAnnounceEntropy::new([0xAB; SelfAnnounceEntropy::LEN]);
pub(crate) const TEST_RATCHET_ENTROPY: RatchetEntropy =
    RatchetEntropy::new([0x55; RatchetEntropy::LEN]);

pub(crate) const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

pub(crate) fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

pub(crate) fn fixed_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
    bytes[..32].fill(0x22);
    bytes[32..].fill(0x11);
    Zeroizing::new(bytes)
}

pub(crate) fn second_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
    bytes[..32].fill(0x55);
    bytes[32..].fill(0x66);
    Zeroizing::new(bytes)
}

pub(crate) fn personal_node_announcer() -> EngineState<Cap> {
    personal_node_announcer_with(RatchetPolicy::NoRatchets)
}

pub(crate) fn personal_node_announcer_with(ratchet_policy: RatchetPolicy) -> EngineState<Cap> {
    let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
    let node = state.transport_identity().unwrap();
    let destination = state
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            ProofStrategy::ProveNone,
            ratchet_policy,
        )
        .unwrap();
    state
        .schedule_announce(
            &destination,
            AnnounceConfig {
                app_data: b"hello-personal",
                schedule: ReannounceSchedule::default(),
            },
        )
        .unwrap();
    state
}

pub(crate) fn ratcheted_personal_node_announcer() -> EngineState<Cap> {
    let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
    let mut buf = [0u8; MTU];
    state
        .write_due_self_announce(
            InstantMillis(1_000),
            TEST_NONCE,
            TEST_RATCHET_ENTROPY,
            &mut buf,
        )
        .expect("writing the seeding announce succeeds")
        .expect("the seeding announce is due");
    state
}

pub(crate) fn plain_data_packet(bytes: &mut [u8]) -> InboundPacket<'_> {
    InboundPacket {
        arrived_at: InstantMillis(1_000),
        source_interface: InterfaceId::new([0x07; 16]),
        bytes,
    }
}

pub(crate) fn sealed_single_packet(
    identity: &InMemoryNodeIdentity,
    destination: DestinationHash,
    plaintext: &[u8],
) -> std::vec::Vec<u8> {
    sealed_single_packet_routed(identity, None, destination, plaintext)
}

pub(crate) fn sealed_single_packet_routed(
    identity: &InMemoryNodeIdentity,
    maybe_transport_id: Option<TransportId>,
    destination: DestinationHash,
    plaintext: &[u8],
) -> std::vec::Vec<u8> {
    use crate::crypto::X25519SecretKey;
    use crate::identity::RemoteIdentity;

    let remote = RemoteIdentity::from_public_keys(
        identity.encryption_public_key(),
        identity.signing_public_key(),
    );
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: maybe_transport_id,
        destination,
        context: WireContext::None,
    };
    let mut buf = [0u8; MTU];
    let header_len = header.write(&mut buf).unwrap();
    let sealed = remote
        .encrypt(
            &X25519SecretKey::new([0x33; 32]),
            &[0x44; 16],
            plaintext,
            &mut buf[header_len..],
        )
        .unwrap();
    buf[..header_len + sealed].to_vec()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TickSnapshot {
    pub(crate) egress_directive_count: usize,
    pub(crate) recovered_from_held_count: usize,
}

pub(crate) fn tick_capture<S: EngineStorage>(
    state: &mut EngineState<S>,
    now: InstantMillis,
) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>) {
    let tick_out = state.tick(now, TEST_ENTROPY);
    let snapshot = TickSnapshot {
        egress_directive_count: tick_out.egress_directive_count(),
        recovered_from_held_count: tick_out.recovered_from_held_count(),
    };
    let mut emitted = std::vec::Vec::new();
    let mut buf = [0u8; MTU];
    for directive in tick_out.egress_directives() {
        let n = directive.to_wire(&mut buf).expect("serialize directive");
        emitted.push(buf[..n].to_vec());
    }
    (snapshot, emitted)
}

pub(crate) fn observable_state<S: EngineStorage>(
    state: &EngineState<S>,
) -> (u64, u64, usize, usize, usize, std::vec::Vec<InterfaceId>) {
    (
        state.tick_count(),
        state.ingested_packet_count(),
        state.route_count(),
        state.held_announce_count(),
        state.pending_announce_rebroadcast_count(),
        state.registered_interfaces().to_vec(),
    )
}

pub(crate) fn routable_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        medium: MediumKind::Loopback,
        state: ConnectionState::Connected,
    }
}

pub(crate) fn register_test_interface(state: &mut EngineState<Cap>, id: InterfaceId) {
    state
        .register_interface_descriptor(&routable_descriptor(id))
        .unwrap();
}
