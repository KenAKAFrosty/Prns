use super::*;

#[derive(Clone, Copy, Debug)]
enum BudgetPage {
    Fresh,
    Rollover,
}

async fn image(page: BudgetPage) -> [u8; CAPACITY] {
    let mut flash = TestFlash::new();
    flash.bytes = baseline(Campaign::CompactThenAppend).await;
    let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |_| {})
        .await
        .unwrap();
    if matches!(page, BudgetPage::Rollover) {
        // Fill both 512-byte pages with the existing 32-byte timebase records.
        for value in 1..=32 {
            journal.record_timebase(InstantMillis(value)).await.unwrap();
        }
    }
    journal.release().bytes
}

async fn exercise(image: [u8; CAPACITY], cut: Option<Cut>) -> Vec<Operation> {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let policy = EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    owner.require_snapshot(EmbeddedPersistenceTarget::CriticalState, WRITE_TIME);
    owner.try_start_compaction(&engine, WRITE_TIME);
    assert_eq!(
        owner.compaction,
        Some(CompactionPhase::RecordBudget { at: WRITE_TIME })
    );
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    if cut.is_some() {
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        let mut pending = core::pin::pin!(owner.progress_compaction(&engine, WRITE_TIME));
        for _ in 0..2 {
            assert!(core::future::Future::poll(pending.as_mut(), &mut context).is_pending());
        }
    } else {
        owner.progress_compaction(&engine, WRITE_TIME).await;
    }
    let trace = control.borrow().trace.clone();
    if cut.is_some() {
        assert_eq!(
            owner.compaction,
            Some(CompactionPhase::RecordBudget { at: WRITE_TIME })
        );
        assert_eq!(owner.next_compaction_not_before, None);
        control.borrow_mut().arm(None);
        owner.progress_compaction(&engine, WRITE_TIME).await;
        if let Some(Cut {
            operation,
            completed_bytes,
        }) = cut
        {
            if matches!(trace[operation], Operation::Write { len, .. } if completed_bytes == len) {
                assert!(control
                    .borrow()
                    .trace
                    .iter()
                    .all(|op| matches!(op, Operation::Read { .. })));
            }
        }
    }
    assert_eq!(owner.compaction, Some(CompactionPhase::Erase { sector: 0 }));
    let allowed = owner
        .next_compaction_not_before
        .expect("durable budget precedes arena erase");
    assert!(allowed.0 >= WRITE_TIME.0 + policy.compaction.minimum_interval_millis);
    assert!(control.borrow().trace.iter().all(|op| match op {
        Operation::Erase { offset, .. } | Operation::Write { offset, .. } => *offset < 2 * ERASE,
        Operation::Read { .. } => true,
    }));
    let persisted = owner.journal.take().unwrap().release().into_image();
    assert_eq!(&persisted[2 * ERASE..], &image[2 * ERASE..]);
    let mut flash = TestFlash::new();
    flash.bytes = persisted;
    let durable = FlashJournal::inspect_timebase_state(&mut flash, LAYOUT)
        .await
        .unwrap();
    assert_eq!(
        durable
            .last_compaction_attempt
            .map(|at| InstantMillis(at.0 + policy.compaction.minimum_interval_millis)),
        Some(allowed),
    );
    for _ in 0..2 {
        let restored_groups = DiscoveryGroupConfigurationStoreExchange::new();
        let mut rebooted = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
            Flash::boot(persisted, control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &restored_groups,
        );
        control.borrow_mut().arm(None);
        rebooted
            .restore(&mut engine, &mut remote, InstantMillis(0))
            .await;
        assert_eq!(
            restored_groups.restored_now(),
            Some(discovery_group_snapshot("confirmed"))
        );
        assert_eq!(rebooted.next_compaction_not_before, Some(allowed));
        control.borrow_mut().arm(None);
        rebooted.try_start_compaction(&engine, InstantMillis(allowed.0 - 1));
        assert_eq!(rebooted.compaction, None);
        assert!(control.borrow().trace.is_empty());
        rebooted.try_start_compaction(&engine, allowed);
        assert_eq!(
            rebooted.compaction,
            Some(CompactionPhase::RecordBudget { at: allowed })
        );
    }
    trace
}

#[test]
fn cancelled_completed_budget_is_read_back_without_another_write() {
    embassy_futures::block_on(async {
        let image = image(BudgetPage::Fresh).await;
        let trace = exercise(image, None).await;
        let operation = trace
            .iter()
            .rposition(|op| matches!(op, Operation::Write { .. }))
            .unwrap();
        let Operation::Write { len, .. } = trace[operation] else {
            unreachable!()
        };
        exercise(
            image,
            Some(Cut {
                operation,
                completed_bytes: len,
            }),
        )
        .await;
    });
}

#[test]
fn cancelled_compaction_budget_rescans_before_erase_and_restores_cooldown() {
    embassy_futures::block_on(async {
        let mut cases = 0;
        for page in [BudgetPage::Fresh, BudgetPage::Rollover] {
            let image = image(page).await;
            let trace = exercise(image, None).await;
            assert_eq!(
                trace.iter().any(|op| matches!(op, Operation::Erase { .. })),
                matches!(page, BudgetPage::Rollover)
            );
            for (operation, event) in trace.iter().enumerate() {
                let prefixes: Vec<_> = match event {
                    Operation::Read { len, .. } => std::vec![0, *len],
                    Operation::Write { len, .. } | Operation::Erase { len, .. } => {
                        (0..=*len).collect()
                    }
                };
                for completed_bytes in prefixes {
                    let observed = exercise(
                        image,
                        Some(Cut {
                            operation,
                            completed_bytes,
                        }),
                    )
                    .await;
                    assert_eq!(observed, trace[..=operation]);
                    cases += 1;
                }
            }
        }
        std::eprintln!("verified {cases} compaction budget cancellation boundaries");
    });
}
