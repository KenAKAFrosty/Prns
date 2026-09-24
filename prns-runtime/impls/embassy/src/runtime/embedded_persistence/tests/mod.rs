use super::*;
use crate::persistence::{routing_table_snapshot_len, TIMEBASE_HEADROOM_MILLIS};
use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
use embedded_storage_async::nor_flash::ReadNorFlash;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::vec::Vec;

mod encoding;
mod power_loss;

static DISCOVERY_GROUP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_discovery_group_store() -> std::sync::MutexGuard<'static, ()> {
    DISCOVERY_GROUP_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const ERASE: usize = 512;
const CAPACITY: usize = ERASE * 6;
// One bounded persistence turn per phase, including the optional discovery-group and node-name
// snapshots.
const EMPTY_STATE_COMPACTION_PROGRESS_STEPS: usize = 11;
const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0, ERASE as u32],
    [
        crate::persistence::FlashArenaRange::new((ERASE * 2) as u32, (ERASE * 4) as u32),
        crate::persistence::FlashArenaRange::new((ERASE * 4) as u32, (ERASE * 6) as u32),
    ],
);

#[derive(Debug)]
struct TestFlash {
    bytes: [u8; CAPACITY],
    sector_erases: [u32; CAPACITY / ERASE],
    fail_next_write: Rc<Cell<bool>>,
}

impl TestFlash {
    fn new() -> Self {
        Self {
            bytes: [0xFF; CAPACITY],
            sector_erases: [0; CAPACITY / ERASE],
            fail_next_write: Rc::new(Cell::new(false)),
        }
    }

    fn controlled() -> (Self, Rc<Cell<bool>>) {
        let flash = Self::new();
        let control = Rc::clone(&flash.fail_next_write);
        (flash, control)
    }
}

#[derive(Debug)]
struct TestFlashError;

impl NorFlashError for TestFlashError {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

impl ErrorType for TestFlash {
    type Error = TestFlashError;
}

impl ReadNorFlash for TestFlash {
    const READ_SIZE: usize = 4;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let start = offset as usize;
        let end = start + bytes.len();
        bytes.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }

    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl NorFlash for TestFlash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = ERASE;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if self.fail_next_write.replace(false) {
            return Err(TestFlashError);
        }
        let start = offset as usize;
        for (stored, written) in self.bytes[start..start + bytes.len()].iter_mut().zip(bytes) {
            *stored &= *written;
        }
        Ok(())
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.bytes[from as usize..to as usize].fill(0xFF);
        for sector in from as usize / ERASE..to as usize / ERASE {
            self.sector_erases[sector] = self.sector_erases[sector].saturating_add(1);
        }
        Ok(())
    }
}

fn ready_with_observer<Observe>(
    observe: Observe,
) -> EmbeddedFlashPersistence<TestFlash, FixedRouteSnapshotKeys<8>, Observe, 4>
where
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    embassy_futures::block_on(async {
        let flash = TestFlash::new();
        let mut scratch = [0u8; RECORD_SCRATCH_LEN];
        let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
            .await
            .unwrap();
        journal.initialize_empty().await.unwrap();
        journal
            .record_compaction_budget(InstantMillis(0))
            .await
            .unwrap();
        let mut persistence = EmbeddedFlashPersistence::new(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            observe,
        );
        persistence.flash = None;
        persistence.journal = Some(journal);
        persistence.next_compaction_not_before =
            Some(InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS));
        persistence.last_timebase_success = Some(InstantMillis(0));
        persistence
    })
}

fn ready() -> EmbeddedFlashPersistence<
    TestFlash,
    FixedRouteSnapshotKeys<8>,
    fn(EmbeddedPersistenceDiagnostic),
    4,
> {
    ready_with_observer((|_| {}) as fn(EmbeddedPersistenceDiagnostic))
}

async fn restore_without_remote_control<Observe>(
    persistence: &mut EmbeddedFlashPersistence<TestFlash, FixedRouteSnapshotKeys<8>, Observe, 4>,
    engine: &mut EngineState<crate::storage::GrowableHeap>,
    now: InstantMillis,
) -> EmbeddedPersistenceRestoreReport
where
    Observe: FnMut(EmbeddedPersistenceDiagnostic),
{
    let mut remote_control = crate::runtime::configure_remote_control_service(
        engine,
        crate::remote_control::RemoteControlService::Unavailable,
    )
    .expect("unavailable RemoteControl requires no storage");
    persistence.restore(engine, &mut remote_control, now).await
}

fn available_remote_control(
    engine: &mut EngineState<crate::storage::GrowableHeap>,
) -> AssembledRemoteControl {
    crate::runtime::configure_remote_control_service(
        engine,
        crate::runtime::node_facade::test_remote_control_service(),
    )
    .expect("RemoteControl fits growable storage")
}

fn controller_grants_snapshot(
    remote_control: &AssembledRemoteControl,
) -> RemoteControlAuthorizationSnapshot {
    let mut bytes = [0u8; REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY];
    let written = remote_control
        .write_controller_grants_snapshot(&mut bytes)
        .unwrap()
        .unwrap();
    RemoteControlAuthorizationSnapshot::from_slice(&bytes[..written]).unwrap()
}

fn target_accesses_snapshot(
    remote_control: &AssembledRemoteControl,
) -> RemoteControlAuthorizationSnapshot {
    let mut bytes = [0u8; REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY];
    let written = remote_control
        .write_target_accesses_snapshot(&mut bytes)
        .unwrap()
        .unwrap();
    RemoteControlAuthorizationSnapshot::from_slice(&bytes[..written]).unwrap()
}

fn discovery_group_snapshot(group: &str) -> DiscoveryGroupConfigurationSnapshot {
    let group = crate::interfaces::DiscoveryGroupId::parse(group).expect("valid group");
    let groups = crate::interfaces::DiscoveryGroupSet::singleton(group).expect("valid set");
    DiscoveryGroupConfigurationSnapshot::try_from_entries(&[
        crate::interfaces::DiscoveryGroupConfigurationEntry::new(
            crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]),
            groups,
        ),
    ])
    .expect("valid snapshot")
}

