//! Pure protocol engine boundary.
//!
//! The engine has two verbs. `ingest` takes a batch of inbound packets, each
//! frozen with the instant it arrived, and is clock-free. `tick` advances the
//! engine's periodic work to a caller-supplied `now`. Neither reads clocks,
//! sockets, or storage directly.

pub mod egress;
pub mod ingress;
pub mod self_announce;

pub use egress::{EgressDirective, EgressSerializeError};
pub use ingress::Ingress;
pub use self_announce::{ReannounceSchedule, SelfAnnounceConfig, SelfAnnounceConfigError};

use crate::engine::egress::write_announce_wire_packet;
use crate::engine::self_announce::SelfAnnounceSettings;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId};
use crate::routing::announce::{
    derive_destination_hash, Announce, AnnounceAcceptanceDecision, AnnounceAcceptanceInput,
    AnnounceId, SelfAnnounceEntropy,
};
use crate::routing::defaults::{jitter_offset_for, JitterSeed};
use crate::routing::held_cache::{HeldAnnouncesCache, DEFAULT_HELD_CACHE_CAPACITY};
use crate::routing::schedule::PendingRebroadcasts;
use crate::routing::storage::{
    AnnounceIdHistory, FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns,
    PackedAppDataArena, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
    TieredAnnounceIdHistory,
};
use crate::routing::{
    DropCause, RoutingTable, UpsertRouteOutcome, DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    DEFAULT_ANNOUNCE_ID_HISTORY_CAP_PER_DESTINATION, DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    DEFAULT_HISTORY_OVERFLOW_CAPACITY, DEFAULT_MAX_TRACKED_DESTINATIONS,
    DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;
use zeroize::Zeroizing;

/// Cap on how many interfaces the engine can own at once. Picked against
/// embedded reality — a real device typically has 1–4 active interfaces; 8
/// gives slack for hosts that present a virtual interface (USB, BLE,
/// loopback for diagnostics) without ballooning per-tick fanout arena
/// storage. Tunable if a real host outgrows it; not exposed as a const
/// generic for now, to keep `EngineState`'s type signature manageable.
pub const MAX_REGISTERED_INTERFACES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstantMillis(pub u64);

/// Bytes the jitter package draws from the per-cycle seed. The jitter package is
/// a `u64`, not raw bytes like the announce nonce, because it is consumed
/// *numerically*: `jitter_offset_for` mixes it with the destination through a
/// splitmix step, so an integer is its natural form and the split converts these
/// bytes to one. (The nonce stays bytes — it is copied to the wire, not computed
/// on.)
const JITTER_SEED_LEN: usize = core::mem::size_of::<u64>();

/// Raw entropy a single step needs, drawn once per cycle by the host and split by
/// [`EngineCycleEntropy::from_seed`] into one typed package per genuine randomness
/// need.
pub const ENGINE_CYCLE_ENTROPY_LEN: usize = JITTER_SEED_LEN + SelfAnnounceEntropy::LEN;

/// The raw CSPRNG bytes a host draws for exactly one engine cycle, before the
/// engine carves them into typed packages at [`EngineCycleEntropy::from_seed`]. A
/// newtype so a host's per-cycle draw is named on the runtime seam and can't be
/// confused with any other buffer; move-only so each step consumes a fresh draw
/// rather than silently reusing one across cycles.
pub struct EngineCycleEntropySeed([u8; ENGINE_CYCLE_ENTROPY_LEN]);

impl EngineCycleEntropySeed {
    pub const fn new(bytes: [u8; ENGINE_CYCLE_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ENGINE_CYCLE_ENTROPY_LEN] {
        &self.0
    }
}

/// One step's entropy, split into a named package per consumer so the type
/// declares exactly what randomness the step needs and for what. Adding a
/// consumer means adding a field here — which forces [`from_seed`](Self::from_seed)
/// and every caller to account for the new draw, rather than silently widening
/// an opaque scalar.
pub struct EngineCycleEntropy {
    pub jitter: JitterSeed,
    pub self_announce: SelfAnnounceEntropy,
}

impl EngineCycleEntropy {
    /// Carve the raw step seed into its packages at the one auditable site:
    /// the low `JITTER_SEED_LEN` bytes seed the jitter spreader, the next
    /// [`SelfAnnounceEntropy::LEN`] are the self-announce nonce.
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
pub struct InboundPacket<'a> {
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub bytes: &'a [u8],
}

/// One serialized Reticulum wire packet on its way out — the outbound
/// counterpart to [`InboundPacket`]. A newtype over the bytes so the egress
/// seam (`InterfaceHandle::send`) names *exactly one packet* rather than a bare
/// `&[u8]` a reader might mistake for a batch of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPacket<'a> {
    pub bytes: &'a [u8],
}

impl<'a> OutboundPacket<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextScheduledEngineWork {
    Immediate,
    At(InstantMillis),
    Idle,
}

