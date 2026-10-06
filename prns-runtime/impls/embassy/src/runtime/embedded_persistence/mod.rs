#[cfg(test)]
use super::discovery_group_store::{
    restored_discovery_group_configuration_now, restored_discovery_groups_now,
    store_discovery_group_configuration, DISCOVERY_GROUP_CONFIGURATION_STORES,
};
use super::discovery_group_store::{
    DiscoveryGroupConfigurationStoreExchange, GlobalDiscoveryGroupStore,
};
use super::node_name_store::{GlobalNodeNameStore, NodeNameStoreExchange};
use embedded_storage_async::nor_flash::NorFlash;
use heapless::Vec as HeaplessVec;

use crate::crypto::ratchets::{LastRotated, SeedSelfRatchetsOutcome};
use crate::crypto::X25519SecretKey;
use crate::engine::{EngineState, InstantMillis, Journaled, RouteSeedOutcome};
use crate::identity::Zeroizing;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{
    DiscoveryGroupConfigurationSnapshot, DiscoveryGroupConfigurationSnapshotError,
    DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN,
};
use crate::persistence::{
    maximum_route_upsert_payload_len, read_remote_control_controller_grants_snapshot,
    read_remote_control_target_accesses_snapshot, read_routing_table_snapshot,
    read_self_ratchets_snapshot, remote_control_controller_grants_snapshot_capacity,
    remote_control_target_accesses_snapshot_capacity, self_ratchets_snapshot_len,
    write_routing_table_snapshot, write_self_ratchets_snapshot, FlashJournal, FlashJournalError,
    FlashJournalLayout, FlashJournalRecord, FlashJournalRecordKind, FlashJournalWarning,
    SnapshotReadError, TIMEBASE_RECORD_INTERVAL_MILLIS,
};
use crate::remote_control::{
    RemoteControlNodeName, DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
    DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES, NODE_NAME_SNAPSHOT_MAX_LEN,
};
use crate::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use crate::routing::{AnnounceIdRing, PersistedRouteRow};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

use super::AssembledRemoteControl;

const CONTROLLER_GRANTS_SNAPSHOT_CAPACITY: usize =
    remote_control_controller_grants_snapshot_capacity(
        DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
    );
const TARGET_ACCESSES_SNAPSHOT_CAPACITY: usize =
    remote_control_target_accesses_snapshot_capacity(DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES);
pub(crate) const REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY: usize =
    if CONTROLLER_GRANTS_SNAPSHOT_CAPACITY > TARGET_ACCESSES_SNAPSHOT_CAPACITY {
        CONTROLLER_GRANTS_SNAPSHOT_CAPACITY
    } else {
        TARGET_ACCESSES_SNAPSHOT_CAPACITY
    };
const MAXIMUM_RECORD_PAYLOAD_LEN: usize =
    if maximum_route_upsert_payload_len(MAX_ANNOUNCE_APP_DATA_LEN, 0)
        > REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY
        && maximum_route_upsert_payload_len(MAX_ANNOUNCE_APP_DATA_LEN, 0)
            > DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN
    {
        maximum_route_upsert_payload_len(MAX_ANNOUNCE_APP_DATA_LEN, 0)
    } else if REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY
        > DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN
    {
        REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY
    } else {
        DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN
    };
const RECORD_SCRATCH_LEN: usize = (MAXIMUM_RECORD_PAYLOAD_LEN + 3) & !3;
const HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS: u64 = 24 * 60 * 60 * 1_000;

pub(crate) type RemoteControlAuthorizationSnapshot =
    HeaplessVec<u8, REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY>;
const _: () = assert!(
    REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY
        >= DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN
);
const _: () = assert!(MAXIMUM_RECORD_PAYLOAD_LEN >= NODE_NAME_SNAPSHOT_MAX_LEN);

#[derive(Clone, Copy)]
pub struct DiscoveryGroupConfigurationChange {
    interface_id: crate::interfaces::InterfaceId,
    groups: crate::interfaces::DiscoveryGroupSet,
}

impl DiscoveryGroupConfigurationChange {
    #[must_use]
    pub fn upsert(
        interface_id: crate::interfaces::InterfaceId,
        groups: crate::interfaces::DiscoveryGroupSet,
    ) -> Self {
        Self {
            interface_id,
            groups,
        }
    }