fn signed_route(secret: u8, app_data: &[u8]) -> crate::routing::PersistedRouteRow<'_> {
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::interfaces::InterfaceId;
    use crate::routing::announce::{Announce, AnnounceId, DottedNameHash};
    use crate::routing::routes::RouteEntry;
    use crate::routing::{AnnounceIdRing, NextHop, RouteResponsiveness, RouteRetention};

    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[secret; 64]);
    let announce = Announce::build_signed(
        &signer,
        DottedNameHash::new([secret; 10]),
        AnnounceId::from_wire([secret.wrapping_add(1); 10]),
        None,
        app_data,
    )
    .unwrap();
    crate::routing::PersistedRouteRow {
        destination: announce.destination,
        entry: RouteEntry {
            hops: secret,
            learned_at: InstantMillis(500),
            last_route_activity_at: InstantMillis(700),
            responsiveness: RouteResponsiveness::Responsive,
            receiving_interface: InterfaceId::new([secret; 8]),
            next_hop: NextHop::Direct,
            retention: RouteRetention::Network,
        },
        public_keys: announce.public_keys,
        dotted_name_hash: announce.dotted_name_hash,
        announce_id: announce.announce_id,
        ratchet: announce.ratchet,
        signature: announce.signature,
        app_data,
        announce_id_ring: AnnounceIdRing::Wire(
            &[0; crate::routing::announce::ANNOUNCE_ID_WIRE_LEN],
        ),
    }
}

#[test]
fn exact_route_and_ratchet_deadlines_are_distinct() {
    let _group_store = lock_discovery_group_store();
    let policy = EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(64));
    assert_eq!(policy.first_route_commit_delay_millis, 2_000);
    assert_eq!(policy.minimum_route_commit_interval_millis, 300_000);
    assert_eq!(policy.ratchet_batch_delay_millis, 2_000);
    assert_eq!(policy.retry_interval_millis, 300_000);
    assert_eq!(
        policy.timebase_record_interval_millis,
        TIMEBASE_RECORD_INTERVAL_MILLIS
    );
    assert_eq!(
        policy.compaction.minimum_interval_millis,
        HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS
    );
    assert_eq!(policy.compaction.critical_reserve_bytes, 64);
}

#[test]
fn deadline_formula_batches_first_write_then_honors_five_minutes() {
    let _group_store = lock_discovery_group_store();
    let mut persistence = ready();
    persistence.route_dirty_since = Some(InstantMillis(1_000));
    assert_eq!(
        persistence.next_deadline(InstantMillis(1_500)),
        Some(InstantMillis(3_000))
    );
    persistence.last_route_success = Some(InstantMillis(10_000));
    persistence.route_dirty_since = Some(InstantMillis(11_000));
    assert_eq!(
        persistence.next_deadline(InstantMillis(12_000)),
        Some(InstantMillis(310_000))
    );
    persistence.ratchet_dirty_since = Some(InstantMillis(12_000));
    assert_eq!(
        persistence.next_deadline(InstantMillis(12_000)),
        Some(InstantMillis(14_000))
    );
    persistence.retry_not_before = Some(InstantMillis(400_000));
    assert_eq!(
        persistence.next_deadline(InstantMillis(12_000)),
        Some(InstantMillis(400_000))
    );
}

#[test]
fn repeated_route_and_ratchet_changes_coalesce_by_destination() {
    let _group_store = lock_discovery_group_store();
    let mut persistence = ready();
    let destination = DestinationHash::new([0x11; TRUNCATED_HASH_BYTE_LEN]);
    persistence.queue_route(
        PendingRouteDelta::RouteUpsert(destination),
        InstantMillis(0),
    );
    persistence.queue_route(
        PendingRouteDelta::RouteUpsert(destination),
        InstantMillis(0),
    );
    persistence.queue_route(
        PendingRouteDelta::RouteRemoval(destination),
        InstantMillis(0),
    );
    persistence.queue_ratchet(destination, InstantMillis(0));
    persistence.queue_ratchet(destination, InstantMillis(0));
    assert_eq!(
        persistence.pending_routes.as_slice(),
        &[PendingRouteDelta::RouteRemoval(destination)]
    );
    assert_eq!(persistence.pending_ratchets.as_slice(), &[destination]);
}

#[test]
fn pending_overflow_waits_for_the_batch_deadline_before_compacting() {
    let _group_store = lock_discovery_group_store();
    let mut persistence = ready();
    for byte in 0..5 {
        persistence.queue_route(
            PendingRouteDelta::RouteUpsert(DestinationHash::new([byte; TRUNCATED_HASH_BYTE_LEN])),
            InstantMillis(1_000),
        );
    }
    persistence.route_dirty_since = Some(InstantMillis(1_000));
    assert!(persistence.snapshot_required);
    assert_eq!(
        persistence.deferred_target,
        Some(EmbeddedPersistenceTarget::Routes)
    );
    assert_eq!(
        persistence.next_deadline(InstantMillis(1_500)),
        Some(InstantMillis(TIMEBASE_RECORD_INTERVAL_MILLIS))
    );

    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(2_999)));
    assert_eq!(persistence.compaction, None);
    assert!(persistence.snapshot_required);

    embassy_futures::block_on(persistence.progress(
        &mut engine,
        InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS),
    ));
    assert_eq!(
        persistence.compaction,
        Some(CompactionPhase::RecordBudget {
            at: InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS)
        })
    );
}

#[test]
fn failures_keep_dirty_state_and_raise_the_notice() {
    let _group_store = lock_discovery_group_store();
    let mut persistence = ready();
    let destination = DestinationHash::new([0x22; TRUNCATED_HASH_BYTE_LEN]);
    persistence.queue_route(
        PendingRouteDelta::RouteUpsert(destination),
        InstantMillis(0),
    );
    persistence.note_codec_failure(InstantMillis(1_000));
    assert_eq!(persistence.pending_routes.len(), 1);
    assert_eq!(persistence.retry_not_before, Some(InstantMillis(301_000)));
    assert!(persistence.state_not_saved());
    assert!(!persistence.snapshot_required);

    persistence.retry_not_before = None;
    persistence.compaction = Some(CompactionPhase::Commit);
    persistence.compaction_target = Some(EmbeddedPersistenceTarget::Routes);
    persistence.note_write_failure(InstantMillis(2_000), EmbeddedPersistenceFailure::Flash);
    assert_eq!(persistence.pending_routes.len(), 0);
    assert_eq!(persistence.retry_not_before, Some(InstantMillis(302_000)));
    assert!(persistence.state_not_saved());
    assert!(persistence.snapshot_required);
    assert_eq!(persistence.compaction, None);
}

#[test]
fn legacy_timebase_allows_one_needed_compaction_then_adopts_the_budget_marker() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let (mut journal, _) = {
            let flash = TestFlash::new();
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                .await
                .unwrap()
        };
        journal.initialize_empty().await.unwrap();
        journal
            .record_timebase(InstantMillis(10_000))
            .await
            .unwrap();
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            journal.release(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let report =
            restore_without_remote_control(&mut persistence, &mut engine, InstantMillis(0)).await;
        assert_eq!(persistence.next_compaction_not_before, None);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, report.logical_start);
        persistence.route_dirty_since = Some(InstantMillis(report.logical_start.0 - 2_000));
        persistence
            .progress(&mut engine, report.logical_start)
            .await;
        persistence
            .progress(&mut engine, report.logical_start)
            .await;
        assert!(matches!(
            persistence.compaction,
            Some(CompactionPhase::Erase { sector: 0 })
        ));

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        restore_without_remote_control(&mut restored, &mut engine, InstantMillis(0)).await;
        assert!(restored.next_compaction_not_before.is_some());
    });
}

