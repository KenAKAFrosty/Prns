pub mod commands;
pub mod directives;
pub mod egress;
pub mod identity_registration;
pub mod ingress;
pub mod proof;
pub mod self_announce;
pub mod self_ratchets;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tick;

pub use commands::{
    AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget, CommandId,
    CommandOutcome, EngineCommand, IssuedCommand, Settleable, Settlement,
};
pub use egress::{EgressDirective, EgressSerializeError};
pub use identity_registration::SetTransportIdentityError;
pub use ingress::{AnnounceIngest, IngestPacketOutcome};
pub use ingress::{DataPacket, Ingress};
pub use proof::{ProofOwed, WriteProofError};
pub use self_announce::{ReannounceSchedule, SelfAnnounceAppData, WriteSelfAnnounceError};
pub use self_ratchets::{RatchetEntropy, RatchetPolicy};
pub use tick::TickOutput;

use crate::engine::self_announce::SelfAnnounces;
use crate::engine::self_ratchets::SelfRatchets;
use crate::identity::held::HeldIdentities;
use crate::identity::{IdentityHash, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{InterfaceDescriptor, InterfaceId, MAX_REGISTERED_INTERFACES};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::announce::SelfAnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::UpstreamAppDestinations;
use crate::routing::RoutingTable;
use heapless::Vec as HeaplessVec;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

const JITTER_SEED_LEN: usize = core::mem::size_of::<u64>();

pub const ENGINE_CYCLE_ENTROPY_LEN: usize =
    JITTER_SEED_LEN + SelfAnnounceEntropy::LEN + RatchetEntropy::LEN;

pub struct EngineCycleEntropySeed([u8; ENGINE_CYCLE_ENTROPY_LEN]);

impl EngineCycleEntropySeed {
    pub const fn new(bytes: [u8; ENGINE_CYCLE_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ENGINE_CYCLE_ENTROPY_LEN] {
        &self.0
    }
}

pub struct EngineCycleEntropy {
    pub jitter: JitterSeed,
    pub self_announce: SelfAnnounceEntropy,
    pub ratchet: RatchetEntropy,
}

impl EngineCycleEntropy {
    pub fn from_seed(seed: EngineCycleEntropySeed) -> Self {
        let bytes = seed.as_bytes();
        let mut jitter = [0u8; JITTER_SEED_LEN];
        jitter.copy_from_slice(&bytes[..JITTER_SEED_LEN]);
        let mut nonce = [0u8; SelfAnnounceEntropy::LEN];
        nonce.copy_from_slice(&bytes[JITTER_SEED_LEN..JITTER_SEED_LEN + SelfAnnounceEntropy::LEN]);
        let mut ratchet = [0u8; RatchetEntropy::LEN];
        ratchet.copy_from_slice(&bytes[JITTER_SEED_LEN + SelfAnnounceEntropy::LEN..]);
        Self {
            jitter: JitterSeed(u64::from_le_bytes(jitter)),
            self_announce: SelfAnnounceEntropy::new(nonce),
            ratchet: RatchetEntropy::new(ratchet),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextScheduledEngineWork {
    Immediate,
    At(InstantMillis),
    Idle,
}

pub struct EngineState<S: EngineStorage> {
    tick_count: u64,
    ingested_packet_count: u64,
    ingested_command_count: u64,
    routing_table: RoutingTable<S::Routes, S::Announces, S::History, S::AppData>,
    held_announces_cache: S::Held,
    pending_rebroadcasts: S::Pending,
    directives: S::Directives,
    interfaces: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
    upstream_app_destinations: UpstreamAppDestinations<S::UpstreamAppDestinations>,
    packet_hash_history: S::PacketHashes,
    held_identities: HeldIdentities<S::HeldIdentities>,
    transport_identity: Option<IdentityHash>,
    self_announces: SelfAnnounces<S::SelfAnnounces>,
    self_ratchets: SelfRatchets<S::SelfRatchets>,
}

impl<S: EngineStorage> Default for EngineState<S> {
    fn default() -> Self {
        Self {
            tick_count: 0,
            ingested_packet_count: 0,
            ingested_command_count: 0,
            routing_table: Default::default(),
            held_announces_cache: Default::default(),
            pending_rebroadcasts: Default::default(),
            directives: Default::default(),
            interfaces: HeaplessVec::new(),
            upstream_app_destinations: UpstreamAppDestinations::default(),
            packet_hash_history: Default::default(),
            held_identities: HeldIdentities::default(),
            transport_identity: None,
            self_announces: SelfAnnounces::default(),
            self_ratchets: SelfRatchets::default(),
        }
    }
}

impl<S: EngineStorage> core::fmt::Debug for EngineState<S>
where
    S::SelfAnnounces: core::fmt::Debug,
    S::Routes: core::fmt::Debug,
    S::Announces: core::fmt::Debug,
    S::History: core::fmt::Debug,
    S::AppData: core::fmt::Debug,
    S::Held: core::fmt::Debug,
    S::Pending: core::fmt::Debug,
    S::UpstreamAppDestinations: core::fmt::Debug,
    S::PacketHashes: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("tick_count", &self.tick_count)
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("ingested_command_count", &self.ingested_command_count)
            .field("routing_table", &self.routing_table)
            .field("held_announces_cache", &self.held_announces_cache)
            .field("pending_rebroadcasts", &self.pending_rebroadcasts)
            .field("interfaces", &self.interfaces)
            .field("upstream_app_destinations", &self.upstream_app_destinations)
            .field("packet_hash_history", &self.packet_hash_history)
            .field("held_identities", &self.held_identities)
            .field("transport_identity", &self.transport_identity)
            .field("self_announces", &self.self_announces)
            .field("self_ratchets", &self.self_ratchets)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterInterfaceError {
    RegistryFull,
}

impl<S: EngineStorage> EngineState<S> {
    /// NOTE: this may need to be re-worked later, but as of this comment's writing
    /// is okay to leave in. Specifically, `new` quietly makes this node's first
    /// identity its transport identity, skipping over the *optionality* of having a
    /// this-node-id at all — a pure repeater may want neither, an app-only node may
    /// want held identities but no transport role. Deliberate for now; re-assess
    /// when Links/transport land and the transport-id story becomes real (relaying
    /// stamps it into transport headers, and routing a Single beyond one hop may
    /// need it too).
    #[allow(clippy::expect_used)]
    pub fn new(identity_secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> Self {
        let mut state = Self::default();
        let identity = state
            .hold_identity(identity_secret_key)
            .expect("an empty store holds the first identity");
        state.transport_identity = Some(identity);
        state
    }

    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
    }

    pub const fn ingested_command_count(&self) -> u64 {
        self.ingested_command_count
    }

    pub fn route_count(&self) -> usize {
        self.routing_table.route_count()
    }

    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routing_table.route_count_via(interface)
    }

    pub fn held_announce_count(&self) -> usize {
        self.held_announces_cache.len()
    }

    pub fn pending_announce_rebroadcast_count(&self) -> usize {
        self.pending_rebroadcasts.pending_count()
    }

    /// Track `descriptor`'s interface for routing. Registration is membership only:
    /// whether a packet actually leaves an interface is re-decided per transmit
    /// against its live connection state and capabilities (in the egress fan), so an
    /// interface that is down now — or comes up later — is handled there, not gated
    /// here. The sole failure is a full registry.
    pub fn register_interface_descriptor(
        &mut self,
        descriptor: &InterfaceDescriptor,
    ) -> Result<(), RegisterInterfaceError> {
        if self.interfaces.contains(&descriptor.id) {
            return Ok(());
        }
        self.interfaces
            .push(descriptor.id)
            .map_err(|_| RegisterInterfaceError::RegistryFull)
    }

    pub fn registered_interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }

    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        if self.held_announce_count() > 0 {
            return NextScheduledEngineWork::Immediate;
        }

        let mut earliest: Option<InstantMillis> = None;

        if self.self_announces.due_announce(now).is_some() {
            return NextScheduledEngineWork::Immediate;
        }
        if let Some(deadline) = self.self_announces.next_due_at() {
            earliest = Some(earliest.map_or(deadline, |e| e.min(deadline)));
        }

        if let Some(due_at) = self.pending_rebroadcasts.earliest_due_at() {
            if due_at <= now {
                return NextScheduledEngineWork::Immediate;
            }
            earliest = Some(earliest.map_or(due_at, |e| e.min(due_at)));
        }

        match earliest {
            Some(instant) => NextScheduledEngineWork::At(instant),
            None => NextScheduledEngineWork::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::interfaces::InboundPacket;
    use crate::interfaces::{ConnectionState, EgressCapability};
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::routing::storage::FixedInline;
    use crate::wire::MTU;

    #[test]
    fn next_wakeup_is_idle_for_a_relay_with_no_scheduled_work() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        assert_eq!(
            state.next_wakeup(InstantMillis(1_000)),
            NextScheduledEngineWork::Idle
        );
    }

    #[test]
    fn next_wakeup_is_immediate_when_a_self_announce_is_due() {
        let state = personal_node_announcer();
        assert_eq!(
            state.next_wakeup(InstantMillis(0)),
            NextScheduledEngineWork::Immediate
        );
    }

    #[test]
    fn next_wakeup_reports_the_reannounce_deadline_once_we_have_announced() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; MTU];
        state
            .write_due_self_announce(
                InstantMillis(1_000),
                TEST_NONCE,
                TEST_RATCHET_ENTROPY,
                &mut buf,
            )
            .expect("first announce is due");

        let interval = ReannounceSchedule::default().interval_millis();
        assert_eq!(
            state.next_wakeup(InstantMillis(2_000)),
            NextScheduledEngineWork::At(InstantMillis(1_000 + interval)),
        );
    }

    #[test]
    fn next_wakeup_accounts_for_a_scheduled_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let _ = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        match state.next_wakeup(InstantMillis(0)) {
            NextScheduledEngineWork::At(t) => assert!(
                t.0 >= 1_000 && t.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                "due_at {} should sit within the jitter window after arrival",
                t.0,
            ),
            other => panic!("expected At(_), got {other:?}"),
        }

        assert_eq!(
            state.next_wakeup(InstantMillis(1_000_000)),
            NextScheduledEngineWork::Immediate,
        );
    }

    #[test]
    fn register_interface_descriptor_tracks_an_interface_in_any_state() {
        for (idx, connection_state) in [
            ConnectionState::Connected,
            ConnectionState::Degraded,
            ConnectionState::Initializing,
            ConnectionState::Reconnecting,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
        ]
        .into_iter()
        .enumerate()
        {
            let id = InterfaceId::new([idx as u8; 16]);
            let descriptor = InterfaceDescriptor {
                state: connection_state,
                ..routable_descriptor(id)
            };
            let mut engine: EngineState<Cap> = EngineState::<Cap>::default();

            assert_eq!(engine.register_interface_descriptor(&descriptor), Ok(()));
            assert_eq!(engine.registered_interfaces(), &[id]);
        }
    }

    #[test]
    fn register_interface_descriptor_tracks_a_receive_only_interface() {
        let mut descriptor = routable_descriptor(InterfaceId::new([0xCD; 16]));
        descriptor.capabilities.egress = EgressCapability::Disabled;
        let mut engine: EngineState<Cap> = EngineState::<Cap>::default();

        assert_eq!(engine.register_interface_descriptor(&descriptor), Ok(()));
        assert_eq!(engine.registered_interfaces(), &[descriptor.id]);
    }

    #[test]
    fn register_interface_descriptor_reports_a_full_registry() {
        let mut engine: EngineState<Cap> = EngineState::<Cap>::default();
        for idx in 0..MAX_REGISTERED_INTERFACES {
            let id = InterfaceId::new([idx as u8; 16]);
            assert_eq!(
                engine.register_interface_descriptor(&routable_descriptor(id)),
                Ok(())
            );
        }
        let overflow = InterfaceId::new([0xFF; 16]);
        assert_eq!(
            engine.register_interface_descriptor(&routable_descriptor(overflow)),
            Err(RegisterInterfaceError::RegistryFull)
        );
        assert_eq!(
            engine.registered_interfaces().len(),
            MAX_REGISTERED_INTERFACES
        );
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state =
            EngineState::<FixedInline<64, 128, 4096, 4, 512, 64, 8, 8, 8, 128, 8>>::default();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
        );
        assert_eq!(out, IngestPacketOutcome::Announce(AnnounceIngest::Accepted));
        assert_eq!(state.route_count(), 1);
    }
}
