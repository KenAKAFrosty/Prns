use super::*;

type Owner = EmbeddedFlashPersistence<
    TestFlash,
    FixedRouteSnapshotKeys<8>,
    fn(EmbeddedPersistenceDiagnostic),
    4,
>;
type TestEngine = EngineState<crate::storage::GrowableHeap>;

fn policy(threshold: u32) -> EmbeddedPersistencePolicy {
    EmbeddedPersistencePolicy::hopspot_default(
        EmbeddedCompactionPolicy::hopspot(0).with_route_replay_compaction_threshold(threshold),
    )
}

async fn append_row(journal: &mut FlashJournal<TestFlash>, row: PersistedRouteRow<'_>) {
    let mut payload = [0; RECORD_SCRATCH_LEN];
    let len = encode_route_upsert(row, &mut payload).unwrap();
    journal
        .append(FlashJournalRecordKind::RouteUpsert, &payload[..len])
        .await
        .unwrap();
}

async fn history(secrets: &[u8], budget: Option<InstantMillis>) -> TestFlash {
    let (mut journal, _) = FlashJournal::open(
        TestFlash::new(),
        LAYOUT,
        &mut [0; RECORD_SCRATCH_LEN],
        |_| {},
    )
    .await
    .unwrap();
    journal.initialize_empty().await.unwrap();
    for secret in secrets {
        append_row(&mut journal, signed_route(*secret, b"saved")).await;
    }
    if let Some(at) = budget {
        journal.record_compaction_budget(at).await.unwrap();
    }
    journal.release()
}

async fn restore(
    flash: TestFlash,
    policy: EmbeddedPersistencePolicy,
) -> (Owner, TestEngine, EmbeddedPersistenceRestoreReport) {
    let mut owner = EmbeddedFlashPersistence::new(
        flash,
        LAYOUT,
        policy,
        FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
    );
    let mut engine = TestEngine::default();
    let report = restore_without_remote_control(&mut owner, &mut engine, InstantMillis(0)).await;
    (owner, engine, report)
}

async fn finish(owner: &mut Owner, engine: &mut TestEngine, now: InstantMillis) {
    for _ in 0..32 {
        owner.progress(engine, now).await;
        if owner.compaction.is_none() && owner.landing_batch.is_none() {
            return;
        }
    }
    panic!("bounded compaction must finish");
}

#[test]
fn idle_restore_compacts_replay_history_then_reboots_without_another_request() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let (mut owner, mut engine, report) =
            restore(history(&[7, 7], None).await, policy(2)).await;
        assert_eq!(report.route_seeded_count, 2);
        assert_eq!(owner.route_replay_records, 2);
        assert!(owner.pending_routes.is_empty());
        assert_eq!(owner.route_dirty_since, None);
        assert!(!owner.snapshot_required);
        assert!(!owner.state_not_saved());
        assert_eq!(
            owner.next_deadline(report.logical_start),
            Some(report.logical_start)
        );
        owner.progress(&mut engine, report.logical_start).await;
        assert!(matches!(
            owner.compaction,
            Some(CompactionPhase::RecordBudget { .. })
        ));
        assert_eq!(
            owner.route_replay_records, 2,
            "planning cannot reset active history"
        );
        finish(&mut owner, &mut engine, report.logical_start).await;
        assert_eq!(owner.route_replay_records, 1);
        assert!(!owner.state_not_saved());
        let flash = owner.journal.take().unwrap().release();
        let (rebooted, restored, second) = restore(flash, policy(2)).await;
        assert_eq!(second.route_seeded_count, 1);
        assert_eq!(restored.persisted_route_destinations().count(), 1);
        assert!(!rebooted.replay_compaction_requested);
    });
}

#[test]
fn replay_policy_is_opt_in_and_requires_both_threshold_and_snapshot_benefit() {
    let _guard = lock_discovery_group_store();
    assert_eq!(
        EmbeddedCompactionPolicy::new(1, 0).route_replay_compaction_threshold,
        0
    );
    assert_eq!(
        EmbeddedCompactionPolicy::hopspot(0).route_replay_compaction_threshold,
        256
    );
    embassy_futures::block_on(async {
        for (secrets, threshold) in [(&[7, 7][..], 0), (&[7, 7][..], 3), (&[7, 8][..], 2)] {
            let (owner, _, _) = restore(history(secrets, None).await, policy(threshold)).await;
            assert!(!owner.replay_compaction_requested);
        }
    });
}

#[test]
fn malformed_route_removals_do_not_count_as_signature_replay_work() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let flash = history(&[7], None).await;
        let (mut journal, _) =
            FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |_| {})
                .await
                .unwrap();
        journal
            .append(FlashJournalRecordKind::RouteRemoval, &[])
            .await
            .unwrap();
        let (owner, _, report) = restore(journal.release(), policy(2)).await;
        assert_eq!(
            (report.route_seeded_count, report.route_refused_count),
            (1, 1)
        );
        assert_eq!(owner.route_replay_records, 1);
        assert!(!owner.replay_compaction_requested);
    });
}