#[test]
fn restore_uses_the_later_of_flash_high_water_and_the_raw_clock() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let recorded_at = InstantMillis(10_000);
        let flash_high_water = InstantMillis(recorded_at.0 + TIMEBASE_HEADROOM_MILLIS);
        let rtc_after_downtime = InstantMillis(flash_high_water.0 + 86_400_000);

        for raw_now in [InstantMillis(0), rtc_after_downtime] {
            let flash = TestFlash::new();
            let mut scratch = [0u8; RECORD_SCRATCH_LEN];
            let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
                .await
                .unwrap();
            journal.initialize_empty().await.unwrap();
            journal.record_timebase(recorded_at).await.unwrap();

            let mut persistence =
                EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
                    journal.release(),
                    LAYOUT,
                    EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
                        0,
                    )),
                    FixedRouteSnapshotKeys::new(),
                    (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                );
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let report =
                restore_without_remote_control(&mut persistence, &mut engine, raw_now).await;

            assert_eq!(report.logical_start, raw_now.max(flash_high_water));
        }
    });
}

#[test]
fn idle_persistence_advances_the_flash_timebase_on_schedule() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let start =
            restore_without_remote_control(&mut persistence, &mut engine, InstantMillis(1_000))
                .await;

        assert_eq!(
            persistence.next_deadline(start.logical_start),
            Some(start.logical_start)
        );
        persistence.progress(&mut engine, start.logical_start).await;
        assert_eq!(persistence.last_timebase_success, Some(start.logical_start));

        let next = InstantMillis(
            start
                .logical_start
                .0
                .saturating_add(TIMEBASE_RECORD_INTERVAL_MILLIS),
        );
        assert_eq!(persistence.next_deadline(start.logical_start), Some(next));
        persistence
            .progress(&mut engine, InstantMillis(next.0 - 1))
            .await;
        assert_eq!(persistence.last_timebase_success, Some(start.logical_start));
        persistence.progress(&mut engine, next).await;
        assert_eq!(persistence.last_timebase_success, Some(next));

        let mut flash = persistence.journal.take().unwrap().release();
        assert_eq!(
            FlashJournal::inspect_timebase(&mut flash, LAYOUT)
                .await
                .unwrap(),
            Some(InstantMillis(next.0 + TIMEBASE_HEADROOM_MILLIS))
        );
    });
}

#[test]
fn failed_compaction_attempt_consumes_the_daily_budget() {
    let _group_store = lock_discovery_group_store();
    let diagnostics = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&diagnostics);
    let mut persistence = ready_with_observer(move |diagnostic| {
        observed.borrow_mut().push(diagnostic);
    });
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let first = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
    persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, first);
    persistence.route_dirty_since = Some(InstantMillis(first.0 - 2_000));
    embassy_futures::block_on(persistence.progress(&mut engine, first));
    let second = InstantMillis(first.0 + HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
    assert_eq!(persistence.next_compaction_not_before, Some(first));
    embassy_futures::block_on(persistence.progress(&mut engine, first));
    assert_eq!(persistence.next_compaction_not_before, Some(second));
    assert_eq!(
        persistence.compaction,
        Some(CompactionPhase::Erase { sector: 0 })
    );

    persistence.note_write_failure(first, EmbeddedPersistenceFailure::Flash);
    assert_eq!(persistence.compaction, None);
    assert!(persistence.snapshot_required);
    assert_eq!(
        persistence.next_deadline(first),
        Some(InstantMillis(first.0 + TIMEBASE_RECORD_INTERVAL_MILLIS))
    );
    embassy_futures::block_on(persistence.progress(&mut engine, InstantMillis(second.0 - 1)));
    assert_eq!(persistence.compaction, None);
    embassy_futures::block_on(persistence.progress(&mut engine, second));
    assert_eq!(
        persistence.compaction,
        Some(CompactionPhase::RecordBudget { at: second })
    );
    embassy_futures::block_on(persistence.progress(&mut engine, second));
    assert_eq!(
        persistence.compaction,
        Some(CompactionPhase::Erase { sector: 0 })
    );

    let starts = diagnostics
        .borrow()
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic,
                EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
            )
        })
        .count();
    assert_eq!(starts, 2);
}

#[test]
fn recorded_compaction_budget_survives_reboot() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let mut persistence = ready();
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let attempt = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, attempt);
        persistence.route_dirty_since = Some(InstantMillis(attempt.0 - 2_000));
        persistence.progress(&mut engine, attempt).await;
        persistence.progress(&mut engine, attempt).await;
        assert_eq!(
            persistence.compaction,
            Some(CompactionPhase::Erase { sector: 0 })
        );

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let report =
            restore_without_remote_control(&mut restored, &mut engine, InstantMillis(0)).await;
        assert!(report.logical_start.0 >= attempt.0);
        assert_eq!(
            restored.next_compaction_not_before,
            Some(InstantMillis(
                attempt
                    .0
                    .saturating_add(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS)
            ))
        );
    });
}

#[test]
fn marker_write_failure_does_not_consume_the_compaction_budget() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let (flash, fail_next_write) = TestFlash::controlled();
        let mut scratch = [0u8; RECORD_SCRATCH_LEN];
        let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut scratch, |_| {})
            .await
            .unwrap();
        journal.initialize_empty().await.unwrap();
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        persistence.flash = None;
        persistence.journal = Some(journal);
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let attempt = InstantMillis(2_000);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, attempt);
        persistence.route_dirty_since = Some(InstantMillis(0));
        persistence.progress(&mut engine, attempt).await;
        fail_next_write.set(true);
        persistence.progress(&mut engine, attempt).await;
        assert_eq!(persistence.next_compaction_not_before, None);
        assert_eq!(persistence.compaction, None);

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        restore_without_remote_control(&mut restored, &mut engine, InstantMillis(0)).await;
        assert_eq!(restored.next_compaction_not_before, None);
    });
}

#[test]
fn timebase_writes_and_repeated_reboots_do_not_move_the_compaction_deadline() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let mut persistence = ready();
        let deadline = persistence.next_compaction_not_before;
        persistence
            .journal
            .as_mut()
            .unwrap()
            .record_timebase(InstantMillis(3 * 60 * 60 * 1_000))
            .await
            .unwrap();
        let flash = persistence.journal.take().unwrap().release();
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();

        let mut first = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        restore_without_remote_control(&mut first, &mut engine, InstantMillis(0)).await;
        assert_eq!(first.next_compaction_not_before, deadline);

        let flash = first.journal.take().unwrap().release();
        let mut second = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        restore_without_remote_control(&mut second, &mut engine, InstantMillis(0)).await;
        assert_eq!(second.next_compaction_not_before, deadline);
    });
}

