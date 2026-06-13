//! Shared builders for the engine's test modules: the capacity spell, the
//! fixture identities, the `personal.node` announcer recipes, and the wire
//! fixtures more than one domain pins against.

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::IdentitySigner;
use crate::interfaces::InboundPacket;
use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceMode, TransportCapability,
};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::AnnounceEntropy;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::storage::TestFixedStorage;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, BROADCAST_MTU,
};

pub(crate) type Cap = TestFixedStorage<64, 64, 4096, 4, 512, 8, 8, 128, 8, 8, 8, 8, 16, 16>;

pub(crate) const TEST_ENTROPY: JitterSeed = JitterSeed(0xCAFE_F00D_DEAD_BEEF);
pub(crate) const TEST_ANNOUNCE_ENTROPY: AnnounceEntropy =
    AnnounceEntropy::new([0xAB; AnnounceEntropy::LEN]);
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

/// The reference's own retransmission of [`RATCHETED_ANNOUNCE_RNS_WIRE`]:
/// minted by RNS 1.3.1 `Transport.jobs()` packet construction (`HEADER_2`,
/// `TRANSPORT`, transport_id `0x7A…` = [`TEST_TRANSPORT_ID`], hops 1) and
/// self-checked through `Identity.validate_announce` before pinning.
pub(crate) const RNS_1_3_1_RETRANSMITTED_ANNOUNCE: &str =
    "71017a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7ac3cfae69b36bb6e3bbfd96a3b5867a59000faa684ed28867b97f\
     4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a\
     016baf8520a332c9778737ab49baa826f122c1437f4444444444444444444438ab664bd86f77d7e66bdd9ae0\
     792913a94fd8b33a1260027e4b46c1f4884c6791d8c21a401611ca859e9ae293e86a6860fb2babd90fe4c58c\
     f315d7a111cc0a3e9646aa7ffdf1530150aa30d0c684aab5b6236ea71a4b8f8c72b2b02768bf0268656c6c6f\
     2d706572736f6e616c";

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

pub(crate) fn personal_node_destination() -> DestinationHash {
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let name = crate::routing::announce::expand_name("personal", &["node"]).expect("valid name");
    crate::routing::announce::derive_destination_hash(&identity.identity_hash(), &name)
}

pub(crate) fn personal_node_announcer() -> EngineState<Cap> {
    personal_node_announcer_with(RatchetPolicy::NoRatchets)
}

pub(crate) fn personal_node_announcer_with(ratchet_policy: RatchetPolicy) -> EngineState<Cap> {
    let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
    let node = state.held_identity_hashes()[0];
    state
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            b"hello-personal",
            ProofStrategy::ProveNone,
            ratchet_policy,
        )
        .unwrap();
    state
}

pub(crate) fn ratcheted_personal_node_announcer() -> EngineState<Cap> {
    let mut state = personal_node_announcer_with(RatchetPolicy::Ratcheted);
    let mut buf = [0u8; BROADCAST_MTU];
    let _ = state.write_commanded_announce(
        &AnnounceNow {
            destination: personal_node_destination(),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        },
        InstantMillis(1_000),
        TEST_ANNOUNCE_ENTROPY,
        TEST_RATCHET_ENTROPY,
        &mut buf,
    );
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
    let mut buf = [0u8; BROADCAST_MTU];
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
}

pub(crate) fn tick_capture<S: StorageLayout>(
    state: &mut EngineState<S>,
    now: InstantMillis,
    interfaces: &[InterfaceConfig],
) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>) {
    let mut emitted = std::vec::Vec::new();
    let _ = state.fire_due_scheduled_announces(now, interfaces, &mut |reaction| {
        if let EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) = reaction {
            emitted.push(bytes.to_vec());
        }
    });

    let snapshot = TickSnapshot {
        egress_directive_count: emitted.len(),
    };
    (snapshot, emitted)
}

pub(crate) fn observable_state<S: StorageLayout>(state: &EngineState<S>) -> (u64, usize, usize) {
    (
        state.ingested_packet_count(),
        state.route_count(),
        state.scheduled_announce_count(),
    )
}

pub(crate) fn routable_descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: None,
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
    }
}

pub(crate) fn repeating_descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        ..routable_descriptor(id)
    }
}

pub(crate) fn transporting_view() -> [InterfaceConfig; 1] {
    [routable_descriptor(InterfaceId::new([0xEE; 16]))]
}

pub(crate) const RATCHETED_ANNOUNCE_RNS_WIRE: &str = "2100c3cfae69b36bb6e3bbfd96a3b5867a5900\
         0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
         ab49baa826f122c1437f44444444444444444444\
         38ab664bd86f77d7e66bdd9ae0792913a94fd8b33a1260027e4b46c1f4884c67\
         91d8c21a401611ca859e9ae293e86a6860fb2babd90fe4c58cf315d7a111cc0a\
         3e9646aa7ffdf1530150aa30d0c684aab5b6236ea71a4b8f8c72b2b02768bf02\
         68656c6c6f2d706572736f6e616c";

/// [`RAW_SEALED_TO_RATCHET`] as RNS 1.3.1 `Transport.outbound` injects it into
/// transport for a multi-hop destination: `HEADER_2`/`TRANSPORT` flags spliced
/// in, addressed at the relay `0x7A…` ([`TEST_TRANSPORT_ID`]), hops untouched.
/// Self-checked: the reference parses it and `get_hash()` equals the direct
/// form's — the packet hash (and so the receipt) is transport-invariant.
pub(crate) const RAW_SEALED_TO_RATCHET_VIA_TRANSPORT: &str =
    "50007a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7ac3cfae69b36bb6e3bbfd96a3b5867a59007b0d47d93427f831116078\
     1c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1444444444444444444444444444444444f0c0d10df07782f3a9\
     a89a271b84960bc9d2525bfcfd385954b4ebda6c6702dd9b82ca630f3b45c1c57457ad70aa14e6";

pub(crate) const RAW_SEALED_TO_RATCHET: &str =
    "0000c3cfae69b36bb6e3bbfd96a3b5867a59007b0d47d93427f8311160781c7c733fd89f88970aef490d8a\
         a0ee19a4cb8a1b1444444444444444444444444444444444f0c0d10df07782f3a9a89a271b84960bc9d252\
         5bfcfd385954b4ebda6c6702dd9b82ca630f3b45c1c57457ad70aa14e6";

/// Run an `EmitFrame` fill against heap scratch and hand back the frame it wrote — the
/// test-side stand-in for a driver's granted slot.
pub(crate) fn filled_frame(
    fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
) -> Option<std::vec::Vec<u8>> {
    let mut scratch = std::vec![0u8; crate::routing::links::MAX_LINK_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE];
    let len = fill(&mut scratch)?;
    scratch.truncate(len);
    Some(scratch)
}