#[test]
fn successful_steady_state_appends_cross_the_replay_threshold_after_the_batch_lands() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let (mut owner, mut engine, _) = restore(history(&[7], None).await, policy(2)).await;
        let destination = signed_route(7, b"saved").destination;
        owner.queue_route(
            PendingRouteDelta::RouteUpsert(destination),
            InstantMillis(0),
        );
        owner.route_dirty_since = Some(InstantMillis(0));
        owner.progress(&mut engine, InstantMillis(2_000)).await;
        assert_eq!(owner.route_replay_records, 2);
        assert!(owner.replay_compaction_requested);
        assert_eq!(owner.landing_batch, Some(BatchKind::Routes));
        assert_eq!(owner.compaction, None);
        owner.progress(&mut engine, InstantMillis(2_000)).await;
        assert_eq!(owner.landing_batch, None);
        owner.progress(&mut engine, InstantMillis(2_000)).await;
        assert!(matches!(
            owner.compaction,
            Some(CompactionPhase::RecordBudget { .. })
        ));
    });
}

#[test]
fn maintenance_waits_for_persisted_wear_budget_without_reporting_unsaved_state_or_spinning() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let (mut owner, mut engine, report) = restore(
            history(&[7, 7], Some(InstantMillis(1_000))).await,
            policy(2),
        )
        .await;
        let allowed = owner.next_compaction_not_before.unwrap();
        let now = report.logical_start;
        assert!(now < allowed);
        owner.progress(&mut engine, now).await; // The normal timebase record may be due.
        assert!(owner.next_deadline(now).unwrap() > now);
        assert!(!owner.state_not_saved());
        assert_eq!(owner.deferred_target, None);
        assert_eq!(owner.compaction, None);
        assert!(owner.replay_compaction_requested);
        let flash = owner.journal.take().unwrap().release();
        let (mut rebooted, mut fresh_engine, _) = restore(flash, policy(2)).await;
        assert_eq!(rebooted.next_compaction_not_before, Some(allowed));
        rebooted
            .progress(&mut fresh_engine, InstantMillis(allowed.0 - 1))
            .await;
        assert_eq!(rebooted.compaction, None);
        assert!(!rebooted.state_not_saved());
        rebooted.progress(&mut fresh_engine, allowed).await;
        assert!(matches!(
            rebooted.compaction,
            Some(CompactionPhase::RecordBudget { .. })
        ));
    });
}

#[test]
fn failed_maintenance_retains_old_replay_count_and_cannot_repeat_erases_before_cooldown() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let flash = history(&[7, 7], None).await;
        let fail = flash.fail_next_write.clone();
        let (mut owner, mut engine, report) = restore(flash, policy(2)).await;
        let now = report.logical_start;
        for _ in 0..8 {
            owner.progress(&mut engine, now).await;
            if owner.compaction == Some(CompactionPhase::Routes { index: 0 }) {
                break;
            }
        }
        assert_eq!(owner.compaction, Some(CompactionPhase::Routes { index: 0 }));
        let allowed = owner.next_compaction_not_before.unwrap();
        fail.set(true);
        owner.progress(&mut engine, now).await;
        assert_eq!(owner.compaction, None);
        assert_eq!(owner.route_replay_records, 2);
        assert!(owner.next_deadline(now).unwrap() > now);
        owner
            .progress(&mut engine, InstantMillis(allowed.0 - 1))
            .await;
        assert_eq!(owner.compaction, None);
        let flash = owner.journal.take().unwrap().release();
        let (rebooted, _, report) = restore(flash, policy(2)).await;
        assert_eq!(report.route_seeded_count, 2);
        assert_eq!(rebooted.route_replay_records, 2);
        assert_eq!(rebooted.next_compaction_not_before, Some(allowed));
    });
}

#[test]
fn compaction_keeps_the_valid_predecessor_when_the_last_route_signature_is_invalid() {
    let _guard = lock_discovery_group_store();
    embassy_futures::block_on(async {
        let flash = history(&[7, 7], None).await;
        let (mut journal, _) =
            FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |_| {})
                .await
                .unwrap();
        let mut forged = signed_route(7, b"forged");
        forged.signature = crate::crypto::Ed25519Signature([0; 64]);
        append_row(&mut journal, forged).await;
        let (mut owner, mut engine, report) = restore(journal.release(), policy(2)).await;
        assert_eq!(
            (report.route_seeded_count, report.route_refused_count),
            (2, 1)
        );
        assert_eq!(owner.route_replay_records, 3);
        finish(&mut owner, &mut engine, report.logical_start).await;
        let (_, restored, report) =
            restore(owner.journal.take().unwrap().release(), policy(2)).await;
        assert_eq!(
            (report.route_seeded_count, report.route_refused_count),
            (1, 0)
        );
        assert_eq!(
            restored
                .persisted_route_row(&signed_route(7, b"saved").destination)
                .unwrap()
                .app_data,
            b"saved"
        );
    });
}