#[test]
fn overflow_during_compaction_commits_once_and_defers_the_next_snapshot() {
    let _group_store = lock_discovery_group_store();
    let diagnostics = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&diagnostics);
    let mut persistence = ready_with_observer(move |diagnostic| {
        observed.borrow_mut().push(diagnostic);
    });
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let now = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
    persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
    persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
    embassy_futures::block_on(persistence.progress(&mut engine, now));

    for byte in 0..6 {
        persistence.queue_route(
            PendingRouteDelta::RouteUpsert(DestinationHash::new([byte; TRUNCATED_HASH_BYTE_LEN])),
            now,
        );
    }
    persistence.route_dirty_since = Some(now);
    for _ in 0..EMPTY_STATE_COMPACTION_PROGRESS_STEPS {
        embassy_futures::block_on(persistence.progress(&mut engine, now));
        if persistence.compaction.is_none() {
            break;
        }
    }

    assert_eq!(persistence.compaction, None);
    assert!(persistence.snapshot_required);
    assert!(persistence.state_not_saved());
    assert_eq!(
        persistence.next_deadline(now),
        Some(InstantMillis(now.0 + TIMEBASE_RECORD_INTERVAL_MILLIS))
    );
    let diagnostics = diagnostics.borrow();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                EmbeddedPersistenceDiagnostic::CompactionCompleted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn captured_route_keys_survive_slot_shifts_and_new_routes_land_after_compaction() {
    let _group_store = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let mut persistence = ready();
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let rows = [signed_route(0x31, &[0xA1]), signed_route(0x32, &[0xA2])];
        for row in &rows {
            assert_eq!(
                engine.seed_route(row, InstantMillis(1_000)),
                RouteSeedOutcome::Seeded
            );
        }
        let now = InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
        persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
        persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
        persistence.progress(&mut engine, now).await;
        persistence.progress(&mut engine, now).await;
        persistence.progress(&mut engine, now).await;

        let removed = rows[0].destination;
        let retained = rows[1].destination;
        let _ = engine.drop_route(&removed, AttachedInterfaces::new(&[]));
        let added = signed_route(0x34, &[0xA4]);
        assert_eq!(engine.seed_route(&added, now), RouteSeedOutcome::Seeded);
        persistence.queue_route(PendingRouteDelta::RouteUpsert(added.destination), now);
        persistence.route_dirty_since = Some(now);

        for _ in 0..EMPTY_STATE_COMPACTION_PROGRESS_STEPS {
            persistence.progress(&mut engine, now).await;
            if persistence.compaction.is_none() {
                break;
            }
        }
        assert_eq!(persistence.compaction, None);
        assert_eq!(
            (
                persistence.pending_routes.len(),
                persistence.snapshot_required,
                persistence.write_failed,
                persistence.route_dirty_since,
                persistence.landing_batch,
            ),
            (1, false, false, Some(now), None)
        );
        let correction_at = InstantMillis(now.0 + 2_000);
        persistence.progress(&mut engine, correction_at).await;
        persistence.progress(&mut engine, correction_at).await;

        let flash = persistence.journal.take().unwrap().release();
        let mut scratch = [0u8; RECORD_SCRATCH_LEN];
        let mut restored = Vec::new();
        let _ = FlashJournal::open(flash, LAYOUT, &mut scratch, |record| {
            if record.kind != FlashJournalRecordKind::RouteUpsert {
                return;
            }
            let mut rows = read_routing_table_snapshot(record.payload).unwrap();
            restored.push(rows.next().unwrap().unwrap().destination);
        })
        .await
        .unwrap();
        assert_eq!(restored, vec![retained, added.destination]);
    });
}