/// Retained engine state. **Purely abstract** in its type parameters — does
/// not name a preset. The no_std stack-resident preset lives in
/// [`FixedCapacityEngineState`]; that's the canonical embedded entry point. A
/// capable host substitutes alternate routing-storage backends at the type
/// parameters directly.
///
/// The engine owns its **interface registry** — the host calls
/// [`register_routable_descriptor`] at startup for each interface it presents.
/// From then on the engine computes positive `fire_on` fanout targets per
/// directive (see [`EgressDirective`]) rather than asking the host to apply
/// "don't reflect back to source" by hand.
///
/// [`register_routable_descriptor`]: EngineState::register_routable_descriptor
/// [`EgressDirective`]: crate::engine::EgressDirective
///
/// `Default` builds a **relay**: no identity, no self-announce — it forwards
/// others' announces but originates nothing of its own. A node that signs,
/// agrees, or announces itself is built with [`new`](EngineState::new) or
/// [`announcing`](EngineState::announcing), which hand the engine its hot
/// in-memory identity. Not `Clone`/`PartialEq`/`Eq`: it owns secret key
/// material, which must not be duplicated or compared. `Debug` is hand-written
/// to redact that material (it prints only the identity's public hash).
#[derive(Default)]
pub struct EngineState<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    tick_count: u64,
    ingested_packet_count: u64,
    routing_table: RoutingTable<R, A, H, D>,
    held_announces_cache: HeldAnnouncesCache<MAX_HELD_ANNOUNCES>,
    // The pending-rebroadcast set is capped at `MAX_HELD_ANNOUNCES`, reusing the held-cache
    // dial: one slot per destination, and the realistic per-tick burst of
    // unique accepts is the same order of magnitude as `MAX_HELD_ANNOUNCES` (we already park
    // up to `MAX_HELD_ANNOUNCES` arena-pressure overflows per tick). Hosts that genuinely
    // see larger bursts widen `MAX_HELD_ANNOUNCES` and get both columns at once. A separate
    // `MAX_PENDING` dial is the obvious next iteration if that assumption
    // breaks.
    pending_rebroadcasts: PendingRebroadcasts<MAX_HELD_ANNOUNCES>,
    interfaces: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
    // The node's own identity, owned hot for the engine's lifetime so every
    // signing / key-agreement operation reads it from memory rather than
    // re-deriving or re-fetching per use. `None` for a pure relay. The secret
    // keys inside zeroize on drop and have no byte accessor (see
    // `InMemoryNodeIdentity`)
    identity: Option<InMemoryNodeIdentity>,
    self_announce: Option<SelfAnnounceSettings>,
}

impl<R, A, H, D, const MAX_HELD_ANNOUNCES: usize> core::fmt::Debug
    for EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns + core::fmt::Debug,
    A: RetainedAnnounceColumns + core::fmt::Debug,
    H: AnnounceIdHistory + core::fmt::Debug,
    D: RetainedAppData + core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EngineState")
            .field("tick_count", &self.tick_count)
            .field("ingested_packet_count", &self.ingested_packet_count)
            .field("routing_table", &self.routing_table)
            .field("held_announces_cache", &self.held_announces_cache)
            .field("pending_rebroadcasts", &self.pending_rebroadcasts)
            .field("interfaces", &self.interfaces)
            // Redacted: only the identity's public hash, never its secret keys.
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

/// The no_std stack-resident engine-state preset — the only place the
/// default backend choices are named. Mirrors
/// [`DefaultRoutingTable`](crate::routing::DefaultRoutingTable)
pub type FixedCapacityEngineState<
    const MAX_TRACKED_DESTINATIONS: usize = DEFAULT_MAX_TRACKED_DESTINATIONS,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = DEFAULT_ANNOUNCE_ID_HISTORY_CAP_PER_DESTINATION,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    const HISTORY_FLOOR_PER_DESTINATION: usize = DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    const HISTORY_OVERFLOW_CAPACITY: usize = DEFAULT_HISTORY_OVERFLOW_CAPACITY,
    const HELD_CACHE_CAPACITY: usize = DEFAULT_HELD_CACHE_CAPACITY,
> = EngineState<
    FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>,
    FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>,
    TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >,
    PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
    HELD_CACHE_CAPACITY,
>;

