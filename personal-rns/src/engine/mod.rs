//! Pure protocol engine boundary.
//!
//! [`ingest_packets`] handles inbound traffic and [`tick`] advances scheduled
//! work without touching clocks, sockets, or storage directly.

pub mod directives;
pub mod egress;
pub mod ingress;
pub mod self_announce;

pub use egress::{EgressDirective, EgressSerializeError};
pub use ingress::Ingress;
pub use self_announce::{ReannounceSchedule, SelfAnnounceConfig, SelfAnnounceConfigError};

use crate::engine::directives::{EngineDirective, EngineDirectives};
use crate::engine::egress::write_announce_wire_packet;
use crate::engine::self_announce::SelfAnnounceSettings;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    ConnectionState, InboundPacket, InterfaceDescriptor, InterfaceId, MAX_REGISTERED_INTERFACES,
};
use crate::routing::announce::{
    derive_destination_hash, Announce, AnnounceAcceptanceDecision, AnnounceAcceptanceInput,
    AnnounceId, SelfAnnounceEntropy,
};
use crate::routing::defaults::{jitter_offset_for, JitterSeed};
use crate::routing::held_cache::HeldAnnounces;
use crate::routing::schedule::RebroadcastQueue;
use crate::routing::storage::Storage;
use crate::routing::{
    DropCause, RoutingTable, UpsertRouteOutcome, DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

const JITTER_SEED_LEN: usize = core::mem::size_of::<u64>();

pub const ENGINE_CYCLE_ENTROPY_LEN: usize = JITTER_SEED_LEN + SelfAnnounceEntropy::LEN;

/// Raw CSPRNG bytes for one engine cycle.
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
    /// Seed used to spread rebroadcast timing.
    pub jitter: JitterSeed,
    /// Nonce material consumed only when a self-announce is due.
    pub self_announce: SelfAnnounceEntropy,
}