#[test]
fn sixteen_route_records_restore_eight_and_report_capacity_drops() {
    let _group_store = lock_discovery_group_store();
    type EightRouteStorage =
        crate::storage::TestFixedStorage<8, 8, 256, 2, 2, 16, 4, 4, 4, 4, 4, 16>;
    let now = InstantMillis(1_000);
    let mut engine = EngineState::<EightRouteStorage>::default();
    let mut remote_control = crate::runtime::configure_remote_control_service(
        &mut engine,
        crate::remote_control::RemoteControlService::Unavailable,
    )
    .expect("unavailable RemoteControl requires no storage");
    let mut controller_grants_snapshot = None;
    let mut target_accesses_snapshot = None;
    let mut discovery_group_configuration_snapshot = None;
    let mut report = EmbeddedPersistenceRestoreReport {
        logical_start: now,
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

    for secret in 1..=16 {
        let row = signed_route(secret, &[]);
        let required = routing_table_snapshot_len(core::iter::once(row.clone()));
        let mut scratch = [0u8; RECORD_SCRATCH_LEN];
        let written =
            write_routing_table_snapshot(core::iter::once(row), &mut scratch[..required]).unwrap();
        apply_record(
            &mut engine,
            &mut remote_control,
            now,
            FlashJournalRecord {
                epoch: 1,
                kind: FlashJournalRecordKind::RouteUpsert,
                payload: &scratch[..written],
            },
            ApplyRecordDestinations {
                controller_grants_snapshot: &mut controller_grants_snapshot,
                target_accesses_snapshot: &mut target_accesses_snapshot,
                discovery_group_configuration_snapshot: &mut discovery_group_configuration_snapshot,
                node_name: &mut None,
                report: &mut report,
            },
        );
    }

    assert_eq!(
        report,
        EmbeddedPersistenceRestoreReport {
            logical_start: now,
            route_seeded_count: 8,
            route_refused_count: 0,
            route_dropped_count: 8,
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
        }
    );
}

#[test]
fn malformed_newest_authorization_records_preserve_the_last_valid_tables() {
    let _group_store = lock_discovery_group_store();
    use crate::identity::IdentityPublicKeys;
    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind, RemoteControlRequestSet,
        RemoteControlTargetAccess, RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
    };

    let now = InstantMillis(1_000);
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote_control = available_remote_control(&mut engine);
    let grant =
        crate::runtime::node_facade::test_remote_control_grant(RemoteControlRequestKind::Describe);
    remote_control.set_controller_grant(grant).unwrap();
    let valid_controller_grants = controller_grants_snapshot(&remote_control);
    remote_control
        .revoke_controller(grant.controller())
        .unwrap();
    let target_public_keys = IdentityPublicKeys {
        encryption: grant.controller().public_keys().encryption,
        signing: grant.controller().public_keys().signing,
    };
    let target = RemoteControlTargetIdentity::new(target_public_keys);
    remote_control
        .set_target_access(
            RemoteControlTargetAccess::new(
                RemoteControlTargetIdentity::new(target_public_keys),
                crate::remote_control::RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
            )
            .unwrap(),
        )
        .unwrap();
    let valid_target_accesses = target_accesses_snapshot(&remote_control);
    remote_control.forget_target(&target).unwrap();
    let mut controller_grants_snapshot = None;
    let mut target_accesses_snapshot = None;
    let mut discovery_group_configuration_snapshot = None;
    let mut report = EmbeddedPersistenceRestoreReport {
        logical_start: now,
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

    for record in [
        FlashJournalRecord {
            epoch: 1,
            kind: FlashJournalRecordKind::RemoteControlControllerGrants,
            payload: &valid_controller_grants,
        },
        FlashJournalRecord {
            epoch: 1,
            kind: FlashJournalRecordKind::RemoteControlTargetAccesses,
            payload: &valid_target_accesses,
        },
        FlashJournalRecord {
            epoch: 1,
            kind: FlashJournalRecordKind::RemoteControlControllerGrants,
            payload: &[0xFF],
        },
        FlashJournalRecord {
            epoch: 1,
            kind: FlashJournalRecordKind::RemoteControlTargetAccesses,
            payload: &[0xFF],
        },
    ] {
        apply_record(
            &mut engine,
            &mut remote_control,
            now,
            record,
            ApplyRecordDestinations {
                controller_grants_snapshot: &mut controller_grants_snapshot,
                target_accesses_snapshot: &mut target_accesses_snapshot,
                discovery_group_configuration_snapshot: &mut discovery_group_configuration_snapshot,
                node_name: &mut None,
                report: &mut report,
            },
        );
    }

    assert_eq!(
        remote_control
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        &[grant],
    );
    assert_eq!(
        remote_control
            .target_accesses()
            .unwrap()
            .accesses_in_identity_hash_order()[0]
            .target(),
        &target,
    );
    assert_eq!(controller_grants_snapshot, Some(valid_controller_grants));
    assert_eq!(target_accesses_snapshot, Some(valid_target_accesses));
    assert_eq!(report.remote_control_controller_grants_restored_count, 1);
    assert_eq!(report.remote_control_controller_grants_refused_count, 1);
    assert_eq!(report.remote_control_controller_grants_dropped_count, 0);
    assert_eq!(report.remote_control_target_accesses_restored_count, 1);
    assert_eq!(report.remote_control_target_accesses_refused_count, 1);
    assert_eq!(report.remote_control_target_accesses_dropped_count, 0);
}

#[test]
fn malformed_newest_discovery_group_record_preserves_the_last_valid_snapshot() {
    let _store = lock_discovery_group_store();
    let now = InstantMillis(1_000);
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote_control = available_remote_control(&mut engine);
    let expected = discovery_group_snapshot("field-mesh");
    let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
    let written = expected.encode(&mut encoded).expect("snapshot fits");
    let mut controller_grants_snapshot = None;
    let mut target_accesses_snapshot = None;
    let mut discovery_group_configuration_snapshot = None;
    let mut report = EmbeddedPersistenceRestoreReport {
        logical_start: now,
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

    for payload in [&encoded[..written], &[0xFF][..]] {
        apply_record(
            &mut engine,
            &mut remote_control,
            now,
            FlashJournalRecord {
                epoch: 1,
                kind: FlashJournalRecordKind::DiscoveryGroupConfigurations,
                payload,
            },
            ApplyRecordDestinations {
                controller_grants_snapshot: &mut controller_grants_snapshot,
                target_accesses_snapshot: &mut target_accesses_snapshot,
                discovery_group_configuration_snapshot: &mut discovery_group_configuration_snapshot,
                node_name: &mut None,
                report: &mut report,
            },
        );
    }

    assert_eq!(discovery_group_configuration_snapshot, Some(expected));
    assert!(report.discovery_group_configuration_restored);
    assert_eq!(report.discovery_group_configuration_refused_count, 1);
}

#[test]
fn isolated_group_owners_keep_admission_completion_and_durable_state_separate() {
    embassy_futures::block_on(async {
        let stores = [
            DiscoveryGroupConfigurationStoreExchange::new(),
            DiscoveryGroupConfigurationStoreExchange::new(),
        ];
        let (flash, fail_write) = TestFlash::controlled();
        let mut owners = [(flash, &stores[0]), (TestFlash::new(), &stores[1])].map(|(flash, store)| {
            EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
                flash, LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), store,
            )
        });
        let mut engines = core::array::from_fn::<_, 2, _>(|_| {
            EngineState::<crate::storage::GrowableHeap>::default()
        });
        for (owner, engine) in owners.iter_mut().zip(&mut engines) {
            let mut remote_control = available_remote_control(engine);
            owner
                .restore(engine, &mut remote_control, InstantMillis(0))
                .await;
        }
        let interface =
            crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
        let groups = crate::interfaces::DiscoveryGroupSet::reticulum();
        let change = || DiscoveryGroupConfigurationChange::upsert(interface, groups);
        let first = stores[0].store(change());
        let second = stores[1].store(change());
        assert_eq!(
            stores[0].store(change()).await,
            Err(EmbeddedPersistenceFailure::Capacity)
        );
        fail_write.set(true);
        owners[0].progress(&mut engines[0], InstantMillis(1)).await;
        assert_eq!(first.await, Err(EmbeddedPersistenceFailure::Flash));
        assert_eq!(stores[0].groups_now(interface), Some(None));
        assert!(stores[1].has_pending_request());
        assert_eq!(stores[1].groups_now(interface), Some(None));
        owners[1].progress(&mut engines[1], InstantMillis(1)).await;
        assert_eq!(second.await, Ok(()));
        assert_eq!(stores[1].groups_now(interface), Some(Some(groups)));
        assert_eq!(stores[0].groups_now(interface), Some(None));

        let abandoned = stores[1].store(change());
        drop(abandoned);
        assert_eq!(
            stores[1].store(change()).await,
            Err(EmbeddedPersistenceFailure::Capacity)
        );
        owners[1].progress(&mut engines[1], InstantMillis(2)).await;
        let next = stores[1].store(change());
        fail_write.set(true);
        let retry = stores[0].store(change());
        owners[0].progress(&mut engines[0], InstantMillis(3)).await;
        assert_eq!(retry.await, Err(EmbeddedPersistenceFailure::Flash));
        assert!(stores[1].has_pending_request());
        owners[1].progress(&mut engines[1], InstantMillis(3)).await;
        assert_eq!(next.await, Ok(()));
    });
    assert_eq!(core::mem::size_of::<GlobalDiscoveryGroupStore>(), 0);
}