impl<R, A, H, D, const MAX_HELD_ANNOUNCES: usize> EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    /// Build an engine that owns `identity_secret_key` as its hot in-memory
    /// identity (see [`InMemoryNodeIdentity`]) but does not announce a
    /// destination of its own. Use this for a node that needs to sign or agree
    /// without periodically announcing itself; a pure relay needs no identity at
    /// all and is built with [`default`](Default::default).
    ///
    /// `identity_secret_key` is the 64 bytes that *are* the node's two private
    /// keys (X25519 ‖ Ed25519, RNS `prv_bytes` layout) — used verbatim, never
    /// stretched. It arrives through a [`Zeroizing`] buffer the host fills from
    /// its own secret store; that is a deliberately separate channel from the
    /// per-cycle CSPRNG seed (which seeds re-announce-timing jitter and must never be
    /// the source of key material).
    pub fn new(identity_secret_key: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> Self
    where
        R: Default,
        A: Default,
        H: Default,
        D: Default,
    {
        Self {
            identity: Some(InMemoryNodeIdentity::from_secret_key_bytes(
                identity_secret_key,
            )),
            ..Self::default()
        }
    }

    /// Like [`new`](Self::new), and additionally configure the node to announce
    /// its own destination on a cadence. Returns a [`SelfAnnounceConfigError`]
    /// if the destination name or app data is malformed; the identity is always
    /// valid (its bytes are used verbatim).
    pub fn announcing(
        identity_secret_key: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        self_announce: SelfAnnounceConfig<'_>,
    ) -> Result<Self, SelfAnnounceConfigError>
    where
        R: Default,
        A: Default,
        H: Default,
        D: Default,
    {
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

    /// Tracked destinations reachable via `interface` — the per-interface
    /// slice of [`route_count`](Self::route_count) a runtime surfaces per card.
    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routing_table.route_count_via(interface)
    }

    pub fn held_announce_count(&self) -> usize {
        self.held_announces_cache.len()
    }

    pub fn pending_announce_rebroadcast_count(&self) -> usize {
        self.pending_rebroadcasts.pending_count()
    }

    /// Register an interface for engine fanout by its
    /// [`InterfaceDescriptor`] — the routing facts an interface presents, since it
    /// owns its byte I/O and surfaces only what the engine routes on.
    /// Checks the load-bearing contract: it must be `Connected`/`Degraded`
    /// (connected enough to route) and it must be able to transmit. Idempotent:
    /// registering an already-known interface id is a no-op that returns
    /// `Ok(())`.
    pub fn register_routable_descriptor(
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

        if !descriptor.capabilities.transmits {
            return Err(RegisterInterfaceError::NotTransmitting);
        }

        if self.interfaces.contains(&descriptor.id) {
            return Ok(());
        }
        self.interfaces
            .push(descriptor.id)
            .map_err(|_| RegisterInterfaceError::RegistryFull)
    }

    /// Currently-registered interfaces, in registration order.
    pub fn registered_interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }

    /// The destination hash this engine announces itself as, if it self-
    /// announces — `derive_destination_hash(identity, name)`. `None` for a relay
    /// or an identity-only node. Lets a host report its own address (e.g. log it
    /// at startup) without reaching into the identity.
    pub fn self_announced_destination(&self) -> Option<DestinationHash> {
        let identity = self.identity.as_ref()?;
        let self_announce = self.self_announce.as_ref()?;
        Some(derive_destination_hash(
            &identity.identity_hash(),
            &self_announce.name_hash(),
        ))
    }

    /// When the host must next drive the engine, given everything scheduled in
    /// state right now: parked held entries (retried next tick), our own
    /// re-announce cadence, and queued rebroadcasts. This is the **single place**
    /// that folds every timed obligation — any new scheduled behavior MUST be
    /// represented here, or a deadline-driven host would sleep through it. Pure;
    /// call it after a step to decide how long to sleep before the next one.
    pub fn next_wakeup(&self, now: InstantMillis) -> NextScheduledEngineWork {
        // Parked held entries are fully drained on the next tick, so any parked
        // entry means there is work to do right now.
        if self.held_announce_count() > 0 {
            return NextScheduledEngineWork::Immediate;
        }

        let mut earliest: Option<InstantMillis> = None;

        // Our own re-announce cadence.
        if let Some(self_announce) = &self.self_announce {
            if self_announce.is_due(now) {
                return NextScheduledEngineWork::Immediate;
            }
            if let Some(deadline) = self_announce.next_due_at() {
                earliest = Some(earliest.map_or(deadline, |e| e.min(deadline)));
            }
        }

        // Queued rebroadcasts: due now → Immediate, else fold the earliest.
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

    /// If this engine self-announces and one is due at `now`, build and sign our
    /// announce, frame it as a fresh (hop-count 0) broadcast packet into `buf`,
    /// record the emission, and return the bytes written. Returns `None` when we
    /// don't self-announce (relay, or identity-only) or none is due yet.
    ///
    /// The announce id (RNS `random_hash`) is minted from `entropy` (its 5-byte
    /// replay nonce) and `now` (its 5-byte monotonic timebase) — both already
    /// owned by the cycle that drives the engine, so origination needs no clock or RNG
    /// of its own. `buf` should be
    /// [`MTU`](crate::wire::MTU)-sized; the framed announce always fits because
    /// the app data is bounded at construction.
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

/// One due directive's fanout target list, materialised at tick-time.
/// Private to the engine: callers see typed [`EgressDirective`]s via
/// [`TickOutput::egress_directives`].
#[derive(Debug, Clone)]
struct DirectiveFanout {
    destination: DestinationHash,
    fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES>,
}

/// What `tick` produced this cycle.
///
/// Holds a mutable borrow on the engine state for its lifetime — so the
/// host can iterate the typed directives via [`egress_directives`], and
/// the engine's commit (drain of processed entries) happens
/// automatically when this value drops. The borrow also structurally
/// enforces the pseudo-pure tick contract: no other state mutation can
/// happen while a `TickOutput` is alive.
///
/// **What stays behind**: only the directives the host is handed this
/// tick get committed (removed from engine state) on Drop. Everything
/// else the engine had scheduled for a future tick — today only
/// not-yet-due entries in the rebroadcast schedule, tomorrow other
/// directive kinds with their own lifecycle (data sends, proofs, link
/// replies, ...) — stays inside engine state and surfaces on a later
/// tick when its time comes. The engine owns its own schedules; the
/// host is a dumb tx/rx pump. The fixed-cap backends used today cap
/// the in-flight schedule — a growable backend is on the roadmap
/// once we want to mentally validate the "engine has effectively
/// infinite room to carry stuff over" model at scale.
///
/// **Fanout is engine-computed**: each yielded [`EgressDirective`]
/// carries an explicit positive `fire_on: &[InterfaceId]` list. The
/// engine builds this from the [registered interfaces](
/// EngineState::registered_interfaces) minus the source, so the host
/// stays a pure tx/rx pump with no "don't reflect to source" filter
/// logic. Directives whose computed `fire_on` would be empty are
/// elided (engine processed → host sees nothing) but still drained on
/// Drop so they don't re-fire next tick.
///
/// [`egress_directives`]: TickOutput::egress_directives
#[must_use]
pub struct TickOutput<'a, R, A, H, D, const MAX_HELD_ANNOUNCES: usize>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state: &'a mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    now: InstantMillis,
    recovered_from_held_count: usize,
    fanouts: HeaplessVec<DirectiveFanout, MAX_HELD_ANNOUNCES>,
}

