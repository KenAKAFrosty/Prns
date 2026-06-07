//! Shared builders for the engine's test modules: the capacity spell, the
//! fixture identities, the `personal.node` announcer recipes, and the wire
//! fixtures more than one domain pins against.

use super::*;
use crate::engine::self_announce::AnnounceConfig;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::IdentitySigner;
use crate::interfaces::InboundPacket;
use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceMode, MediumKind, TransportCapability,
};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::FixedInline;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, MTU,
};

pub(crate) type Cap = FixedInline<64, 64, 4096, 4, 512, 64, 8, 8, 8, 128, 8, 8>;

pub(crate) const TEST_ENTROPY: JitterSeed = JitterSeed(0xCAFE_F00D_DEAD_BEEF);
pub(crate) const TEST_NONCE: SelfAnnounceEntropy =
    SelfAnnounceEntropy::new([0xAB; SelfAnnounceEntropy::LEN]);
pub(crate) const TEST_RATCHET_ENTROPY: RatchetEntropy =
    RatchetEntropy::new([0x55; RatchetEntropy::LEN]);
pub(crate) const TEST_TRANSPORT_ID: TransportId = TransportId::new([0x7A; 16]);

pub(crate) fn transporting_node() -> EngineState<Cap> {
    let mut state: EngineState<Cap> = EngineState::<Cap>::default();
    state.set_transport_id(TEST_TRANSPORT_ID);
    state
}

pub(crate) const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

pub(crate) fn raw_announce_accepted(hops: u8) -> IngestPacketOutcome<'static> {
    IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
        destination: DestinationHash::new(
            hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
        ),
        hops,
        rebroadcast: RebroadcastDecision::Scheduled,
    }))
}

pub(crate) const RAW_SEALED_FOR_PROOF: &str =
    "0000c3cfae69b36bb6e3bbfd96a3b5867a59007b0d47d93427f8311160781c7c733fd89f88970aef490d8a\
     a0ee19a4cb8a1b1444444444444444444444444444444444084624da14eb2a916d8a20cad6da4623aff598\
     25ec6b58715afe16269730584f5fe3a55a6429ded73c3d4b2458f67ef9";

pub(crate) const RNS_1_3_1_IMPLICIT_PROOF: &str =
    "0300a34e24b00ebdda0179b642579b71266c00f52e874f44101203b553179c107604fc01ef99e210895f95\
     423f14aca8094a5a09938d9337aec5c6cb1bc38458d65da559450a9f8e0e78921ca690bed8430100";

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
    let node = state.held_identity_hashes()[0];
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
        .written_len();
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
    interfaces: &[InterfaceDescriptor],
) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>) {
    let tick_out = state.tick(now, TEST_ENTROPY, interfaces);
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
) -> (u64, u64, usize, usize, usize) {
    (
        state.tick_count(),
        state.ingested_packet_count(),
        state.route_count(),
        state.held_announce_count(),
        state.pending_announce_rebroadcast_count(),
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

pub(crate) fn repeating_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        ..routable_descriptor(id)
    }
}

pub(crate) fn transporting_view() -> [InterfaceDescriptor; 1] {
    [routable_descriptor(InterfaceId::new([0xEE; 16]))]
}

pub(crate) const RATCHETED_SELF_ANNOUNCE_RNS_WIRE: &str = "2100c3cfae69b36bb6e3bbfd96a3b5867a5900\
         0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
         ab49baa826f122c1437f44444444444444444444\
         38ab664bd86f77d7e66bdd9ae0792913a94fd8b33a1260027e4b46c1f4884c67\
         91d8c21a401611ca859e9ae293e86a6860fb2babd90fe4c58cf315d7a111cc0a\
         3e9646aa7ffdf1530150aa30d0c684aab5b6236ea71a4b8f8c72b2b02768bf02\
         68656c6c6f2d706572736f6e616c";

pub(crate) const RAW_SEALED_TO_RATCHET: &str =
    "0000c3cfae69b36bb6e3bbfd96a3b5867a59007b0d47d93427f8311160781c7c733fd89f88970aef490d8a\
         a0ee19a4cb8a1b1444444444444444444444444444444444f0c0d10df07782f3a9a89a271b84960bc9d252\
         5bfcfd385954b4ebda6c6702dd9b82ca630f3b45c1c57457ad70aa14e6";