#[test]
fn failed_discovery_group_write_retains_the_last_durable_snapshot() {
    let _store = lock_discovery_group_store();
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
    embassy_futures::block_on(async {
        let (flash, fail_next_write) = TestFlash::controlled();
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = available_remote_control(&mut engine);
        persistence
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        let confirmed = discovery_group_snapshot("confirmed");
        assert_eq!(
            persistence
                .store_discovery_group_configuration_snapshot(
                    &engine,
                    &confirmed,
                    InstantMillis(1),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
        );

        fail_next_write.set(true);
        assert!(matches!(
            persistence
                .store_discovery_group_configuration_snapshot(
                    &engine,
                    &discovery_group_snapshot("candidate"),
                    InstantMillis(2),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Flash,
                ..
            },
        ));
        assert_eq!(
            restored_discovery_group_configuration_now(),
            Some(confirmed)
        );

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let report = restored
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        assert!(report.discovery_group_configuration_restored);
        assert_eq!(
            restored_discovery_group_configuration_now(),
            Some(confirmed)
        );
    });
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
}

#[test]
fn discovery_group_snapshot_survives_compaction_and_reboot() {
    let _store = lock_discovery_group_store();
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
    embassy_futures::block_on(async {
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = available_remote_control(&mut engine);
        persistence
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        let expected = discovery_group_snapshot("field-mesh");
        assert_eq!(
            persistence
                .store_discovery_group_configuration_snapshot(&engine, &expected, InstantMillis(1),)
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
        );

        persistence.require_snapshot(EmbeddedPersistenceTarget::CriticalState, InstantMillis(2));
        persistence.try_start_compaction(&engine, InstantMillis(2));
        for now in 2..32 {
            if persistence.compaction.is_none() {
                break;
            }
            persistence
                .progress_compaction(&engine, InstantMillis(now))
                .await;
        }
        assert_eq!(persistence.compaction, None);

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let report = restored
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        assert!(report.discovery_group_configuration_restored);
        assert_eq!(report.discovery_group_configuration_refused_count, 0);
        assert_eq!(restored_discovery_group_configuration_now(), Some(expected));
    });
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
}

#[test]
fn a_pending_node_name_makes_persistence_due_immediately() {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let names = NodeNameStoreExchange::new();
    embassy_futures::block_on(async {
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _, _>::with_configuration_stores(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            &groups, &names,
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = available_remote_control(&mut engine);
        persistence
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        // Settle whatever a fresh journal schedules first, so only the name can make it due.
        for now in 1..64 {
            match persistence.next_deadline(InstantMillis(now)) {
                Some(deadline) if deadline.0 <= now => {
                    persistence.progress(&mut engine, InstantMillis(now)).await;
                }
                _ => break,
            }
        }
        assert!(persistence
            .next_deadline(InstantMillis(5))
            .is_none_or(|deadline| deadline.0 > 5));
        let stored = names.store(RemoteControlNodeName::new("Box-Hopspot").unwrap());
        // Without this the manifold spins: work is signalled but progress is never due.
        assert_eq!(
            persistence.next_deadline(InstantMillis(5)),
            Some(InstantMillis(5))
        );
        persistence.progress(&mut engine, InstantMillis(5)).await;
        assert_eq!(stored.await, Ok(()));
        assert!(persistence
            .next_deadline(InstantMillis(6))
            .is_none_or(|deadline| deadline.0 > 6));
    });
}

#[test]
fn node_name_survives_progress_reboot_compaction_and_reboot() {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let names = NodeNameStoreExchange::new();
    let persistence_on = |flash| {
        EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _, _>::with_configuration_stores(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
            &groups, &names,
        )
    };
    embassy_futures::block_on(async {
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = available_remote_control(&mut engine);
        let mut persistence = persistence_on(TestFlash::new());
        persistence
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        assert_eq!(names.restored_now(), Some(None));

        let name = RemoteControlNodeName::new("Rooftop RAK").unwrap();
        let stored = names.store(name);
        persistence.progress(&mut engine, InstantMillis(1)).await;
        assert_eq!(stored.await, Ok(()));
        assert_eq!(names.restored_now(), Some(Some(name)));

        let mut rebooted = persistence_on(persistence.journal.take().unwrap().release());
        names.reset_for_test();
        rebooted
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        assert_eq!(names.restored_now(), Some(Some(name)));

        rebooted.require_snapshot(EmbeddedPersistenceTarget::CriticalState, InstantMillis(2));
        rebooted.try_start_compaction(&engine, InstantMillis(2));
        for now in 2..32 {
            if rebooted.compaction.is_none() {
                break;
            }
            rebooted
                .progress_compaction(&engine, InstantMillis(now))
                .await;
        }
        assert_eq!(rebooted.compaction, None);

        let mut compacted = persistence_on(rebooted.journal.take().unwrap().release());
        names.reset_for_test();
        compacted
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        assert_eq!(names.restored_now(), Some(Some(name)));
    });
}

#[test]
fn node_name_owners_isolate_restore_failure_cancellation_and_retry() {
    #[track_caller]
    fn ready<F: core::future::Future>(future: F) -> F::Output {
        use core::task::{Context, Poll, Waker};
        match core::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("expected a settled name request"),
        }
    }
    let names = [NodeNameStoreExchange::new(), NodeNameStoreExchange::new()];
    let groups = [
        DiscoveryGroupConfigurationStoreExchange::new(),
        DiscoveryGroupConfigurationStoreExchange::new(),
    ];
    let first_name = RemoteControlNodeName::new("First node").unwrap();
    let second_name = RemoteControlNodeName::new("Second node").unwrap();
    embassy_futures::block_on(async {
        let (flash, fail_write) = TestFlash::controlled();
        let mut owners = [(flash, &groups[0], &names[0]), (TestFlash::new(), &groups[1], &names[1])].map(|(flash, groups, names)| {
            EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _, _>::with_configuration_stores(
                flash, LAYOUT,
                EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
                FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), groups, names,
            )
        });
        let mut engines = core::array::from_fn::<_, 2, _>(|_| {
            EngineState::<crate::storage::GrowableHeap>::default()
        });
        for (owner, engine) in owners.iter_mut().zip(&mut engines) {
            let mut remote = available_remote_control(engine);
            owner.restore(engine, &mut remote, InstantMillis(0)).await;
        }
        let first = names[0].store(first_name);
        let second = names[1].store(second_name);
        fail_write.set(true);
        owners[0].progress(&mut engines[0], InstantMillis(1)).await;
        assert_eq!(ready(first), Err(EmbeddedPersistenceFailure::Flash));
        assert_eq!(names[0].restored_now(), Some(None));
        assert!(names[1].has_pending_request());
        owners[1].progress(&mut engines[1], InstantMillis(1)).await;
        assert_eq!(ready(second), Ok(()));
        assert_eq!(names[1].restored_now(), Some(Some(second_name)));

        let abandoned = names[1].store(first_name);
        drop(abandoned);
        assert_eq!(
            ready(names[1].store(second_name)),
            Err(EmbeddedPersistenceFailure::Capacity)
        );
        owners[1].progress(&mut engines[1], InstantMillis(2)).await;
        assert_eq!(names[1].restored_now(), Some(Some(first_name)));
        let subsequent = names[1].store(second_name);
        owners[1].progress(&mut engines[1], InstantMillis(3)).await;
        assert_eq!(ready(subsequent), Ok(()));

        let retry = names[0].store(first_name);
        let retry_at = owners[0].retry_not_before.unwrap_or(InstantMillis(4));
        // A failed append may retire an arena; allow each bounded compaction phase to run.
        for step in 0..32 {
            if !names[0].has_pending_request() {
                break;
            }
            owners[0]
                .progress(&mut engines[0], InstantMillis(retry_at.0 + step))
                .await;
        }
        assert_eq!(ready(retry), Ok(()));
        assert_eq!(names[0].restored_now(), Some(Some(first_name)));
        engines[0] = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote = available_remote_control(&mut engines[0]);
        let flash = owners[0].journal.take().unwrap().release();
        let mut rebooted = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _, _>::with_configuration_stores(
            flash, LAYOUT, EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups[0], &names[0],
        );
        rebooted
            .restore(&mut engines[0], &mut remote, InstantMillis(0))
            .await;
        assert_eq!(names[0].restored_now(), Some(Some(first_name)));
        assert_eq!(names[1].restored_now(), Some(Some(second_name)));
    });
    assert_eq!(core::mem::size_of::<GlobalNodeNameStore>(), 0);
}