impl<'a, R, A, H, D, const MAX_HELD_ANNOUNCES: usize> TickOutput<'a, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    /// Count of directives the host receives this tick — identical to
    /// the number of items [`egress_directives`] will yield.
    ///
    /// [`egress_directives`]: TickOutput::egress_directives
    pub fn egress_directive_count(&self) -> usize {
        self.fanouts.len()
    }

    pub const fn recovered_from_held_count(&self) -> usize {
        self.recovered_from_held_count
    }

    /// Iterate every directive the engine is handing to the host this
    /// tick. Read-only — the iterator yields [`EgressDirective`]
    /// borrowing from `&self` (the announce body comes from the
    /// routing table; the `fire_on` slice comes from this
    /// `TickOutput`'s fanout arena). The host can re-iterate, count,
    /// peek, find, collect snapshots. On [`commit`](Self::commit) (or
    /// Drop as a backstop) the engine removes exactly the set of entries
    /// yielded here from whichever per-kind schedule produced them;
    /// everything else stays in state for a future tick. Today the only source is the
    /// rebroadcast schedule; as more directive kinds land the
    /// iterator will chain across additional sources, each with its
    /// own commit.
    pub fn egress_directives(&self) -> impl Iterator<Item = EgressDirective<'_>> + '_ {
        let state = &*self.state;
        self.fanouts.iter().filter_map(move |fanout| {
            let retained = state
                .routing_table
                .retained_announce_for(&fanout.destination)?;
            Some(EgressDirective::ReemitAnnounce {
                announce: retained.announce,
                emit_hops: retained.hops,
                fire_on: fanout.fire_on.as_slice(),
            })
        })
    }

    /// Commit the tick: remove the due re-emissions this tick yielded from state
    /// so they don't re-fire on a later tick. Consumes the output — releasing its
    /// `&mut state` borrow — so call it once the host has iterated
    /// [`egress_directives`](Self::egress_directives). [`Drop`] runs the same
    /// commit as a backstop if you forget; the drain is idempotent, so the
    /// explicit call plus the Drop backstop never double-commit.
    pub fn commit(mut self) {
        self.commit_in_place();
    }

    /// The commit's work, shared by [`commit`](Self::commit) and [`Drop`]. Each
    /// per-kind schedule drops exactly the entries it just yielded to the host
    /// (today: the rebroadcast set, drained by `due_at <= now`); everything else
    /// stays in state for a future tick. Dispatch failures are NOT retried here —
    /// failure feedback flows back via future inputs (interface state changes,
    /// the re-broadcast cycle), never via cancelling this commit.
    fn commit_in_place(&mut self) {
        self.state.pending_rebroadcasts.drain_due(self.now);
    }
}