impl EngineCycleEntropy {
    /// Split the raw seed into jitter and self-announce entropy.
    pub fn from_seed(seed: EngineCycleEntropySeed) -> Self {
        let bytes = seed.as_bytes();
        let mut jitter = [0u8; JITTER_SEED_LEN];
        jitter.copy_from_slice(&bytes[..JITTER_SEED_LEN]);
        let mut nonce = [0u8; SelfAnnounceEntropy::LEN];
        nonce.copy_from_slice(&bytes[JITTER_SEED_LEN..]);
        Self {
            jitter: JitterSeed(u64::from_le_bytes(jitter)),
            self_announce: SelfAnnounceEntropy::new(nonce),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextScheduledEngineWork {
    Immediate,
    At(InstantMillis),
    Idle,
}

pub struct EngineState<S: Storage> {
    tick_count: u64,
    ingested_packet_count: u64,
    routing_table: RoutingTable<S::Routes, S::Announces, S::History, S::AppData>,
    held_announces_cache: S::Held,
    pending_rebroadcasts: S::Pending,
    directives: S::Directives,
    interfaces: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
    identity: Option<InMemoryNodeIdentity>,
    self_announce: Option<SelfAnnounceSettings>,
}

impl<S: Storage> Default for EngineState<S> {
    fn default() -> Self {
        Self {
            tick_count: 0,
            ingested_packet_count: 0,
            routing_table: Default::default(),
            held_announces_cache: Default::default(),
            pending_rebroadcasts: Default::default(),
            directives: Default::default(),
            interfaces: HeaplessVec::new(),
            identity: None,
            self_announce: None,
        }
    }
}

impl<S: Storage> core::fmt::Debug for EngineState<S>
where
    S::Routes: core::fmt::Debug,
    S::Announces: core::fmt::Debug,
    S::History: core::fmt::Debug,
    S::AppData: core::fmt::Debug,
    S::Held: core::fmt::Debug,
    S::Pending: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("tick_count", &self.tick_count)
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("routing_table", &self.routing_table)
            .field("held_announces_cache", &self.held_announces_cache)
            .field("pending_rebroadcasts", &self.pending_rebroadcasts)
            .field("interfaces", &self.interfaces)
            .field(
                "identity_hash",
                &self.identity.as_ref().map(|id| id.identity_hash()),
            )
            .field("self_announce", &self.self_announce)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterInterfaceError {
    RegistryFull,
    NotTransmitting,
    NotRoutable { state: ConnectionState },
}

impl<S: Storage> EngineState<S> {
    pub fn new(identity_secret_key: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> Self {
        Self {
            identity: Some(InMemoryNodeIdentity::from_secret_key_bytes(
                identity_secret_key,
            )),
            ..Self::default()
        }
    }

    /// TEMPORARY AND WILL BE REMOVED
    pub fn announcing(
        identity_secret_key: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        self_announce: SelfAnnounceConfig<'_>,
    ) -> Result<Self, SelfAnnounceConfigError> {
        let self_announce = SelfAnnounceSettings::from_config(self_announce)?;
        Ok(Self {
            self_announce: Some(self_announce),
            ..Self::new(identity_secret_key)
        })
    }

    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub const fn ingested_packet_count(&self) -> u64 {
        self.ingested_packet_count
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

    pub fn register_routable_interface_descriptor(
        &mut self,
        descriptor: &InterfaceDescriptor,
    ) -> Result<(), RegisterInterfaceError> {
        match descriptor.state {
            ConnectionState::Connected | ConnectionState::Degraded => {}
            ConnectionState::Initializing
            | ConnectionState::Reconnecting
            | ConnectionState::Failed
            | ConnectionState::Disconnected => {
                return Err(RegisterInterfaceError::NotRoutable {
                    state: descriptor.state,
                });
            }
        }

        if !descriptor.capabilities.allows_transmit() {
            return Err(RegisterInterfaceError::NotTransmitting);
        }

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

    pub fn self_announced_destination(&self) -> Option<DestinationHash> {
        let identity = self.identity.as_ref()?;
        let self_announce = self.self_announce.as_ref()?;
        Some(derive_destination_hash(
            &identity.identity_hash(),
            &self_announce.name_hash(),
        ))
    }

    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        if self.held_announce_count() > 0 {
            return NextScheduledEngineWork::Immediate;
        }

        let mut earliest: Option<InstantMillis> = None;

        if let Some(self_announce) = &self.self_announce {
            if self_announce.is_due(now) {
                return NextScheduledEngineWork::Immediate;
            }
            if let Some(deadline) = self_announce.next_due_at() {
                earliest = Some(earliest.map_or(deadline, |e| e.min(deadline)));
            }
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

    /// Write a due self-announce into `buf`, if one is due at `now`.
    pub fn write_due_self_announce(
        &mut self,
        now: InstantMillis,
        entropy: SelfAnnounceEntropy,
        buf: &mut [u8],
    ) -> Option<usize> {
        let identity = self.identity.as_ref()?;
        let self_announce = self.self_announce.as_ref()?;
        if !self_announce.is_due(now) {
            return None;
        }

        let announce = Announce::build_signed(
            identity,
            self_announce.name_hash(),
            AnnounceId::mint(entropy, now),
            None,
            self_announce.app_data(),
        )
        .expect("bounded self-announce app data always fits an announce");
        let written = write_announce_wire_packet(&announce, 0, buf)
            .expect("MTU-sized buffer fits a bounded self-announce");

        self.self_announce
            .as_mut()
            .expect("self_announce was Some above")
            .mark_announced(now);
        Some(written)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestOutput {
    processed_packet_count: usize,
    accepted_announce_count: usize,
    held_for_retry_count: usize,
    scheduled_rebroadcast_count: usize,
}

impl IngestOutput {
    pub const fn processed_packet_count(&self) -> usize {
        self.processed_packet_count
    }
    pub const fn accepted_announce_count(&self) -> usize {
        self.accepted_announce_count
    }
    pub const fn held_for_retry_count(&self) -> usize {
        self.held_for_retry_count
    }
    pub const fn scheduled_rebroadcast_count(&self) -> usize {
        self.scheduled_rebroadcast_count
    }
}

#[must_use]
pub struct TickOutput<'a, S: Storage> {
    state: &'a mut EngineState<S>,
    now: InstantMillis,
    recovered_from_held_count: usize,
}

impl<'a, S: Storage> TickOutput<'a, S> {
    pub fn egress_directive_count(&self) -> usize {
        self.state.directives.len()
    }

    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }

    pub fn egress_directives(&self) -> impl Iterator<Item = EgressDirective<'_>> + '_ {
        let state = &*self.state;
        state.directives.iter().filter_map(move |directive| {
            let EngineDirective::ReemitAnnounce {
                destination,
                fire_on,
            } = directive;
            let retained = state.routing_table.retained_announce_for(destination)?;
            Some(EgressDirective::ReemitAnnounce {
                announce: retained.announce,
                emit_hops: retained.hops,
                fire_on: fire_on.as_slice(),
            })
        })
    }

    pub fn commit(mut self) {
        self.commit_in_place();
    }

    fn commit_in_place(&mut self) {
        self.state.pending_rebroadcasts.drain_due(self.now);
    }
}

impl<S: Storage> Drop for TickOutput<'_, S> {
    fn drop(&mut self) {
        self.commit_in_place();
    }
}

#[must_use]
pub fn ingest_packets<'p, I, S: Storage>(
    state: &mut EngineState<S>,
    packets: impl IntoIterator<Item = I>,
    jitter: JitterSeed,
) -> IngestOutput
where
    I: core::borrow::Borrow<InboundPacket<'p>>,
{
    let mut counters = IngestCounters::default();
    let mut processed: usize = 0;

    for packet in packets {
        processed += 1;
        let packet: &InboundPacket = core::borrow::Borrow::borrow(&packet);
        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                received_hops,
                source_interface,
                arrived_at,
            } => ingest_announce(
                state,
                announce,
                received_hops,
                source_interface,
                arrived_at,
                jitter,
                &mut counters,
            ),

            Ingress::Data | Ingress::LinkRequest | Ingress::Proof => {}
            Ingress::Unparseable => {}
        }
    }

    state.ingested_packet_count = state.ingested_packet_count.saturating_add(processed as u64);

    IngestOutput {
        processed_packet_count: processed,
        accepted_announce_count: counters.accepted,
        held_for_retry_count: counters.held,
        scheduled_rebroadcast_count: counters.scheduled,
    }
}

#[derive(Default)]
struct IngestCounters {
    accepted: usize,
    held: usize,
    scheduled: usize,
}

fn ingest_announce<S: Storage>(
    state: &mut EngineState<S>,
    announce: Announce<'_>,
    received_hops: u8,
    source_interface: InterfaceId,
    arrived_at: InstantMillis,
    jitter: JitterSeed,
    counters: &mut IngestCounters,
) {
    let decision = AnnounceAcceptanceInput {
        packet_hops: received_hops,
        announce_id: announce.announce_id,
        destination_is_local: false,
        existing_route: state
            .routing_table
            .existing_route_for(&announce.destination),
        arrived_at,
    }
    .determine_acceptance();

    if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
        return;
    }

    let outcome =
        state
            .routing_table
            .upsert_route(received_hops, arrived_at, source_interface, &announce);
    match outcome {
        UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
            counters.accepted += 1;
            let offset = jitter_offset_for(
                jitter,
                &announce.destination,
                DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
            );
            state.pending_rebroadcasts.schedule(
                announce.destination,
                InstantMillis(arrived_at.0.saturating_add(offset)),
                source_interface,
            );
            counters.scheduled += 1;
        }
        UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull) => {
            use crate::routing::held_cache::{HoldReason, ParkOutcome};
            match state.held_announces_cache.park(
                &announce,
                arrived_at,
                received_hops,
                HoldReason::RoutingArenaPressure,
                source_interface,
            ) {
                ParkOutcome::Parked | ParkOutcome::Overwrote => {
                    counters.held += 1;
                }
                ParkOutcome::CacheFull | ParkOutcome::AppDataTooLarge => {}
            }
        }
        UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull) => {}
    }
}