#[test]
fn cancelled_group_store_cannot_be_overwritten_or_observe_stale_completion() {
    let _store = lock_discovery_group_store();
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
    let interface_id =
        crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
    let first = DiscoveryGroupConfigurationChange::upsert(
        interface_id,
        crate::interfaces::DiscoveryGroupSet::reticulum(),
    );

    let abandoned = store_discovery_group_configuration(first);
    drop(abandoned);
    assert_eq!(
        embassy_futures::block_on(store_discovery_group_configuration(first)),
        Err(EmbeddedPersistenceFailure::Capacity),
    );

    let pending = DISCOVERY_GROUP_CONFIGURATION_STORES
        .try_take_request()
        .expect("the abandoned request remains owned by the writer");
    assert_eq!(
        embassy_futures::block_on(store_discovery_group_configuration(first)),
        Err(EmbeddedPersistenceFailure::Capacity),
    );
    DISCOVERY_GROUP_CONFIGURATION_STORES.settle(Ok(()));

    let replacement = DiscoveryGroupConfigurationChange::upsert(
        interface_id,
        discovery_group_snapshot("replacement")
            .groups_for(interface_id)
            .copied()
            .expect("fixture contains the interface"),
    );
    let replacement_result = store_discovery_group_configuration(replacement);
    let admitted = DISCOVERY_GROUP_CONFIGURATION_STORES
        .try_take_request()
        .expect("settlement reopens the bounded exchange");
    assert_eq!(admitted.interface_id, pending.interface_id);
    DISCOVERY_GROUP_CONFIGURATION_STORES.settle(Err(EmbeddedPersistenceFailure::Flash));
    assert_eq!(
        embassy_futures::block_on(replacement_result),
        Err(EmbeddedPersistenceFailure::Flash),
    );
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
}

#[test]
fn group_store_without_an_open_journal_settles_as_flash_failure() {
    let _store = lock_discovery_group_store();
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
    let interface_id =
        crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
    let change = DiscoveryGroupConfigurationChange::upsert(
        interface_id,
        crate::interfaces::DiscoveryGroupSet::reticulum(),
    );
    let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
        TestFlash::new(),
        LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let now = InstantMillis(123);
    let pending = store_discovery_group_configuration(change);

    assert_eq!(persistence.next_deadline(now), Some(now));
    embassy_futures::block_on(persistence.progress(&mut engine, now));
    assert_eq!(
        embassy_futures::block_on(pending),
        Err(EmbeddedPersistenceFailure::Flash),
    );
    assert_eq!(persistence.next_deadline(now), None);
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
}

#[test]
fn group_store_commits_restored_state_only_after_durable_success() {
    let _store = lock_discovery_group_store();
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
    let interface_id =
        crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
    assert_eq!(restored_discovery_groups_now(interface_id), None);
    DISCOVERY_GROUP_CONFIGURATION_STORES
        .publish_restored(Some(DiscoveryGroupConfigurationSnapshot::empty()));
    assert_eq!(restored_discovery_groups_now(interface_id), Some(None));

    let desired = discovery_group_snapshot("durable")
        .groups_for(interface_id)
        .copied()
        .expect("fixture contains the interface");
    let change = DiscoveryGroupConfigurationChange::upsert(interface_id, desired);
    assert!(DISCOVERY_GROUP_CONFIGURATION_STORES.submit(change));
    let admitted = DISCOVERY_GROUP_CONFIGURATION_STORES
        .try_take_request()
        .expect("submitted change is ready");
    DISCOVERY_GROUP_CONFIGURATION_STORES.settle(Err(EmbeddedPersistenceFailure::Flash));
    assert_eq!(restored_discovery_groups_now(interface_id), Some(None));

    assert!(DISCOVERY_GROUP_CONFIGURATION_STORES.submit(admitted));
    let admitted = DISCOVERY_GROUP_CONFIGURATION_STORES
        .try_take_request()
        .expect("retried change is ready");
    assert_eq!(
        DISCOVERY_GROUP_CONFIGURATION_STORES.commit_change(&admitted),
        Ok(()),
    );
    DISCOVERY_GROUP_CONFIGURATION_STORES.settle(Ok(()));
    assert_eq!(
        restored_discovery_groups_now(interface_id),
        Some(Some(desired)),
    );
    DISCOVERY_GROUP_CONFIGURATION_STORES.reset_for_test();
}

#[test]
fn authorization_store_returns_the_flash_retry_policy_deadline() {
    embassy_futures::block_on(async {
        let (flash, fail_next_write) = TestFlash::controlled();
        let mut policy =
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
        policy.retry_interval_millis = 1_234;
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            policy,
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        restore_without_remote_control(&mut persistence, &mut engine, InstantMillis(0)).await;
        let snapshot = RemoteControlAuthorizationSnapshot::from_slice(&[0x80]).unwrap();
        fail_next_write.set(true);
        for now in [8, 9, 1_241] {
            assert_eq!(
                persistence
                    .store_remote_control_authorization_snapshot(
                        &engine,
                        RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                        &snapshot,
                        InstantMillis(now),
                    )
                    .await,
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                    failure: EmbeddedPersistenceFailure::Flash,
                    retry_at: Some(InstantMillis(1_242)),
                },
            );
        }
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                    &snapshot,
                    InstantMillis(1_242),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress,
        );
        for _ in 0..32 {
            persistence
                .progress_compaction(&engine, InstantMillis(1_242))
                .await;
            if persistence.compaction.is_none() {
                break;
            }
        }
        assert!(persistence.compaction.is_none());
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                    &snapshot,
                    InstantMillis(1_242),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored
        );
    });
}