impl<R, A, H, D, const MAX_HELD_ANNOUNCES: usize> Drop
    for TickOutput<'_, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    fn drop(&mut self) {
        // Backstop for a caller that didn't call `commit` explicitly.
        self.commit_in_place();
    }
}

/// Process a batch of inbound packets. Clock-free: each packet carries its own
/// arrival instant, so the result is a deterministic function of `(state, packets,
/// entropy)`. An empty batch is valid and a no-op.
#[must_use]
pub fn ingest_packets<'p, I, R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    packets: impl IntoIterator<Item = I>,
    jitter: JitterSeed,
) -> IngestOutput
where
    I: core::borrow::Borrow<InboundPacket<'p>>,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
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

            // Wire-recognised but not yet handled by the engine. Future
            // slices land their dispatch here.
            Ingress::Data | Ingress::LinkRequest | Ingress::Proof => {}

            // Bad header / failed announce validation; dropped.
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

/// Per-batch counters mutated by per-variant ingest handlers. Stays
/// private to `ingest`; the public surface is [`IngestOutput`].
#[derive(Default)]
struct IngestCounters {
    accepted: usize,
    held: usize,
    scheduled: usize,
}

/// Mutates `state` and `counters` in place; currently returns nothing because
/// every branch's side effects are already captured by the counters.
fn ingest_announce<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    announce: Announce<'_>,
    received_hops: u8,
    source_interface: InterfaceId,
    arrived_at: InstantMillis,
    jitter: JitterSeed,
    counters: &mut IngestCounters,
) where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    let decision = AnnounceAcceptanceInput {
        packet_hops: received_hops,
        announce_id: announce.announce_id,
        // No local identities yet, so no announce is ever for us.
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
            // Park the structured announce; retry on tick will
            // re-evaluate against current arena state. Park can return
            // CacheFull (cap reached, dropped) — we count only the
            // successful parks.
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
        UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull) => {
            // Nowhere to retry to until route eviction exists.
        }
    }
}