    pub(super) fn apply_to(
        &self,
        snapshot: &mut DiscoveryGroupConfigurationSnapshot,
    ) -> Result<(), DiscoveryGroupConfigurationSnapshotError> {
        snapshot.upsert(self.interface_id, self.groups)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedCompactionPolicy {
    minimum_interval_millis: u64,
    critical_reserve_bytes: usize,
}

impl EmbeddedCompactionPolicy {
    #[must_use]
    pub const fn new(minimum_interval_millis: u64, critical_reserve_bytes: usize) -> Self {
        Self {
            minimum_interval_millis,
            critical_reserve_bytes,
        }
    }

    #[must_use]
    pub const fn hopspot(critical_reserve_bytes: usize) -> Self {
        Self::new(
            HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS,
            critical_reserve_bytes,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPersistencePolicy {
    first_route_commit_delay_millis: u64,
    minimum_route_commit_interval_millis: u64,
    ratchet_batch_delay_millis: u64,
    retry_interval_millis: u64,
    timebase_record_interval_millis: u64,
    compaction: EmbeddedCompactionPolicy,
}

impl EmbeddedPersistencePolicy {
    #[must_use]
    pub const fn new(
        first_route_commit_delay_millis: u64,
        minimum_route_commit_interval_millis: u64,
        ratchet_batch_delay_millis: u64,
        retry_interval_millis: u64,
        timebase_record_interval_millis: u64,
        compaction: EmbeddedCompactionPolicy,
    ) -> Self {
        Self {
            first_route_commit_delay_millis,
            minimum_route_commit_interval_millis,
            ratchet_batch_delay_millis,
            retry_interval_millis,
            timebase_record_interval_millis,
            compaction,
        }
    }

    #[must_use]
    pub const fn hopspot_default(compaction: EmbeddedCompactionPolicy) -> Self {
        Self::new(
            2_000,
            5 * 60 * 1_000,
            2_000,
            5 * 60 * 1_000,
            TIMEBASE_RECORD_INTERVAL_MILLIS,
            compaction,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSnapshotKeyError {
    Capacity,
}

pub trait RouteSnapshotKeys {
    fn clear(&mut self);
    fn push(&mut self, destination: DestinationHash) -> Result<(), RouteSnapshotKeyError>;
    fn get(&self, index: usize) -> Option<DestinationHash>;
}

pub struct FixedRouteSnapshotKeys<const N: usize> {
    keys: HeaplessVec<DestinationHash, N>,
}

impl<const N: usize> FixedRouteSnapshotKeys<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: HeaplessVec::new(),
        }
    }
}

impl<const N: usize> Default for FixedRouteSnapshotKeys<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RouteSnapshotKeys for FixedRouteSnapshotKeys<N> {
    fn clear(&mut self) {
        self.keys.clear();
    }

    fn push(&mut self, destination: DestinationHash) -> Result<(), RouteSnapshotKeyError> {
        self.keys
            .push(destination)
            .map_err(|_| RouteSnapshotKeyError::Capacity)
    }

    fn get(&self, index: usize) -> Option<DestinationHash> {
        self.keys.get(index).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceFailure {
    Flash,
    Codec,
    Capacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceTarget {
    Routes,
    CriticalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPersistenceRestoreReport {
    pub logical_start: InstantMillis,
    pub route_seeded_count: u32,
    pub route_refused_count: u32,
    pub route_dropped_count: u32,
    pub ratchet_seeded_count: u32,
    pub ratchet_refused_count: u32,
    pub remote_control_controller_grants_restored_count: u32,
    pub remote_control_controller_grants_refused_count: u32,
    pub remote_control_controller_grants_dropped_count: u32,
    pub remote_control_target_accesses_restored_count: u32,
    pub remote_control_target_accesses_refused_count: u32,
    pub remote_control_target_accesses_dropped_count: u32,
    pub discovery_group_configuration_restored: bool,
    pub discovery_group_configuration_refused_count: u32,
    pub warning: Option<FlashJournalWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPersistenceDiagnostic {
    Restored(EmbeddedPersistenceRestoreReport),
    BatchPersisted {
        records: u32,
        at: InstantMillis,
        state_not_saved: bool,
    },
    CompactionStarted {
        at: InstantMillis,
        next_allowed_at: InstantMillis,
    },
    CompactionCompleted {
        records: u32,
        at: InstantMillis,
        state_not_saved: bool,
    },
    DurabilityDeferred {
        target: EmbeddedPersistenceTarget,
        until: InstantMillis,
    },
    WriteFailed {
        failure: EmbeddedPersistenceFailure,
        retry_at: InstantMillis,
    },
    RemoteControlPairingFailed {
        failure: super::EmbeddedRemoteControlPairingPersistenceFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRouteDelta {
    RouteUpsert(DestinationHash),
    RouteRemoval(DestinationHash),
}

impl PendingRouteDelta {
    fn destination(self) -> DestinationHash {
        match self {
            Self::RouteUpsert(destination) | Self::RouteRemoval(destination) => destination,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchKind {
    Routes,
    Ratchets,
    Compaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionPhase {
    RecordBudget { at: InstantMillis },
    Erase { sector: usize },
    EraseInFlight,
    Routes { index: usize },
    Ratchets { index: usize },
    AuthorizationSnapshot(RemoteControlAuthorizationSnapshotKind),
    DiscoveryGroupConfigurations,
    NodeName,
    Commit,
    ConfirmCommit { at: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteControlAuthorizationSnapshotKind {
    ControllerGrants,
    TargetAccesses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreRemoteControlAuthorizationSnapshotOutcome {
    Stored,
    CompactionInProgress,
    ConfirmationPending {
        retry_at: InstantMillis,
    },
    Failed {
        failure: EmbeddedPersistenceFailure,
        // The store owns retry policy; None means it cannot schedule a retry.
        retry_at: Option<InstantMillis>,
    },
}

struct EncodedDelta {
    kind: FlashJournalRecordKind,
    payload: Zeroizing<[u8; RECORD_SCRATCH_LEN]>,
    len: usize,
}

pub struct EmbeddedFlashPersistence<
    F,
    Keys,
    Observe,
    const PENDING: usize,
    Groups = GlobalDiscoveryGroupStore,
    Names = GlobalNodeNameStore,
> where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    groups: Groups,
    names: Names,
    flash: Option<F>,
    journal: Option<FlashJournal<F>>,
    layout: FlashJournalLayout,
    policy: EmbeddedPersistencePolicy,
    observe_diagnostic: Observe,
    pending_routes: HeaplessVec<PendingRouteDelta, PENDING>,
    pending_ratchets: HeaplessVec<DestinationHash, PENDING>,
    compaction_route_keys: Keys,
    compaction_ratchet_keys: HeaplessVec<DestinationHash, PENDING>,
    remote_control_controller_grants_snapshot: Option<RemoteControlAuthorizationSnapshot>,
    remote_control_target_accesses_snapshot: Option<RemoteControlAuthorizationSnapshot>,
    pending_confirmation: Option<(u32, FlashJournalRecordKind)>,
    route_dirty_since: Option<InstantMillis>,
    ratchet_dirty_since: Option<InstantMillis>,
    last_route_success: Option<InstantMillis>,
    last_timebase_success: Option<InstantMillis>,
    retry_not_before: Option<InstantMillis>,
    landing_batch: Option<BatchKind>,
    landing_records: u32,
    compaction: Option<CompactionPhase>,
    compaction_target: Option<EmbeddedPersistenceTarget>,
    snapshot_required: bool,
    snapshot_target: EmbeddedPersistenceTarget,
    next_compaction_not_before: Option<InstantMillis>,
    deferred_target: Option<EmbeddedPersistenceTarget>,
    deferred_until: Option<InstantMillis>,
    write_failed: bool,
}

impl<F, Keys, Observe, const PENDING: usize> EmbeddedFlashPersistence<F, Keys, Observe, PENDING>
where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    #[must_use]
    pub fn new(
        flash: F,
        layout: FlashJournalLayout,
        policy: EmbeddedPersistencePolicy,
        compaction_route_keys: Keys,
        observe_diagnostic: Observe,
    ) -> Self {
        Self::with_configuration_stores(
            flash,
            layout,
            policy,
            compaction_route_keys,
            observe_diagnostic,
            GlobalDiscoveryGroupStore,
            GlobalNodeNameStore,
        )
    }
}

impl<F, Keys, Observe, const PENDING: usize, Groups>
    EmbeddedFlashPersistence<F, Keys, Observe, PENDING, Groups>
where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
    Groups: AsRef<DiscoveryGroupConfigurationStoreExchange>,
{
    /// Uses one node's group exchange and owns a private name exchange.
    /// Use `with_configuration_stores` to expose both exchanges to controllers.
    /// The group exchange must not be shared with another active persistence owner.
    #[must_use]
    pub fn with_discovery_group_store(
        flash: F,
        layout: FlashJournalLayout,
        policy: EmbeddedPersistencePolicy,
        compaction_route_keys: Keys,
        observe_diagnostic: Observe,
        groups: Groups,
    ) -> EmbeddedFlashPersistence<F, Keys, Observe, PENDING, Groups, NodeNameStoreExchange> {
        EmbeddedFlashPersistence::with_configuration_stores(
            flash,
            layout,
            policy,
            compaction_route_keys,
            observe_diagnostic,
            groups,
            NodeNameStoreExchange::new(),
        )
    }
}

impl<F, Keys, Observe, const PENDING: usize, Groups, Names>
    EmbeddedFlashPersistence<F, Keys, Observe, PENDING, Groups, Names>
where
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
    Groups: AsRef<DiscoveryGroupConfigurationStoreExchange>,
    Names: AsRef<NodeNameStoreExchange>,
{
    /// Each active node owns separate exchanges for its durable configuration.
    #[must_use]
    pub fn with_configuration_stores(
        flash: F,
        layout: FlashJournalLayout,
        policy: EmbeddedPersistencePolicy,
        compaction_route_keys: Keys,
        observe_diagnostic: Observe,
        groups: Groups,
        names: Names,
    ) -> Self {
        Self {
            groups,
            names,
            flash: Some(flash),
            journal: None,
            layout,
            policy,
            observe_diagnostic,
            pending_routes: HeaplessVec::new(),
            pending_ratchets: HeaplessVec::new(),
            compaction_route_keys,
            compaction_ratchet_keys: HeaplessVec::new(),
            remote_control_controller_grants_snapshot: None,
            remote_control_target_accesses_snapshot: None,
            pending_confirmation: None,
            route_dirty_since: None,
            ratchet_dirty_since: None,
            last_route_success: None,
            last_timebase_success: None,
            retry_not_before: None,
            landing_batch: None,
            landing_records: 0,
            compaction: None,
            compaction_target: None,
            snapshot_required: false,
            snapshot_target: EmbeddedPersistenceTarget::Routes,
            next_compaction_not_before: None,
            deferred_target: None,
            deferred_until: None,
            write_failed: false,
        }
    }

    #[must_use]
    pub fn state_not_saved(&self) -> bool {
        self.write_failed || self.deferred_target.is_some()
    }

    pub async fn restore<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        remote_control: &mut AssembledRemoteControl,
        raw_now: InstantMillis,
    ) -> EmbeddedPersistenceRestoreReport {
        let Some(mut flash) = self.flash.take() else {
            return self.empty_restore_report(raw_now, Some(FlashJournalWarning::Corrupt));
        };
        let timebase_state = FlashJournal::inspect_timebase_state(&mut flash, self.layout)
            .await
            .ok();
        let logical_start = timebase_state
            .and_then(|state| state.high_water)
            .map_or(raw_now, |high_water| high_water.max(raw_now));
        let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
        let mut report = EmbeddedPersistenceRestoreReport {
            logical_start,
            route_seeded_count: 0,
            route_refused_count: 0,
            route_dropped_count: 0,
            ratchet_seeded_count: 0,
            ratchet_refused_count: 0,
            remote_control_controller_grants_restored_count: 0,
            remote_control_controller_grants_refused_count: 0,
            remote_control_controller_grants_dropped_count: 0,
            remote_control_target_accesses_restored_count: 0,
            remote_control_target_accesses_refused_count: 0,
            remote_control_target_accesses_dropped_count: 0,
            discovery_group_configuration_restored: false,
            discovery_group_configuration_refused_count: 0,
            warning: None,
        };
        let mut controller_grants_snapshot = None;
        let mut target_accesses_snapshot = None;
        let mut discovery_group_configuration_snapshot = None;
        let mut node_name = None;
        let opened = FlashJournal::open(flash, self.layout, &mut scratch[..], |record| {
            apply_record(
                engine,
                remote_control,
                logical_start,
                record,
                ApplyRecordDestinations {
                    controller_grants_snapshot: &mut controller_grants_snapshot,
                    target_accesses_snapshot: &mut target_accesses_snapshot,
                    discovery_group_configuration_snapshot:
                        &mut discovery_group_configuration_snapshot,
                    node_name: &mut node_name,
                    report: &mut report,
                },
            )
        })
        .await;
        let Ok((mut journal, restored)) = opened else {
            report.warning = Some(FlashJournalWarning::Corrupt);
            self.groups.as_ref().publish_restored(None);
            self.names.as_ref().publish_restored(None);
            (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
            return report;
        };
        report.warning = restored.warning;
        self.remote_control_controller_grants_snapshot = controller_grants_snapshot;
        self.remote_control_target_accesses_snapshot = target_accesses_snapshot;
        self.groups
            .as_ref()
            .publish_restored(discovery_group_configuration_snapshot);
        self.names.as_ref().publish_restored(node_name);
        let initialization_failed =
            restored.active_epoch.is_none() && journal.initialize_empty().await.is_err();
        self.next_compaction_not_before = timebase_state
            .and_then(|state| state.last_compaction_attempt)
            .map(|attempt| {
                InstantMillis(
                    attempt
                        .0
                        .saturating_add(self.policy.compaction.minimum_interval_millis),
                )
            });
        self.journal = Some(journal);
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
        if initialization_failed {
            self.note_write_failure(raw_now, EmbeddedPersistenceFailure::Flash);
        }
        report
    }

    fn empty_restore_report(
        &mut self,
        logical_start: InstantMillis,
        warning: Option<FlashJournalWarning>,
    ) -> EmbeddedPersistenceRestoreReport {
        self.groups.as_ref().publish_restored(None);
        self.names.as_ref().publish_restored(None);
        let report = EmbeddedPersistenceRestoreReport {
            logical_start,
            route_seeded_count: 0,
            route_refused_count: 0,
            route_dropped_count: 0,
            ratchet_seeded_count: 0,
            ratchet_refused_count: 0,
            remote_control_controller_grants_restored_count: 0,
            remote_control_controller_grants_refused_count: 0,
            remote_control_controller_grants_dropped_count: 0,
            remote_control_target_accesses_restored_count: 0,
            remote_control_target_accesses_refused_count: 0,
            remote_control_target_accesses_dropped_count: 0,
            discovery_group_configuration_restored: false,
            discovery_group_configuration_refused_count: 0,
            warning,
        };
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::Restored(report));
        report
    }

    fn observe_journaled(&mut self, journaled: &Journaled<'_>, now: InstantMillis) {
        match journaled {
            Journaled::AnnounceHeard { observation, .. } => {
                self.queue_route(PendingRouteDelta::RouteUpsert(observation.destination), now);
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::RouteRemoved { destination, .. } => {
                self.queue_route(PendingRouteDelta::RouteRemoval(*destination), now);
                if self.route_dirty_since.is_none() {
                    self.route_dirty_since = Some(now);
                }
            }
            Journaled::SelfRatchetRotated { destination } => {
                self.queue_ratchet(*destination, now);
                if self.ratchet_dirty_since.is_none() {
                    self.ratchet_dirty_since = Some(now);
                }
            }
            Journaled::AnnounceHeldDropped { .. }
            | Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::PersistenceFlushed { .. }
            | Journaled::PersistenceFlushFailed { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::LinkInterfaceMismatch { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::RemoteControlPairingAvailabilityObserved(_)
            | Journaled::RemoteControlPairingExpired { .. }
            | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
            | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
            | Journaled::RemoteControlTargetPairingAuthorizationRequired { .. }
            | Journaled::RemoteControlTargetPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlTargetPairingExpiredDuringAuthorization { .. }
            | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
            | Journaled::RemoteControlControllerPairingPersistenceRequired(_)
            | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlControllerPairingAuthorizationPersistenceFailed { .. }
            | Journaled::RemoteControlControllerPairingExpired { .. }
            | Journaled::RemoteControlControllerPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingExpired { .. }
            | Journaled::RemoteControlTargetPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
            | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
            | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
        }
    }

    fn queue_route(&mut self, delta: PendingRouteDelta, now: InstantMillis) {
        if self.snapshot_required {
            return;
        }
        if let Some(existing) = self
            .pending_routes
            .iter_mut()
            .find(|pending| pending.destination() == delta.destination())
        {
            *existing = delta;
            return;
        }
        if self.pending_routes.push(delta).is_err() {
            self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
        }
    }

    fn queue_ratchet(&mut self, destination: DestinationHash, now: InstantMillis) {
        if self.pending_ratchets.contains(&destination) {
            return;
        }
        if self.pending_ratchets.push(destination).is_err() {
            self.require_snapshot(EmbeddedPersistenceTarget::CriticalState, now);
        }
    }

    fn require_snapshot(&mut self, target: EmbeddedPersistenceTarget, now: InstantMillis) {
        self.snapshot_required = true;
        self.snapshot_target = match (self.snapshot_target, target) {
            (EmbeddedPersistenceTarget::CriticalState, _)
            | (_, EmbeddedPersistenceTarget::CriticalState) => {
                EmbeddedPersistenceTarget::CriticalState
            }
            (EmbeddedPersistenceTarget::Routes, EmbeddedPersistenceTarget::Routes) => {
                EmbeddedPersistenceTarget::Routes
            }
        };
        self.pending_routes.clear();
        if target == EmbeddedPersistenceTarget::CriticalState {
            self.pending_ratchets.clear();
        }
        let Some(until) = self.next_compaction_not_before else {
            return;
        };
        if now.0 >= until.0 {
            return;
        }
        if self.deferred_target == Some(self.snapshot_target) && self.deferred_until == Some(until)
        {
            return;
        }
        self.deferred_target = Some(self.snapshot_target);
        self.deferred_until = Some(until);
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::DurabilityDeferred {
            target: self.snapshot_target,
            until,
        });
    }

    fn next_deadline(&self, now: InstantMillis) -> Option<InstantMillis> {
        if self.pending_confirmation.is_some() {
            return self.retry_not_before.or(Some(now));
        }
        // A pending settings change is due now: the manifold only runs `progress` once the
        // deadline passes, and `wait_for_work` keeps waking it until the change is taken.
        let configuration_pending =
            self.groups.as_ref().has_pending_request() || self.names.as_ref().has_pending_request();
        if self.journal.is_none() {
            return configuration_pending.then_some(now);
        }
        let mut deadline =
            if self.compaction.is_some() || self.landing_batch.is_some() || configuration_pending {
                Some(now)
            } else {
                None
            };
        let ratchet_ready = self.ratchet_ready_at();
        let route_ready = self.route_ready_at();
        deadline = earlier(deadline, ratchet_ready);
        if self.snapshot_required {
            let requested = match self.snapshot_target {
                EmbeddedPersistenceTarget::Routes => route_ready,
                EmbeddedPersistenceTarget::CriticalState => {
                    earlier(route_ready, ratchet_ready).or(Some(now))
                }
            };
            if let Some(requested) = requested {
                let allowed = self.next_compaction_not_before.unwrap_or(requested);
                deadline = earlier(deadline, Some(InstantMillis(requested.0.max(allowed.0))));
            }
        } else {
            deadline = earlier(deadline, route_ready);
        }
        let timebase_ready = self.last_timebase_success.map_or(now, |last| {
            InstantMillis(
                last.0
                    .saturating_add(self.policy.timebase_record_interval_millis),
            )
        });
        deadline = earlier(deadline, Some(timebase_ready));
        match (deadline, self.retry_not_before) {
            (Some(deadline), Some(retry)) => Some(InstantMillis(deadline.0.max(retry.0))),
            (deadline, None) => deadline,
            (None, Some(_)) => None,
        }
    }

    fn ratchet_ready_at(&self) -> Option<InstantMillis> {
        self.ratchet_dirty_since.map(|dirty| {
            InstantMillis(
                dirty
                    .0
                    .saturating_add(self.policy.ratchet_batch_delay_millis),
            )
        })
    }

    fn route_ready_at(&self) -> Option<InstantMillis> {
        self.route_dirty_since.map(|dirty| {
            let first_ready = dirty
                .0
                .saturating_add(self.policy.first_route_commit_delay_millis);
            let interval_ready = self.last_route_success.map_or(0, |last| {
                last.0
                    .saturating_add(self.policy.minimum_route_commit_interval_millis)
            });
            InstantMillis(first_ready.max(interval_ready))
        })
    }

    async fn progress<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        now: InstantMillis,
    ) {
        if self.pending_confirmation.is_some_and(|(_, kind)| {
            kind != FlashJournalRecordKind::DiscoveryGroupConfigurations
                && kind != FlashJournalRecordKind::NodeName
        }) {
            return;
        }
        // Finish an uncertain name append before a newer group request can claim the writer.
        let group_change = match self.pending_confirmation {
            Some((_, FlashJournalRecordKind::NodeName)) => None,
            _ => self.groups.as_ref().try_take_request(),
        };
        if let Some(change) = group_change {
            match self
                .store_discovery_group_configuration_change(engine, &change, now)
                .await
            {
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                    let result = self
                        .groups
                        .as_ref()
                        .commit_change(&change)
                        .map_err(|_| EmbeddedPersistenceFailure::Codec);
                    self.groups.as_ref().settle(result);
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {
                    self.groups.as_ref().resignal_request(change);
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::ConfirmationPending { .. } => {
                    self.groups.as_ref().resignal_request(change);
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed { failure, .. } => {
                    self.groups.as_ref().settle(Err(failure));
                }
            }
            return;
        }
        if let Some(name) = self.names.as_ref().try_take_request() {
            match self.store_node_name(engine, name, now).await {
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                    self.names.as_ref().commit(name);
                    self.names.as_ref().settle(Ok(()));
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress
                | StoreRemoteControlAuthorizationSnapshotOutcome::ConfirmationPending { .. } => {
                    self.names.as_ref().resignal_request(name);
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed { failure, .. } => {
                    self.names.as_ref().settle(Err(failure));
                }
            }
            return;
        }
        if self.pending_confirmation.is_some()
            || self.retry_not_before.is_some_and(|retry| now.0 < retry.0)
        {
            return;
        }
        if self.compaction.is_some() {
            self.progress_compaction(engine, now).await;
            return;
        }
        if let Some(batch) = self.landing_batch {
            let new_work = match batch {
                BatchKind::Routes => !self.pending_routes.is_empty() || self.snapshot_required,
                BatchKind::Ratchets => !self.pending_ratchets.is_empty(),
                BatchKind::Compaction => {
                    !self.pending_routes.is_empty()
                        || !self.pending_ratchets.is_empty()
                        || self.snapshot_required
                }
            };
            if new_work {
                self.landing_batch = None;
            } else {
                self.land_timebase(batch, now).await;
                return;
            }
        }
        let ratchet_due = self
            .ratchet_ready_at()
            .is_some_and(|ready| now.0 >= ready.0);
        let route_due = self.route_ready_at().is_some_and(|ready| now.0 >= ready.0);
        if ratchet_due {
            if let Some(index) = (!self.pending_ratchets.is_empty()).then_some(0) {
                self.append_ratchet(engine, index, now).await;
                return;
            }
            if self.snapshot_required
                && self.snapshot_target == EmbeddedPersistenceTarget::CriticalState
            {
                self.try_start_compaction(engine, now);
                return;
            }
        }
        if !route_due {
            let timebase_due = self.last_timebase_success.is_none_or(|last| {
                now.0.saturating_sub(last.0) >= self.policy.timebase_record_interval_millis
            });
            if timebase_due {
                self.record_timebase(now).await;
            }
            return;
        }
        if self.snapshot_required {
            self.try_start_compaction(engine, now);
            return;
        }
        if !self.pending_routes.is_empty() {
            self.append_route(engine, 0, now).await;
        }
    }

    async fn append_route<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        index: usize,
        now: InstantMillis,
    ) {
        let delta = self.pending_routes[index];
        let encoded = encode_route_delta(engine, delta);
        let Ok(payload) = encoded else {
            self.note_codec_failure(now);
            return;
        };
        let can_fit = self.journal.as_ref().is_some_and(|journal| {
            journal.active_can_fit(payload.len, self.policy.compaction.critical_reserve_bytes)
        });
        if !can_fit {
            self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
            self.try_start_compaction(engine, now);
            return;
        }
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        let result = journal
            .append(payload.kind, &payload.payload[..payload.len])
            .await;
        match result {
            Ok(()) => {
                self.pending_routes.swap_remove(index);
                self.landing_records = self.landing_records.saturating_add(1);
                if self.pending_routes.is_empty() {
                    self.landing_batch = Some(BatchKind::Routes);
                }
            }
            Err(FlashJournalError::ArenaFull) => {
                self.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
                self.try_start_compaction(engine, now);
            }
            Err(error) => {
                let failure = failure_from_journal(error);
                self.note_write_failure(now, failure);
            }
        }
    }

    async fn append_ratchet<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        index: usize,
        now: InstantMillis,
    ) {
        let destination = self.pending_ratchets[index];
        let Ok(payload) = encode_ratchet(engine, destination) else {
            self.note_codec_failure(now);
            return;
        };
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        let result = journal
            .append(payload.kind, &payload.payload[..payload.len])
            .await;
        match result {
            Ok(()) => {
                self.pending_ratchets.swap_remove(index);
                self.landing_records = self.landing_records.saturating_add(1);
                if self.pending_ratchets.is_empty() {
                    self.landing_batch = Some(BatchKind::Ratchets);
                }
            }
            Err(FlashJournalError::ArenaFull) => {
                self.require_snapshot(EmbeddedPersistenceTarget::CriticalState, now);
                self.try_start_compaction(engine, now);
            }
            Err(error) => self.note_write_failure(now, failure_from_journal(error)),
        }
    }

    pub(crate) async fn store_remote_control_authorization_snapshot<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: &RemoteControlAuthorizationSnapshot,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        let record_kind = match kind {
            RemoteControlAuthorizationSnapshotKind::ControllerGrants => {
                FlashJournalRecordKind::RemoteControlControllerGrants
            }
            RemoteControlAuthorizationSnapshotKind::TargetAccesses => {
                FlashJournalRecordKind::RemoteControlTargetAccesses
            }
        };
        let outcome = self
            .store_critical_snapshot(engine, record_kind, snapshot, now)
            .await;
        if outcome == StoreRemoteControlAuthorizationSnapshotOutcome::Stored {
            match kind {
                RemoteControlAuthorizationSnapshotKind::ControllerGrants => {
                    self.remote_control_controller_grants_snapshot = Some(snapshot.clone());
                }
                RemoteControlAuthorizationSnapshotKind::TargetAccesses => {
                    self.remote_control_target_accesses_snapshot = Some(snapshot.clone());
                }
            }
        }
        outcome
    }

    #[cfg(test)]
    pub(crate) async fn store_discovery_group_configuration_snapshot<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        snapshot: &DiscoveryGroupConfigurationSnapshot,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
        let written = snapshot.encode_into_max(&mut encoded);
        let outcome = self
            .store_critical_snapshot(
                engine,
                FlashJournalRecordKind::DiscoveryGroupConfigurations,
                &encoded[..written],
                now,
            )
            .await;
        if outcome == StoreRemoteControlAuthorizationSnapshotOutcome::Stored {
            self.groups.as_ref().publish_restored(Some(*snapshot));
        }
        outcome
    }

    #[inline(never)]
    pub(crate) async fn store_discovery_group_configuration_change<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        change: &DiscoveryGroupConfigurationChange,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
        let Ok(written) = self.groups.as_ref().encode_projected(change, &mut encoded) else {
            return StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Codec,
                retry_at: None,
            };
        };
        self.store_critical_snapshot(
            engine,
            FlashJournalRecordKind::DiscoveryGroupConfigurations,
            &encoded[..written],
            now,
        )
        .await
    }

    #[inline(never)]
    pub(crate) async fn store_node_name<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        name: RemoteControlNodeName,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        let mut encoded = [0u8; NODE_NAME_SNAPSHOT_MAX_LEN];
        let written = name.encode_snapshot(&mut encoded);
        self.store_critical_snapshot(
            engine,
            FlashJournalRecordKind::NodeName,
            &encoded[..written],
            now,
        )
        .await
    }

    async fn store_critical_snapshot<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        record_kind: FlashJournalRecordKind,
        payload: &[u8],
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        if let Some((at, pending_kind)) = self.pending_confirmation {
            let retry_at = self.retry_not_before.unwrap_or(now);
            if now < retry_at || pending_kind != record_kind {
                return StoreRemoteControlAuthorizationSnapshotOutcome::ConfirmationPending {
                    retry_at,
                };
            }
            let resolution = match self.journal.as_mut() {
                Some(journal) => journal.confirm_append(at, record_kind, payload).await,
                None => Err(FlashJournalError::Uninitialized),
            };
            match resolution {
                Ok(crate::persistence::FlashJournalCommitResolution::Committed) => {
                    self.pending_confirmation = None;
                    self.retry_not_before = None;
                    return StoreRemoteControlAuthorizationSnapshotOutcome::Stored;
                }
                Ok(crate::persistence::FlashJournalCommitResolution::NotCommitted) => {
                    self.pending_confirmation = None;
                    self.note_write_failure(now, EmbeddedPersistenceFailure::Flash);
                    return StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                        failure: EmbeddedPersistenceFailure::Flash,
                        retry_at: self.retry_not_before,
                    };
                }
                Err(error) => {
                    let failure = failure_from_journal(error);
                    self.note_write_failure(now, failure);
                    return StoreRemoteControlAuthorizationSnapshotOutcome::ConfirmationPending {
                        retry_at: self.retry_not_before.unwrap_or(now),
                    };
                }
            }
        }
        if self.retry_not_before.is_some_and(|retry| now.0 < retry.0) {
            return StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Flash,
                retry_at: self.retry_not_before,
            };
        }
        if self.compaction.is_some() {
            self.progress_compaction(engine, now).await;
            return StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress;
        }
        let Some(journal) = self.journal.as_mut() else {
            return StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Flash,
                retry_at: None,
            };
        };
        // Retain the exact record before I/O: cancellation can hide a completed commit.
        self.pending_confirmation = journal.active_append_offset().map(|at| (at, record_kind));
        let appended = journal.append(record_kind, payload).await;
        self.pending_confirmation = None;
        match appended {
            Ok(()) => StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
            Err(FlashJournalError::CommitUnconfirmed { at, .. }) => {
                self.pending_confirmation = Some((at, record_kind));
                self.note_write_failure(now, EmbeddedPersistenceFailure::Flash);
                StoreRemoteControlAuthorizationSnapshotOutcome::ConfirmationPending {
                    retry_at: self.retry_not_before.unwrap_or(now),
                }
            }
            Err(FlashJournalError::ArenaFull) => {
                self.require_snapshot(EmbeddedPersistenceTarget::CriticalState, now);
                let allowed = self.next_compaction_not_before.unwrap_or(now);
                if now.0 < allowed.0 {
                    return StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                        failure: EmbeddedPersistenceFailure::Capacity,
                        retry_at: Some(allowed),
                    };
                }
                self.try_start_compaction(engine, now);
                if self.compaction.is_some() {
                    StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress
                } else {
                    StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                        failure: EmbeddedPersistenceFailure::Capacity,
                        retry_at: self.retry_not_before,
                    }
                }
            }
            Err(error) => {
                let failure = failure_from_journal(error);
                self.note_write_failure(now, failure);
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                    failure,
                    retry_at: self.retry_not_before,
                }
            }
        }
    }

    fn try_start_compaction<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        now: InstantMillis,
    ) {
        let allowed = self.next_compaction_not_before.unwrap_or(now);
        if now.0 < allowed.0 {
            self.require_snapshot(self.snapshot_target, now);
            return;
        }
        self.compaction_route_keys.clear();
        let mut route_capacity_failed = false;
        for destination in engine.persisted_route_destinations() {
            if self.compaction_route_keys.push(destination).is_err() {
                route_capacity_failed = true;
                break;
            }
        }
        if route_capacity_failed {
            self.note_write_failure(now, EmbeddedPersistenceFailure::Capacity);
            return;
        }
        self.compaction_ratchet_keys.clear();
        let mut ratchet_capacity_failed = false;
        for (destination, _, _) in engine.persisted_self_ratchet_rows() {
            if self.compaction_ratchet_keys.push(destination).is_err() {
                ratchet_capacity_failed = true;
                break;
            }
        }
        if ratchet_capacity_failed {
            self.note_write_failure(now, EmbeddedPersistenceFailure::Capacity);
            return;
        }
        let target = self.snapshot_target;
        self.pending_routes.clear();
        self.pending_ratchets.clear();
        self.route_dirty_since = None;
        self.ratchet_dirty_since = None;
        self.snapshot_required = false;
        self.snapshot_target = EmbeddedPersistenceTarget::Routes;
        self.compaction_target = Some(target);
        self.compaction = Some(CompactionPhase::RecordBudget { at: now });
        self.landing_batch = None;
        self.landing_records = 0;
    }

    async fn progress_compaction<S: StorageLayout>(
        &mut self,
        engine: &EngineState<S>,
        now: InstantMillis,
    ) {
        let Some(phase) = self.compaction else {
            return;
        };
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        match phase {
            CompactionPhase::RecordBudget { at } => {
                match journal.record_compaction_budget(at).await {
                    Ok(recorded_at) => {
                        self.last_timebase_success = Some(at);
                        let next_allowed_at = InstantMillis(
                            recorded_at
                                .0
                                .saturating_add(self.policy.compaction.minimum_interval_millis),
                        );
                        self.next_compaction_not_before = Some(next_allowed_at);
                        (self.observe_diagnostic)(
                            EmbeddedPersistenceDiagnostic::CompactionStarted {
                                at,
                                next_allowed_at,
                            },
                        );
                        self.compaction = Some(CompactionPhase::Erase { sector: 0 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Erase { sector } => {
                if sector < journal.inactive_sector_count() {
                    self.compaction = Some(CompactionPhase::EraseInFlight);
                    match journal.erase_inactive_sector(sector).await {
                        Ok(()) => {
                            let next = sector + 1;
                            if next == journal.inactive_sector_count() {
                                if journal.begin_compaction().is_err() {
                                    self.note_write_failure(
                                        now,
                                        EmbeddedPersistenceFailure::Capacity,
                                    );
                                    return;
                                }
                                self.compaction = Some(CompactionPhase::Routes { index: 0 });
                            } else {
                                self.compaction = Some(CompactionPhase::Erase { sector: next });
                            }
                        }
                        Err(error) => {
                            self.note_write_failure(now, failure_from_journal(error));
                        }
                    }
                }
            }
            CompactionPhase::EraseInFlight => {
                self.note_write_failure(now, EmbeddedPersistenceFailure::Flash);
            }
            CompactionPhase::Routes { index } => {
                let mut scratch = [0u8; RECORD_SCRATCH_LEN];
                let Some(destination) = self.compaction_route_keys.get(index) else {
                    self.compaction = Some(CompactionPhase::Ratchets { index: 0 });
                    return;
                };
                let Some(row) = engine.persisted_route_row(&destination) else {
                    self.compaction = Some(CompactionPhase::Routes { index: index + 1 });
                    return;
                };
                let Ok(written) = encode_route_upsert(row, &mut scratch) else {
                    self.note_codec_failure(now);
                    return;
                };
                match journal
                    .append_compacted(FlashJournalRecordKind::RouteUpsert, &scratch[..written])
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::Routes { index: index + 1 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Ratchets { index } => {
                let Some(destination) = self.compaction_ratchet_keys.get(index).copied() else {
                    self.compaction = Some(CompactionPhase::AuthorizationSnapshot(
                        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    ));
                    return;
                };
                let Some((last_rotated, secrets)) = engine.persisted_self_ratchet_row(&destination)
                else {
                    self.compaction = Some(CompactionPhase::Ratchets { index: index + 1 });
                    return;
                };
                let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
                let Ok(written) =
                    encode_ratchet_row(destination, last_rotated, secrets, &mut *scratch)
                else {
                    self.note_codec_failure(now);
                    return;
                };
                match journal
                    .append_compacted(FlashJournalRecordKind::SelfRatchet, &scratch[..written])
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::Ratchets { index: index + 1 });
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::AuthorizationSnapshot(kind) => {
                let (snapshot, kind, next) = match kind {
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants => (
                        self.remote_control_controller_grants_snapshot.as_deref(),
                        FlashJournalRecordKind::RemoteControlControllerGrants,
                        CompactionPhase::AuthorizationSnapshot(
                            RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                        ),
                    ),
                    RemoteControlAuthorizationSnapshotKind::TargetAccesses => (
                        self.remote_control_target_accesses_snapshot.as_deref(),
                        FlashJournalRecordKind::RemoteControlTargetAccesses,
                        CompactionPhase::DiscoveryGroupConfigurations,
                    ),
                };
                let Some(snapshot) = snapshot else {
                    self.compaction = Some(next);
                    return;
                };
                match journal.append_compacted(kind, snapshot).await {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(next);
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::DiscoveryGroupConfigurations => {
                let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
                let Some(written) = self.groups.as_ref().encode_restored(&mut encoded) else {
                    self.compaction = Some(CompactionPhase::NodeName);
                    return;
                };
                match journal
                    .append_compacted(
                        FlashJournalRecordKind::DiscoveryGroupConfigurations,
                        &encoded[..written],
                    )
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::NodeName);
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::NodeName => {
                let mut encoded = [0u8; NODE_NAME_SNAPSHOT_MAX_LEN];
                let Some(written) = self.names.as_ref().encode_restored(&mut encoded) else {
                    self.compaction = Some(CompactionPhase::Commit);
                    return;
                };
                match journal
                    .append_compacted(FlashJournalRecordKind::NodeName, &encoded[..written])
                    .await
                {
                    Ok(()) => {
                        self.landing_records = self.landing_records.saturating_add(1);
                        self.compaction = Some(CompactionPhase::Commit);
                    }
                    Err(error) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::Commit => {
                let Some(at) = journal.compaction_append_offset() else {
                    self.note_write_failure(now, EmbeddedPersistenceFailure::Flash);
                    return;
                };
                self.compaction = Some(CompactionPhase::ConfirmCommit { at });
                match journal.commit_compaction().await {
                    Ok(()) => self.complete_compaction(now),
                    Err(error @ FlashJournalError::CommitUnconfirmed { .. }) => {
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                    Err(error) => {
                        self.compaction = Some(CompactionPhase::Commit);
                        self.note_write_failure(now, failure_from_journal(error));
                    }
                }
            }
            CompactionPhase::ConfirmCommit { at } => {
                match journal.confirm_compaction_commit(at).await {
                    Ok(crate::persistence::FlashJournalCommitResolution::Committed) => {
                        self.complete_compaction(now)
                    }
                    Ok(crate::persistence::FlashJournalCommitResolution::NotCommitted) => {
                        self.compaction = Some(CompactionPhase::Commit);
                        self.note_write_failure(now, EmbeddedPersistenceFailure::Flash);
                    }
                    Err(error) => self.note_write_failure(now, failure_from_journal(error)),
                }
            }
        }
    }

    fn complete_compaction(&mut self, now: InstantMillis) {
        self.compaction = None;
        self.compaction_target = None;
        let records = core::mem::take(&mut self.landing_records);
        self.retry_not_before = None;
        self.write_failed = false;
        if self.snapshot_required {
            self.require_snapshot(self.snapshot_target, now);
        } else {
            self.deferred_target = None;
            self.deferred_until = None;
        }
        let state_not_saved = self.state_not_saved();
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::CompactionCompleted {
            records,
            at: now,
            state_not_saved,
        });
        if !self.snapshot_required
            && self.pending_routes.is_empty()
            && self.pending_ratchets.is_empty()
        {
            self.landing_batch = Some(BatchKind::Compaction);
        }
    }

    async fn land_timebase(&mut self, batch: BatchKind, now: InstantMillis) {
        if !self.record_timebase(now).await {
            return;
        }
        match batch {
            BatchKind::Routes => {
                self.route_dirty_since = None;
                self.last_route_success = Some(now);
            }
            BatchKind::Ratchets => {
                self.ratchet_dirty_since = None;
            }
            BatchKind::Compaction => {
                self.route_dirty_since = None;
                self.ratchet_dirty_since = None;
                self.last_route_success = Some(now);
            }
        }
        self.retry_not_before = None;
        self.write_failed = false;
        let records = core::mem::take(&mut self.landing_records);
        self.landing_batch = None;
        if batch != BatchKind::Compaction {
            let state_not_saved = self.state_not_saved();
            (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::BatchPersisted {
                records,
                at: now,
                state_not_saved,
            });
        }
    }

    async fn record_timebase(&mut self, now: InstantMillis) -> bool {
        let should_record = self.last_timebase_success.is_none_or(|last| {
            now.0.saturating_sub(last.0) >= self.policy.timebase_record_interval_millis
        });
        if !should_record {
            return true;
        }
        let Some(journal) = self.journal.as_mut() else {
            return false;
        };
        if let Err(error) = journal.record_timebase(now).await {
            self.note_write_failure(now, failure_from_journal(error));
            return false;
        }
        self.last_timebase_success = Some(now);
        self.retry_not_before = None;
        self.write_failed = false;
        true
    }

    fn note_codec_failure(&mut self, now: InstantMillis) {
        self.note_write_failure(now, EmbeddedPersistenceFailure::Codec);
    }

    fn note_write_failure(&mut self, now: InstantMillis, failure: EmbeddedPersistenceFailure) {
        let retry_at = InstantMillis(now.0.saturating_add(self.policy.retry_interval_millis));
        self.retry_not_before = Some(retry_at);
        self.write_failed = true;
        if self.compaction.is_some()
            && !matches!(self.compaction, Some(CompactionPhase::ConfirmCommit { .. }))
        {
            let target = self
                .compaction_target
                .unwrap_or(EmbeddedPersistenceTarget::Routes);
            if let Some(journal) = self.journal.as_mut() {
                journal.abort_compaction();
            }
            self.compaction = None;
            self.compaction_target = None;
            self.require_snapshot(target, now);
            match target {
                EmbeddedPersistenceTarget::Routes => {
                    self.route_dirty_since.get_or_insert(now);
                }
                EmbeddedPersistenceTarget::CriticalState => {
                    self.ratchet_dirty_since.get_or_insert(now);
                }
            }
        }
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::WriteFailed { failure, retry_at });
    }
}

pub(crate) trait ManifoldPersistence<S: StorageLayout> {
    fn has_pending_configuration_change(&self) -> bool;
    fn observe(&mut self, journaled: &Journaled<'_>, now: InstantMillis);
    fn deadline(&mut self, now: InstantMillis) -> Option<InstantMillis>;
    async fn wait_for_work(&self) {
        core::future::pending().await
    }
    fn observe_remote_control_pairing_failure(
        &mut self,
        failure: super::EmbeddedRemoteControlPairingPersistenceFailure,
    );
    async fn progress(&mut self, engine: &mut EngineState<S>, now: InstantMillis);
    async fn store_remote_control_authorization_snapshot(
        &mut self,
        engine: &EngineState<S>,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: &RemoteControlAuthorizationSnapshot,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome;
}

impl<S, F, Keys, Observe, const PENDING: usize, Groups, Names> ManifoldPersistence<S>
    for EmbeddedFlashPersistence<F, Keys, Observe, PENDING, Groups, Names>
where
    S: StorageLayout,
    F: NorFlash,
    Keys: RouteSnapshotKeys,
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
    Groups: AsRef<DiscoveryGroupConfigurationStoreExchange>,
    Names: AsRef<NodeNameStoreExchange>,
{
    fn has_pending_configuration_change(&self) -> bool {
        self.pending_confirmation.is_none_or(|(_, kind)| {
            kind == FlashJournalRecordKind::DiscoveryGroupConfigurations
                || kind == FlashJournalRecordKind::NodeName
        }) && (self.groups.as_ref().has_pending_request()
            || self.names.as_ref().has_pending_request())
    }

    fn observe(&mut self, journaled: &Journaled<'_>, now: InstantMillis) {
        self.observe_journaled(journaled, now);
    }

    fn deadline(&mut self, now: InstantMillis) -> Option<InstantMillis> {
        self.next_deadline(now)
    }

    async fn wait_for_work(&self) {
        if self.pending_confirmation.is_some() {
            core::future::pending::<()>().await;
        }
        embassy_futures::select::select(
            self.groups.as_ref().wait_until_request_ready(),
            self.names.as_ref().wait_until_request_ready(),
        )
        .await;
    }

    fn observe_remote_control_pairing_failure(
        &mut self,
        failure: super::EmbeddedRemoteControlPairingPersistenceFailure,
    ) {
        (self.observe_diagnostic)(EmbeddedPersistenceDiagnostic::RemoteControlPairingFailed {
            failure,
        });
    }

    async fn progress(&mut self, engine: &mut EngineState<S>, now: InstantMillis) {
        self.progress(engine, now).await;
    }

    async fn store_remote_control_authorization_snapshot(
        &mut self,
        engine: &EngineState<S>,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: &RemoteControlAuthorizationSnapshot,
        now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        self.store_remote_control_authorization_snapshot(engine, kind, snapshot, now)
            .await
    }
}

pub(crate) struct NoManifoldPersistence;

impl<S: StorageLayout> ManifoldPersistence<S> for NoManifoldPersistence {
    fn has_pending_configuration_change(&self) -> bool {
        false
    }

    fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

    fn deadline(&mut self, _now: InstantMillis) -> Option<InstantMillis> {
        None
    }

    fn observe_remote_control_pairing_failure(
        &mut self,
        _failure: super::EmbeddedRemoteControlPairingPersistenceFailure,
    ) {
    }

    async fn progress(&mut self, _engine: &mut EngineState<S>, _now: InstantMillis) {}

    async fn store_remote_control_authorization_snapshot(
        &mut self,
        _engine: &EngineState<S>,
        _kind: RemoteControlAuthorizationSnapshotKind,
        _snapshot: &RemoteControlAuthorizationSnapshot,
        _now: InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
            failure: EmbeddedPersistenceFailure::Flash,
            retry_at: None,
        }
    }
}

fn encode_route_delta<S: StorageLayout>(
    engine: &EngineState<S>,
    delta: PendingRouteDelta,
) -> Result<EncodedDelta, ()> {
    match delta {
        PendingRouteDelta::RouteUpsert(destination) => {
            let Some(row) = engine.persisted_route_row(&destination) else {
                return encode_tombstone(destination);
            };
            let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
            let written = encode_route_upsert(row, &mut *scratch)?;
            Ok(EncodedDelta {
                kind: FlashJournalRecordKind::RouteUpsert,
                payload: scratch,
                len: written,
            })
        }
        PendingRouteDelta::RouteRemoval(destination) => encode_tombstone(destination),
    }
}

fn encode_route_upsert(mut row: PersistedRouteRow<'_>, scratch: &mut [u8]) -> Result<usize, ()> {
    row.announce_id_ring = AnnounceIdRing::Table(&[]);
    write_routing_table_snapshot(core::iter::once(row), scratch).map_err(|_| ())
}

fn encode_ratchet<S: StorageLayout>(
    engine: &EngineState<S>,
    destination: DestinationHash,
) -> Result<EncodedDelta, ()> {
    let Some((last_rotated, secrets)) = engine.persisted_self_ratchet_row(&destination) else {
        return Err(());
    };
    let mut scratch = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
    let written = encode_ratchet_row(destination, last_rotated, secrets, &mut *scratch)?;
    Ok(EncodedDelta {
        kind: FlashJournalRecordKind::SelfRatchet,
        payload: scratch,
        len: written,
    })
}

fn encode_ratchet_row(
    destination: DestinationHash,
    last_rotated: LastRotated,
    secrets: &[X25519SecretKey],
    scratch: &mut [u8],
) -> Result<usize, ()> {
    let required = self_ratchets_snapshot_len(secrets.len());
    let end = TRUNCATED_HASH_BYTE_LEN.checked_add(required).ok_or(())?;
    let output = scratch.get_mut(..end).ok_or(())?;
    output[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    let written = write_self_ratchets_snapshot(
        last_rotated,
        secrets,
        &mut output[TRUNCATED_HASH_BYTE_LEN..],
    )
    .map_err(|_| ())?;
    Ok(TRUNCATED_HASH_BYTE_LEN + written)
}

fn encode_tombstone(destination: DestinationHash) -> Result<EncodedDelta, ()> {
    let mut payload = Zeroizing::new([0u8; RECORD_SCRATCH_LEN]);
    payload[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    Ok(EncodedDelta {
        kind: FlashJournalRecordKind::RouteRemoval,
        payload,
        len: TRUNCATED_HASH_BYTE_LEN,
    })
}

struct ApplyRecordDestinations<'a> {
    controller_grants_snapshot: &'a mut Option<RemoteControlAuthorizationSnapshot>,
    target_accesses_snapshot: &'a mut Option<RemoteControlAuthorizationSnapshot>,
    discovery_group_configuration_snapshot: &'a mut Option<DiscoveryGroupConfigurationSnapshot>,
    node_name: &'a mut Option<RemoteControlNodeName>,
    report: &'a mut EmbeddedPersistenceRestoreReport,
}

fn apply_record<S: StorageLayout>(
    engine: &mut EngineState<S>,
    remote_control: &mut AssembledRemoteControl,
    now: InstantMillis,
    record: FlashJournalRecord<'_>,
    destinations: ApplyRecordDestinations<'_>,
) {
    let ApplyRecordDestinations {
        controller_grants_snapshot,
        target_accesses_snapshot,
        discovery_group_configuration_snapshot,
        node_name,
        report,
    } = destinations;
    match record.kind {
        FlashJournalRecordKind::ArenaCommit => {}
        FlashJournalRecordKind::RouteUpsert => {
            let Ok(mut rows) = read_routing_table_snapshot(record.payload) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let Some(Ok(row)) = rows.next() else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            if rows.next().is_some() {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            }
            let destination = row.destination;
            let Ok(pending) = engine.prepare_persisted_route(row) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let Ok(verified) = pending.verify() else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let _ = engine.drop_route(&destination, AttachedInterfaces::new(&[]));
            match engine.seed_verified_route(verified, now) {
                RouteSeedOutcome::Seeded => {
                    report.route_seeded_count = report.route_seeded_count.saturating_add(1);
                }
                RouteSeedOutcome::RefusedDestinationMismatch
                | RouteSeedOutcome::RefusedBlackholedIdentity
                | RouteSeedOutcome::RefusedInvalidSignature => {
                    report.route_refused_count = report.route_refused_count.saturating_add(1);
                }
                RouteSeedOutcome::AlreadyPresent
                | RouteSeedOutcome::TableFull
                | RouteSeedOutcome::AppDataArenaFull => {
                    report.route_dropped_count = report.route_dropped_count.saturating_add(1);
                }
            }
        }
        FlashJournalRecordKind::RouteRemoval => {
            let Ok(bytes) = <[u8; TRUNCATED_HASH_BYTE_LEN]>::try_from(record.payload) else {
                report.route_refused_count = report.route_refused_count.saturating_add(1);
                return;
            };
            let destination = DestinationHash::new(bytes);
            let _ = engine.drop_route(&destination, AttachedInterfaces::new(&[]));
        }
        FlashJournalRecordKind::SelfRatchet => {
            let Some((destination, sealed)) = record
                .payload
                .split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()
            else {
                report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                return;
            };
            let Ok(restored) = read_self_ratchets_snapshot(sealed) else {
                report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                return;
            };
            match engine.replace_persisted_self_ratchets(
                &DestinationHash::new(*destination),
                restored.last_rotated,
                restored.secrets_newest_first(),
            ) {
                SeedSelfRatchetsOutcome::Seeded => {
                    report.ratchet_seeded_count = report.ratchet_seeded_count.saturating_add(1);
                }
                SeedSelfRatchetsOutcome::AlreadyMinted | SeedSelfRatchetsOutcome::Untracked => {
                    report.ratchet_refused_count = report.ratchet_refused_count.saturating_add(1);
                }
            }
        }
        FlashJournalRecordKind::RemoteControlControllerGrants => {
            let restored = read_remote_control_controller_grants_snapshot(record.payload);
            let restored = match restored {
                Ok(restored) => restored,
                Err(SnapshotReadError::Envelope(_) | SnapshotReadError::MalformedPayload) => {
                    report.remote_control_controller_grants_refused_count = report
                        .remote_control_controller_grants_refused_count
                        .saturating_add(1);
                    return;
                }
            };
            let restored_count = restored.grant_count();
            let Ok(snapshot) = RemoteControlAuthorizationSnapshot::from_slice(record.payload)
            else {
                report.remote_control_controller_grants_dropped_count = report
                    .remote_control_controller_grants_dropped_count
                    .saturating_add(restored_count as u32);
                return;
            };
            match remote_control.restore_controller_grants(restored) {
                Ok(outcome) => {
                    *controller_grants_snapshot = Some(snapshot);
                    report.remote_control_controller_grants_restored_count =
                        outcome.restored_count as u32;
                }
                Err(
                    super::RemoteControlAuthorizationRestoreError::Unavailable
                    | super::RemoteControlAuthorizationRestoreError::CapacityExhausted,
                ) => {
                    report.remote_control_controller_grants_dropped_count = report
                        .remote_control_controller_grants_dropped_count
                        .saturating_add(restored_count as u32);
                }
            }
        }
        FlashJournalRecordKind::RemoteControlTargetAccesses => {
            let restored = read_remote_control_target_accesses_snapshot(record.payload);
            let restored = match restored {
                Ok(restored) => restored,
                Err(SnapshotReadError::Envelope(_) | SnapshotReadError::MalformedPayload) => {
                    report.remote_control_target_accesses_refused_count = report
                        .remote_control_target_accesses_refused_count
                        .saturating_add(1);
                    return;
                }
            };
            let restored_count = restored.access_count();
            let Ok(snapshot) = RemoteControlAuthorizationSnapshot::from_slice(record.payload)
            else {
                report.remote_control_target_accesses_dropped_count = report
                    .remote_control_target_accesses_dropped_count
                    .saturating_add(restored_count as u32);
                return;
            };
            match remote_control.restore_target_accesses(restored) {
                Ok(outcome) => {
                    *target_accesses_snapshot = Some(snapshot);
                    report.remote_control_target_accesses_restored_count =
                        outcome.restored_count as u32;
                }
                Err(
                    super::RemoteControlAuthorizationRestoreError::Unavailable
                    | super::RemoteControlAuthorizationRestoreError::CapacityExhausted,
                ) => {
                    report.remote_control_target_accesses_dropped_count = report
                        .remote_control_target_accesses_dropped_count
                        .saturating_add(restored_count as u32);
                }
            }
        }
        FlashJournalRecordKind::DiscoveryGroupConfigurations => {
            match DiscoveryGroupConfigurationSnapshot::decode(record.payload) {
                Ok(snapshot) => {
                    *discovery_group_configuration_snapshot = Some(snapshot);
                    report.discovery_group_configuration_restored = true;
                }
                Err(_) => {
                    report.discovery_group_configuration_refused_count = report
                        .discovery_group_configuration_refused_count
                        .saturating_add(1);
                }
            }
        }
        // A later record supersedes an earlier one; a malformed record keeps the previous name.
        FlashJournalRecordKind::NodeName => {
            if let Some(name) = RemoteControlNodeName::decode_snapshot(record.payload) {
                *node_name = Some(name);
            }
        }
    }
}

fn failure_from_journal<E>(error: FlashJournalError<E>) -> EmbeddedPersistenceFailure {
    match error {
        FlashJournalError::Flash(_)
        | FlashJournalError::CommitUnconfirmed { .. }
        | FlashJournalError::VerificationFailed => EmbeddedPersistenceFailure::Flash,
        FlashJournalError::ArenaFull
        | FlashJournalError::OutOfBounds
        | FlashJournalError::Misaligned
        | FlashJournalError::Uninitialized
        | FlashJournalError::CompactionInProgress
        | FlashJournalError::NoCompaction
        | FlashJournalError::PayloadTooLarge
        | FlashJournalError::ScratchTooShort => EmbeddedPersistenceFailure::Capacity,
    }
}

fn earlier(first: Option<InstantMillis>, second: Option<InstantMillis>) -> Option<InstantMillis> {
    match (first, second) {
        (Some(first), Some(second)) => Some(InstantMillis(first.0.min(second.0))),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests;