#[test]
fn authorization_store_returns_the_compaction_cooldown_deadline() {
    embassy_futures::block_on(async {
        let mut persistence = ready();
        let engine = EngineState::<crate::storage::GrowableHeap>::default();
        let snapshot = RemoteControlAuthorizationSnapshot::from_slice(&[0; 512]).unwrap();
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &snapshot,
                    InstantMillis(8),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
        );
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &snapshot,
                    InstantMillis(9),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Capacity,
                retry_at: Some(InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS)),
            },
        );
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &snapshot,
                    InstantMillis(HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress,
        );
        assert!(persistence.compaction.is_some());
    });
}

#[test]
fn authorization_store_exposes_retry_after_a_failed_compaction_step() {
    embassy_futures::block_on(async {
        let (flash, fail_next_write) = TestFlash::controlled();
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        restore_without_remote_control(&mut persistence, &mut engine, InstantMillis(0)).await;
        persistence.require_snapshot(EmbeddedPersistenceTarget::CriticalState, InstantMillis(8));
        persistence.try_start_compaction(&engine, InstantMillis(8));
        assert!(persistence.compaction.is_some());
        fail_next_write.set(true);
        let snapshot = RemoteControlAuthorizationSnapshot::new();
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &snapshot,
                    InstantMillis(8),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress,
        );
        assert!(persistence.compaction.is_none());
        // progress_compaction records failure internally. One more store call
        // exposes its retry deadline; the wrapper must then stop calling early.
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &snapshot,
                    InstantMillis(9),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Flash,
                retry_at: Some(InstantMillis(8 + persistence.policy.retry_interval_millis)),
            },
        );
    });
}

#[test]
fn unavailable_authorization_store_has_no_retry_schedule() {
    embassy_futures::block_on(async {
        let mut persistence = ready();
        persistence.journal = None;
        let engine = EngineState::<crate::storage::GrowableHeap>::default();
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                    &RemoteControlAuthorizationSnapshot::new(),
                    InstantMillis(8),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                failure: EmbeddedPersistenceFailure::Flash,
                retry_at: None,
            },
        );
    });
}

#[test]
fn authorization_snapshots_survive_compaction_and_restore_complete_runtime_tables() {
    let _group_store = lock_discovery_group_store();
    use crate::identity::IdentityPublicKeys;
    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind, RemoteControlRequestSet,
        RemoteControlTargetAccess, RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
    };

    embassy_futures::block_on(async {
        let mut persistence = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            TestFlash::new(),
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = available_remote_control(&mut engine);
        persistence
            .restore(&mut engine, &mut remote_control, InstantMillis(0))
            .await;
        let grant = crate::runtime::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        remote_control.set_controller_grant(grant).unwrap();
        let target_public_keys = IdentityPublicKeys {
            encryption: grant.controller().public_keys().encryption,
            signing: grant.controller().public_keys().signing,
        };
        let access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(target_public_keys),
            crate::remote_control::RemoteControlControllerAuthority::Operator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
        )
        .unwrap();
        remote_control.set_target_access(access).unwrap();
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &controller_grants_snapshot(&remote_control),
                    InstantMillis(1),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
        );
        assert_eq!(
            persistence
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                    &target_accesses_snapshot(&remote_control),
                    InstantMillis(2),
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored,
        );

        persistence.require_snapshot(EmbeddedPersistenceTarget::CriticalState, InstantMillis(3));
        persistence.try_start_compaction(&engine, InstantMillis(3));
        for now in 3..32 {
            if persistence.compaction.is_none() {
                break;
            }
            persistence
                .progress_compaction(&engine, InstantMillis(now))
                .await;
        }
        assert!(persistence.compaction.is_none());

        let flash = persistence.journal.take().unwrap().release();
        let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4>::new(
            flash,
            LAYOUT,
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
            FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        );
        let mut restored_engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut restored_remote_control = available_remote_control(&mut restored_engine);
        let report = restored
            .restore(
                &mut restored_engine,
                &mut restored_remote_control,
                InstantMillis(0),
            )
            .await;

        assert_eq!(report.remote_control_controller_grants_restored_count, 1);
        assert_eq!(report.remote_control_controller_grants_refused_count, 0);
        assert_eq!(report.remote_control_controller_grants_dropped_count, 0);
        assert_eq!(report.remote_control_target_accesses_restored_count, 1);
        assert_eq!(report.remote_control_target_accesses_refused_count, 0);
        assert_eq!(report.remote_control_target_accesses_dropped_count, 0);
        assert_eq!(
            restored_remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[grant],
        );
        let restored_access = restored_remote_control
            .target_accesses()
            .unwrap()
            .accesses_in_identity_hash_order()
            .first()
            .unwrap();
        assert_eq!(restored_access.target().public_keys(), &target_public_keys);
        assert_eq!(
            restored_access.permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
        );
    });
}

#[test]
fn thirty_days_of_pressure_erase_each_arena_sector_at_most_fifteen_times() {
    let _group_store = lock_discovery_group_store();
    let diagnostics = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&diagnostics);
    let mut persistence = ready_with_observer(move |diagnostic| {
        observed.borrow_mut().push(diagnostic);
    });
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    embassy_futures::block_on(async {
        for day in 1..=30 {
            let now = InstantMillis(day * HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
            persistence.require_snapshot(EmbeddedPersistenceTarget::Routes, now);
            persistence.route_dirty_since = Some(InstantMillis(now.0 - 2_000));
            for _ in 0..EMPTY_STATE_COMPACTION_PROGRESS_STEPS {
                persistence.progress(&mut engine, now).await;
                if persistence.compaction.is_none() && !persistence.snapshot_required {
                    break;
                }
            }
            assert_eq!(persistence.compaction, None);
        }
    });
    assert_eq!(
        diagnostics
            .borrow()
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic,
                EmbeddedPersistenceDiagnostic::CompactionStarted { .. }
            ))
            .count(),
        30
    );
    let flash = persistence.journal.take().unwrap().release();
    assert_eq!(
        [
            flash.sector_erases[2] - 1,
            flash.sector_erases[3] - 1,
            flash.sector_erases[4],
            flash.sector_erases[5],
        ],
        [15, 15, 15, 15]
    );
}