/// Advance the engine's periodic work to `now`. Fully drains the held-cache
/// (retrying every parked entry, lowest-hops-first, so nothing is left to retry
/// on a later tick), maintains the rebroadcast schedule, and returns a
/// [`TickOutput`] holding `&mut state` until the host has iterated the
/// directives the engine produced.
///
/// `entropy` is the same per-step value passed to `ingest`; reused here
/// so a held-recovery accept gets a deterministic jittered re-emission
/// slot. The returned [`TickOutput`] is itself `#[must_use]`, so
/// dropping it without iterating is a compile-time warning.
pub fn tick<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
    state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
    now: InstantMillis,
    jitter: JitterSeed,
) -> TickOutput<'_, R, A, H, D, MAX_HELD_ANNOUNCES>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    state.tick_count = state.tick_count.saturating_add(1);

    // Drain the WHOLE held cache this tick — not one entry per tick. Each parked
    // entry gets exactly one retry against the current (post-ingest) arena state
    // and is then either installed or discarded; we never re-park (the livelock
    // guard), so the loop strictly shrinks the cache and terminates, bounded by
    // its capacity. Draining fully means the cache is empty after every tick, so
    // the engine's wake-up scheduling never has to account for still-parked work
    // — retry was never time-driven, it was just paced by the old poll cadence.
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
                    // On Dropped(_) or Reject we discard — see the
                    // held-cache module note on livelock avoidance.
                }
            }
        }
    }

    // Materialise per-due-directive fanout: for each rebroadcast whose
    // due_at <= now, build the positive `fire_on` list (registered
    // interfaces minus the source). Directives whose computed list is
    // empty are elided here (engine processed → host sees nothing) but
    // still drained on Drop so they don't re-fire next tick. The host
    // iterates the typed directives via `TickOutput::egress_directives`
    // and commits on Drop. Anything not-yet-due stays parked in
    // `pending_rebroadcasts` and surfaces on a later tick.
    let mut fanouts: HeaplessVec<DirectiveFanout, MAX_HELD_ANNOUNCES> = HeaplessVec::new();
    for scheduled in state
        .pending_rebroadcasts
        .iter()
        .filter(|sr| sr.due_at <= now)
    {
        let mut fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES> = HeaplessVec::new();
        for &iface in &state.interfaces {
            if iface != scheduled.source_interface {
                // Push is infallible: state.interfaces is also capped at
                // MAX_REGISTERED_INTERFACES, so the filter never produces
                // more than the destination's capacity.
                let _ = fire_on.push(iface);
            }
        }
        if fire_on.is_empty() {
            continue;
        }
        // Both caps match (MAX_HELD_ANNOUNCES == max due directives == max fanouts),
        // so push is infallible here too.
        let _ = fanouts.push(DirectiveFanout {
            destination: scheduled.destination,
            fire_on,
        });
    }

    TickOutput {
        state,
        now,
        recovered_from_held_count,
        fanouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{Capabilities, InterfaceMode, MediumKind};
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, PropagationType, WirePacketHeader, MTU,
    };

    /// Fixed entropy so determinism tests can compare two runs apples-to-apples;
    /// the engine treats entropy as opaque data, the value just has to be stable.
    const TEST_ENTROPY: JitterSeed = JitterSeed(0xCAFE_F00D_DEAD_BEEF);
    const TEST_NONCE: SelfAnnounceEntropy =
        SelfAnnounceEntropy::new([0xAB; SelfAnnounceEntropy::LEN]);

    /// What the tests need to assert against a tick, snapshotted to a
    /// value type so it can outlive the `TickOutput` borrow on state.
    /// `TickOutput` itself holds `&mut state` until drop (the commit),
    /// so we can't bubble it out of `tick_capture` — instead we drain
    /// the directives, copy their wire serialisation, and return both
    /// the counters and the captured bytes.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct TickSnapshot {
        egress_directive_count: usize,
        recovered_from_held_count: usize,
    }

    /// Test-side `tick` helper: runs one tick, serializes every due
    /// directive into its own owned wire buffer, and returns the
    /// captured bytes alongside a [`TickSnapshot`]. Tests that don't
    /// care about emission ignore the byte vec.
    fn tick_capture<R, A, H, D, const MAX_HELD_ANNOUNCES: usize>(
        state: &mut EngineState<R, A, H, D, MAX_HELD_ANNOUNCES>,
        now: InstantMillis,
    ) -> (TickSnapshot, std::vec::Vec<std::vec::Vec<u8>>)
    where
        R: RouteColumns,
        A: RetainedAnnounceColumns,
        H: AnnounceIdHistory,
        D: RetainedAppData,
    {
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

    /// The public, observable surface of an engine, snapshotted for
    /// determinism comparisons. `EngineState` is intentionally not `PartialEq`
    /// (it owns secret identity material we must never compare byte-wise), so
    /// "two runs ended in the same place" is asserted through its accessors.
    fn observable_state<R, A, H, D, const N: usize>(
        state: &EngineState<R, A, H, D, N>,
    ) -> (u64, u64, usize, usize, usize, std::vec::Vec<InterfaceId>)
    where
        R: RouteColumns,
        A: RetainedAnnounceColumns,
        H: AnnounceIdHistory,
        D: RetainedAppData,
    {
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
        let mut left: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let mut right: FixedCapacityEngineState = FixedCapacityEngineState::default();

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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
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

        let out = ingest_packets(&mut state, &batch, TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 2);
        assert_eq!(state.ingested_packet_count(), 2);

        // Empty batch is valid and does not move state.
        let empty = ingest_packets(&mut state, &[], TEST_ENTROPY);
        assert_eq!(empty.processed_packet_count(), 0);
        assert_eq!(state.ingested_packet_count(), 2);
    }

    // The 64 secret-key bytes the identity/crypto vectors pin: X25519 prv
    // [0x22; 32] ‖ Ed25519 prv [0x11; 32], i.e. RNS `prv_bytes` for this node.
    fn fixed_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(0x22);
        bytes[32..].fill(0x11);
        Zeroizing::new(bytes)
    }

    // A node that announces destination `personal.node` with app data
    // `hello-personal` on the default cadence.
    fn personal_node_announcer() -> FixedCapacityEngineState {
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

    // The exact `announce_data` RNS 1.3.1 emits for the fixed identity above,
    // destination `personal.node`, random_hash [0x44; 10], and app data
    // `hello-personal` (no ratchet). Generated offline against the oracle.
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
        // `now`'s low 5 bytes become the announce-id timebase and the nonce
        // package supplies the other 5, so the minted random_hash is [0x44; 10]
        // — matching the deterministic oracle vector.
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
        // Byte-for-byte equal to what RNS 1.3.1 puts on the wire for this node.
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
        // Immediately after, nothing is due.
        assert!(state
            .write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf)
            .is_none());
        // One interval later, due again.
        assert!(state
            .write_due_self_announce(InstantMillis(1_000 + interval), TEST_NONCE, &mut buf)
            .is_some());
    }

    #[test]
    fn a_relay_default_state_never_originates() {
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf),
            None,
        );
    }

    #[test]
    fn an_identity_only_node_never_originates() {
        let mut state: FixedCapacityEngineState = EngineState::new(&fixed_secret_key());
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_due_self_announce(InstantMillis(1_000), TEST_NONCE, &mut buf),
            None,
        );
    }

    #[test]
    fn self_announced_destination_reports_our_address_only_when_announcing() {
        // An announcer reports the same destination it puts on the wire.
        assert_eq!(
            personal_node_announcer().self_announced_destination(),
            Some(DestinationHash::new(
                hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()
            )),
        );
        // A relay and an identity-only node have no announced destination.
        let relay: FixedCapacityEngineState = FixedCapacityEngineState::default();
        assert_eq!(relay.self_announced_destination(), None);
        let identity_only: FixedCapacityEngineState = EngineState::new(&fixed_secret_key());
        assert_eq!(identity_only.self_announced_destination(), None);
    }

    #[test]
    fn next_wakeup_is_idle_for_a_relay_with_no_scheduled_work() {
        let state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        assert_eq!(
            state.next_wakeup(InstantMillis(1_000)),
            NextScheduledEngineWork::Idle
        );
    }

    #[test]
    fn next_wakeup_is_immediate_when_a_self_announce_is_due() {
        // A fresh announcer has never announced → due immediately.
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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let _ = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        // Before its (jittered) due time → wake At that instant, within the
        // window after arrival.
        match state.next_wakeup(InstantMillis(0)) {
            NextScheduledEngineWork::At(t) => assert!(
                t.0 >= 1_000 && t.0 < 1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                "due_at {} should sit within the jitter window after arrival",
                t.0,
            ),
            other => panic!("expected At(_), got {other:?}"),
        }

        // Well past the due time → Immediate.
        assert_eq!(
            state.next_wakeup(InstantMillis(1_000_000)),
            NextScheduledEngineWork::Immediate,
        );
    }

    // A genuine RNS 1.3.1 announce (the same vector the announce module validates).
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

    /// A connected, transmitting, full-medium descriptor — the routing facts a
    /// healthy interface presents. Tests tweak individual fields off this base.
    fn routable_descriptor(id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: Capabilities {
                receives: true,
                transmits: true,
                forwards: true,
                repeats: false,
            },
            mode: InterfaceMode::Full,
            medium: MediumKind::Loopback,
            state: ConnectionState::Connected,
        }
    }

    fn register_test_interface(state: &mut FixedCapacityEngineState, id: InterfaceId) {
        state
            .register_routable_descriptor(&routable_descriptor(id))
            .unwrap();
    }

    #[test]
    fn register_routable_descriptor_accepts_a_connected_transmitting_interface() {
        let id = InterfaceId::new([0xAB; 16]);
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();

        assert_eq!(
            state.register_routable_descriptor(&routable_descriptor(id)),
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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();

        assert_eq!(state.register_routable_descriptor(&descriptor), Ok(()));
        assert_eq!(state.registered_interfaces(), &[id]);
    }

    #[test]
    fn register_routable_descriptor_rejects_non_transmitting_interfaces() {
        let mut descriptor = routable_descriptor(InterfaceId::new([0xCD; 16]));
        descriptor.capabilities.transmits = false;
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();

        assert_eq!(
            state.register_routable_descriptor(&descriptor),
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
            let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();

            assert_eq!(
                state.register_routable_descriptor(&descriptor),
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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();

        let first = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(first.accepted_announce_count(), 1);
        assert_eq!(state.route_count(), 1);

        // The identical announce again is a known-route replay: rejected, no new path.
        let second = ingest_packets(
            &mut state,
            &[InboundPacket {
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
        // RNS increments hops on receive, then accepts only `incremented <
        // PATHFINDER_M+1`. So 127 on the wire (128 after the increment) is the
        // last acceptable value, and 128 on the wire (129 after) is beyond reach.
        // The hop byte lives in the header, not the signed payload, so editing it
        // leaves the announce's signature intact.
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &at_limit,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
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

        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);

        // The structured retained announce reproduces the wire payload exactly
        // via to_wire (so the signature still validates on re-emission), and
        // hops are incremented on receive.
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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 16]),
            bytes: &[0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = ingest_packets(&mut state, &[junk], TEST_ENTROPY);
        assert_eq!(out.processed_packet_count(), 1);
        assert_eq!(out.accepted_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn arena_full_drops_park_the_inbound_bytes_for_retry() {
        // Arena tuned to 8 bytes — smaller than the real announce's 14-byte
        // app_data ("hello-personal") — so upsert returns Dropped(PayloadArenaFull).
        let raw = hx(RAW_ANNOUNCE);
        let mut state = FixedCapacityEngineState::<4, 64, 8>::default();

        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
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
        let mut state = FixedCapacityEngineState::<4, 64, 8>::default();
        let _ = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.held_announce_count(), 1);

        // Arena state unchanged → retry hits Dropped(PayloadArenaFull) again
        // and the held entry is discarded. We don't re-park (livelock).
        let (out, _bytes) = tick_capture(&mut state, InstantMillis(2_000));
        assert_eq!(out.recovered_from_held_count, 0);
        assert_eq!(state.held_announce_count(), 0);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn tick_drains_the_entire_held_cache_in_one_pass() {
        // Two distinct destinations both hit arena pressure and park. A single
        // tick must retry BOTH — the cache is drained fully, not one-per-tick —
        // so it is empty afterward (here both discard, the arena stays full).
        use crate::engine::egress::write_announce_wire_packet;
        use crate::routing::announce::expand_name;

        let mut state = FixedCapacityEngineState::<4, 64, 8>::default(); // 8-byte arena

        // A second valid announce for a *different* destination than RAW_ANNOUNCE
        // (fixture identity + a distinct aspect), framed onto the wire so ingest
        // takes it through the same path.
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
            &[
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
        // The const-generic lever: a roomier table is just a different type, and
        // ingest is generic over it — same engine, no heap, no API change. (Very
        // large widths belong on the heap; this inline default lives on the stack.)
        let raw = hx(RAW_ANNOUNCE);
        let mut state = FixedCapacityEngineState::<64, 128>::default();
        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
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
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        // Register a peer so fanout has a target (source is [0u8;16]; the
        // engine's fire_on = registered minus source = [peer]).
        register_test_interface(&mut state, InterfaceId::new([0xFE; 16]));

        let arrival = InstantMillis(1_000);
        let out = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(out.accepted_announce_count(), 1);
        assert_eq!(out.scheduled_rebroadcast_count(), 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        // Far past the jitter window: the rebroadcast is due and tick emits it.
        let (tick_out, emitted) = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
        );
        assert_eq!(tick_out.egress_directive_count, 1);
        assert_eq!(state.pending_announce_rebroadcast_count(), 0);

        // The emitted bytes round-trip back to the same announce, with the
        // hop count incremented (received_hops becomes emit hops). Same
        // signature, so the on-wire packet would re-validate on any peer.
        assert_eq!(emitted.len(), 1);
        let wire = &emitted[0];
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        let original = WirePacketHeader::parse(&raw).unwrap().0;
        assert_eq!(header.hops, original.hops + 1);
        assert_eq!(header.destination, original.destination);
        // And the body bytes are byte-for-byte the same as the original wire
        // payload — `Announce::to_wire(from_wire(payload)) == payload`.
        let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn pending_rebroadcasts_are_not_emitted_before_their_due_time() {
        let raw = hx(RAW_ANNOUNCE);
        let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let arrival = InstantMillis(1_000);
        let _ = ingest_packets(
            &mut state,
            &[InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 16]),
                bytes: &raw,
            }],
            TEST_ENTROPY,
        );
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);

        // `now < arrival` is strictly before any due_at — the offset is
        // non-negative so `due_at >= arrival > now`, and nothing emits.
        let (tick_out, emitted) = tick_capture(&mut state, InstantMillis(arrival.0 - 1));
        assert_eq!(tick_out.egress_directive_count, 0);
        assert!(emitted.is_empty());
        assert_eq!(state.pending_announce_rebroadcast_count(), 1);
    }

    #[test]
    fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
        // Determinism: two engines fed the same packets + same entropy emit
        // the same wire bytes at the same tick. The whole point of "entropy
        // as data" — no hidden RNG state moves results around.
        let raw = hx(RAW_ANNOUNCE);
        let now = InstantMillis(5_000);
        let arrival = InstantMillis(1_000);

        let mut left: FixedCapacityEngineState = FixedCapacityEngineState::default();
        let mut right: FixedCapacityEngineState = FixedCapacityEngineState::default();

        for state in [&mut left, &mut right] {
            // Identical registries: byte-identical emissions depend on
            // both engines computing the same fanout target sets.
            register_test_interface(state, InterfaceId::new([0xFE; 16]));
            let _ = ingest_packets(
                state,
                &[InboundPacket {
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
        // Arena stays full across both calls so the held-cache retry inside
        // `tick` also fails. The schedule should not move: only successful
        // accepts schedule. (The successful held-recovery case is exercised
        // once eviction lands and a follow-up packet can free arena space.)
        let raw = hx(RAW_ANNOUNCE);
        let mut state = FixedCapacityEngineState::<4, 64, 8, 4, 16, 4>::default();
        let _ = ingest_packets(
            &mut state,
            &[InboundPacket {
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