pub fn tick<S: Storage>(
    state: &mut EngineState<S>,
    now: InstantMillis,
    jitter: JitterSeed,
) -> TickOutput<'_, S> {
    state.tick_count = state.tick_count.saturating_add(1);

    let mut recovered_from_held_count = 0;
    while let Some(held) = state.held_announces_cache.take_next() {
        use crate::routing::held_cache::HoldReason;
        match held.reason() {
            HoldReason::RoutingArenaPressure => {
                let announce = held.announce();
                let arrival = held.arrived_at();
                let received_hops = held.received_hops();
                let source_interface = held.source_interface();
                let decision = AnnounceAcceptanceInput {
                    packet_hops: received_hops,
                    announce_id: announce.announce_id,
                    destination_is_local: false,
                    existing_route: state
                        .routing_table
                        .existing_route_for(&announce.destination),
                    arrived_at: arrival,
                }
                .determine_acceptance();
                if matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
                    let outcome = state.routing_table.upsert_route(
                        received_hops,
                        arrival,
                        source_interface,
                        &announce,
                    );
                    if matches!(
                        outcome,
                        UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated
                    ) {
                        recovered_from_held_count += 1;
                        let offset = jitter_offset_for(
                            jitter,
                            &announce.destination,
                            DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                        );
                        state.pending_rebroadcasts.schedule(
                            announce.destination,
                            InstantMillis(arrival.0.saturating_add(offset)),
                            source_interface,
                        );
                    }
                }
            }
        }
    }

    // Materialize this tick's directives from the due rebroadcasts. Indexed (not
    // iterated) so the read of `pending_rebroadcasts` doesn't overlap the write to
    // `directives` — both are fields of `state`.
    state.directives.clear();
    for index in 0..state.pending_rebroadcasts.as_slice().len() {
        let scheduled = state.pending_rebroadcasts.as_slice()[index];
        if scheduled.due_at > now {
            continue;
        }
        let mut fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES> = HeaplessVec::new();
        for &iface in &state.interfaces {
            if iface != scheduled.source_interface {
                let _ = fire_on.push(iface);
            }
        }
        if fire_on.is_empty() {
            continue;
        }
        state.directives.push(EngineDirective::ReemitAnnounce {
            destination: scheduled.destination,
            fire_on,
        });
    }

    TickOutput {
        state,
        now,
        recovered_from_held_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{
        EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceMode, MediumKind,
        TransitCapability,
    };
    use crate::routing::storage::FixedCapacity;
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, PropagationType, WirePacketHeader, MTU,
    };

    const TEST_ENTROPY: JitterSeed = JitterSeed(0xCAFE_F00D_DEAD_BEEF);
    const TEST_NONCE: SelfAnnounceEntropy =
        SelfAnnounceEntropy::new([0xAB; SelfAnnounceEntropy::LEN]);

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct TickSnapshot {
        egress_directive_count: usize,
        recovered_from_held_count: usize,
    }

    fn tick_capture<S: Storage>(
        state: &mut EngineState<S>,
        now: InstantMillis,
    ) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>) {
        let tick_out = tick(state, now, TEST_ENTROPY);
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

    fn observable_state<S: Storage>(
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

    #[test]
    fn tick_advances_count_deterministically() {
        let mut left: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let mut right: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        let (left_out, left_bytes) = tick_capture(&mut left, InstantMillis(1_000));
        let (right_out, right_bytes) = tick_capture(&mut right, InstantMillis(1_000));

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left.tick_count(), 1);
        assert_eq!(left_out, right_out);
        assert_eq!(left_out.egress_directive_count, 0);
        assert!(left_bytes.is_empty());
        assert_eq!(left_bytes, right_bytes);
    }

    #[test]
    fn ingest_counts_the_batch_without_a_clock() {
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let batch = [
            InboundPacket {
                arrived_at: InstantMillis(10),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[1, 2, 3],
            },
            InboundPacket {
                arrived_at: InstantMillis(20),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &[4],
            },
        ];

        let out = ingest_packets(&mut state, batch, TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);

        let empty = ingest_packets(
            &mut state,
            core::iter::empty::<InboundPacket<'_>>(),
            TEST_ENTROPY,
        );
        assert_eq!(empty.processed_packet_count(), 0);
        assert_eq!(state.ingested_packet_count(), 2);
    }

    fn fixed_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(0x22);
        bytes[32..].fill(0x11);
        Zeroizing::new(bytes)
    }

    fn personal_node_announcer() -> EngineState<FixedCapacity> {
        EngineState::announcing(
            &fixed_secret_key(),
            SelfAnnounceConfig {
                app_name: "personal",
                aspects: &["node"],
                app_data: b"hello-personal",
                schedule: ReannounceSchedule::default(),
            },
        )
        .expect("valid self-announce config")
    }

    const SELF_ANNOUNCE_RNS_ANNOUNCE_DATA: &str =
        "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
         ab49baa826f122c1437f44444444444444444444\
         3dba22d6ca6544a5cc056182536b9c42077e769ebd4398fea328a66424fa8972\
         0d8639c7ad031b59ed698508eddf96dc0a130a21af65b2022ae0a118e497660f\
         68656c6c6f2d706572736f6e616c";

    #[test]
    fn self_announce_originates_the_rns_1_3_1_vector() {
        let mut state = personal_node_announcer();
        let now = InstantMillis(0x44_4444_4444);
        let nonce = SelfAnnounceEntropy::new([0x44; SelfAnnounceEntropy::LEN]);

        let mut buf = [0u8; MTU];
        let n = state
            .write_due_self_announce(now, nonce, &mut buf)
            .expect("a self-announce is due on the first call");

        let (header, payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert_eq!(header.hops, 0, "we originate at hop count 0");
        assert_eq!(
            header.destination,
            DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()),
        );
        assert_eq!(payload, hx(SELF_ANNOUNCE_RNS_ANNOUNCE_DATA));
    }

    #[test]
    fn self_announce_is_not_due_again_until_the_interval_elapses() {
        let mut state = personal_node_announcer();
        let mut buf = [0u8; MTU];
        let interval = ReannounceSchedule::default().interval_millis();

        assert!(state
            .write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf)
            .is_some());
        assert!(state
            .write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf)
            .is_none());
        assert!(state
            .write_due_self_announce(InstantMillis(1_000 + interval), TEST_NONCE, &mut buf)
            .is_some());
    }

    #[test]
    fn a_relay_default_state_never_originates() {
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf),
            None,
        );
    }

    #[test]
    fn an_identity_only_node_never_originates() {
        let mut state: EngineState<FixedCapacity> = EngineState::new(&fixed_secret_key());
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf),
            None,
        );
    }

    #[test]
    fn self_announced_destination_reports_our_address_only_when_announcing() {
        assert_eq!(
            personal_node_announcer().self_announced_destination(),
            Some(DestinationHash::new(
                hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()
            )),
        );
        let relay: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        assert_eq!(relay.self_announced_destination(), None);
        let identity_only: EngineState<FixedCapacity> = EngineState::new(&fixed_secret_key());
        assert_eq!(identity_only.self_announced_destination(), None);
    }

    #[test]
    fn next_wakeup_is_idle_for_a_relay_with_no_scheduled_work() {
        let state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
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
            .write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf)
            .expect("first announce is due");

        let interval = ReannounceSchedule::default().interval_millis();
        assert_eq!(
            state.next_wakeup(InstantMillis(2_000)),
            NextScheduledEngineWork::At(InstantMillis(1_000 + interval)),
        );
    }

    #[test]
    fn next_wakeup_accounts_for_a_scheduled_rebroadcast() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let _ = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
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

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn routable_descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            medium: MediumKind::Loopback,
            state: ConnectionState::Connected,
        }
    }

    fn register_test_interface(state: &mut EngineState<FixedCapacity>, id: InterfaceId) {
        state
            .register_routable_interface_descriptor(&routable_descriptor(id))
            .unwrap();
    }

    #[test]
    fn register_routable_descriptor_accepts_a_connected_transmitting_interface() {
        let id = InterfaceId::new([0xAB; 16]);
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        assert_eq!(
            state.register_routable_interface_descriptor(&routable_descriptor(id)),
            Ok(())
        );
        assert_eq!(state.registered_interfaces(), &[id]);
    }

    #[test]
    fn register_routable_descriptor_accepts_degraded_transmitting_interfaces() {
        let id = InterfaceId::new([0xBC; 16]);
        let descriptor = InterfaceDescriptor {
            state: ConnectionState::Degraded,
            ..routable_descriptor(id)
        };
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        assert_eq!(
            state.register_routable_interface_descriptor(&descriptor),
            Ok(())
        );
        assert_eq!(state.registered_interfaces(), &[id]);
    }

    #[test]
    fn register_routable_descriptor_rejects_non_transmitting_interfaces() {
        let mut descriptor = routable_descriptor(InterfaceId::new([0xCD; 16]));
        descriptor.capabilities.egress = EgressCapability::Disabled;
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        assert_eq!(
            state.register_routable_interface_descriptor(&descriptor),
            Err(RegisterInterfaceError::NotTransmitting)
        );
        assert!(state.registered_interfaces().is_empty());
    }

    #[test]
    fn register_routable_descriptor_rejects_unroutable_connection_states() {
        for (idx, connection_state) in [
            ConnectionState::Initializing,
            ConnectionState::Reconnecting,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
        ]
        .into_iter()
        .enumerate()
        {
            let descriptor = InterfaceDescriptor {
                state: connection_state,
                ..routable_descriptor(InterfaceId::new([idx as u8; 16]))
            };
            let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

            assert_eq!(
                state.register_routable_interface_descriptor(&descriptor),
                Err(RegisterInterfaceError::NotRoutable {
                    state: connection_state
                })
            );
            assert!(state.registered_interfaces().is_empty());
        }
    }

    #[test]
    fn ingest_accepts_a_real_announce_then_rejects_its_replay() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        let first = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(first.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);

        let second = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(second.processed_packet_count(), 1);
        assert_eq!(second.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &at_limit,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &beyond,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination =
            DestinationHash::from_slice(&raw[2..18]).expect("16-byte destination hash");

        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        let retained = state
            .routing_table
            .retained_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(retained.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = retained.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn ingest_processes_but_does_not_accept_non_announce_bytes() {
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 16]),
            bytes: &[0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = ingest_packets(&mut state, [junk], TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 1);
        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn arena_full_drops_park_the_inbound_bytes_for_retry() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<FixedCapacity<4, 64, 8>>::default();

        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );

        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(out.held_for_retry_count(), 1);
        assert_eq!(state.route_count(), 0);
        assert_eq!(state.held_announce_count(), 1);
    }

    #[test]
    fn tick_retries_a_held_entry_and_discards_it_when_the_arena_is_still_full() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<FixedCapacity<4, 64, 8>>::default();
        let _ = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.held_announce_count(), 1);

        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(out.recovered_from_held_count, 0);
        assert_eq!(state.held_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn tick_drains_the_entire_held_cache_in_one_pass() {
        use crate::engine::egress::write_announce_wire_packet;
        use crate::routing::announce::expand_name;

        let mut state = EngineState::<FixedCapacity<4, 64, 8>>::default();

        let key = fixed_secret_key();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&key);
        let announce2 = Announce::build_signed(
            &identity,
            expand_name("personal", &["other"]).unwrap(),
            AnnounceId::from_wire([0x55; 10]),
            None,
            b"hello-personal",
        )
        .unwrap();
        let mut buf2 = [0u8; MTU];
        let n2 = write_announce_wire_packet(&announce2, 0, &mut buf2).unwrap();

        let raw1 = hx(RAW_ANNOUNCE);
        let _ = ingest_packets(
            &mut state,
            [
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0u8; 16]),
                    bytes: &raw1,
                },
                InboundPacket {
                    arrived_at: InstantMillis(1_001),
                    source_interface: InterfaceId::new([0u8; 16]),
                    bytes: &buf2[..n2],
                },
            ],
            TEST_ENTROPY,
        );
        assert_eq!(
            state.held_announce_count(),
            2,
            "both distinct destinations parked under arena pressure"
        );

        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(
            state.held_announce_count(),
            0,
            "one tick drains the entire held cache, not just one entry"
        );
        assert_eq!(
            out.recovered_from_held_count, 0,
            "arena still full → both discard"
        );
    }

    #[test]
    fn a_capable_host_can_widen_the_routing_table_at_the_type_level() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<FixedCapacity<64, 128>>::default();
        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn accepted_announces_schedule_a_rebroadcast_and_tick_emits_them() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        register_test_interface(&mut state, InterfaceId::new([0xFE; 16]));

        let arrival = InstantMillis(1_000);
        let out = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(out.scheduled_rebroadcast_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        );
        assert_eq!(tick_out.egress_directive_count, 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        assert_eq!(emitted.len(), 1);
        let wire = &emitted[0];
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        let original = WirePacketHeader::parse(&raw).unwrap().0;
        assert_eq!(header.hops, original.hops + 1);
        assert_eq!(header.destination, original.destination);
        let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn pending_rebroadcasts_are_not_emitted_before_their_due_time() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let arrival = InstantMillis(1_000);
        let _ = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1));
        assert_eq!(tick_out.egress_directive_count, 0);
        assert!(emitted.is_empty());
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);
    }

    #[test]
    fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
        let raw = hx(RAW_ANNOUNCE);
        let now = InstantMillis(5_000);
        let arrival = InstantMillis(1_000);

        let mut left: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();
        let mut right: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::default();

        for state in [&mut left, &mut right] {
            register_test_interface(state, InterfaceId::new([0xFE; 16]));
            let _ = ingest_packets(
                state,
                [InboundPacket {
                    arrived_at: arrival,
                    source_interface: InterfaceId::new([0u8; 16]),
                    bytes: &raw,
                }],
                TEST_ENTROPY,
            );
        }
        let (left_tick, left_bytes) = tick_capture(&mut left, now);
        let (right_tick, right_bytes) = tick_capture(&mut right, now);

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left_tick, right_tick);
        assert_eq!(left_bytes, right_bytes);
        assert_eq!(left_bytes.len(), 1);
    }

    #[test]
    fn held_retry_that_fails_does_not_schedule_a_rebroadcast() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<FixedCapacity<4, 64, 8, 4, 16, 4>>::default();
        let _ = ingest_packets(
            &mut state,
            [InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.held_announce_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        let (tick_out, bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(tick_out.recovered_from_held_count, 0);
        assert_eq!(tick_out.egress_directive_count, 0);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);
        assert!(bytes.is_empty());
    }
}
