use super::*;

// The 100 ms attempt is rounded up to the journal's next whole-minute marker.
const RECORDED_ATTEMPT: InstantMillis = InstantMillis(60_000);

type Owner<'a> = EmbeddedFlashPersistence<
    Flash,
    FixedRouteSnapshotKeys<8>,
    fn(EmbeddedPersistenceDiagnostic),
    4,
    &'a DiscoveryGroupConfigurationStoreExchange,
    NodeNameStoreExchange,
>;

pub(super) enum Recovery {
    Immediate,
    AfterCooldown,
}

async fn submit(
    owner: &mut Owner<'_>,
    exchange: &DiscoveryGroupConfigurationStoreExchange,
    engine: &mut EngineState<crate::storage::GrowableHeap>,
    now: InstantMillis,
) -> Result<(), EmbeddedPersistenceFailure> {
    let interface =
        crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
    let next = discovery_group_snapshot("after-reboot");
    let change =
        DiscoveryGroupConfigurationChange::upsert(interface, *next.groups_for(interface).unwrap());
    let mut completion = core::pin::pin!(exchange.store(change));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
    for _ in 0..MAX_PROGRESS_STEPS {
        owner.progress(engine, now).await;
        if let core::task::Poll::Ready(result) =
            core::future::Future::poll(completion.as_mut(), &mut context)
        {
            return result;
        }
    }
    panic!("recovered owner failed to settle within its work budget");
}

pub(super) async fn verify(
    image: [u8; CAPACITY],
    expected: DiscoveryGroupConfigurationSnapshot,
) -> Recovery {
    let exchange = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()),
        LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
        &exchange,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    let report = owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert_eq!(exchange.restored_now(), Some(expected));
    control.borrow_mut().arm(None);
    let recovery = match submit(&mut owner, &exchange, &mut engine, report.logical_start).await {
        Ok(()) => Recovery::Immediate,
        Err(EmbeddedPersistenceFailure::Capacity) => {
            let deadline =
                InstantMillis(RECORDED_ATTEMPT.0 + HOPSPOT_MINIMUM_COMPACTION_INTERVAL_MILLIS);
            assert_eq!(owner.next_compaction_not_before, Some(deadline));
            assert!(report.logical_start.0 < deadline.0);
            assert!(
                control.borrow().trace.is_empty(),
                "cooldown must not write or erase"
            );
            assert_eq!(exchange.restored_now(), Some(expected));
            assert_eq!(
                submit(
                    &mut owner,
                    &exchange,
                    &mut engine,
                    InstantMillis(deadline.0 - 1)
                )
                .await,
                Err(EmbeddedPersistenceFailure::Capacity)
            );
            assert!(control.borrow().trace.is_empty());
            assert_eq!(exchange.restored_now(), Some(expected));
            assert_eq!(
                submit(&mut owner, &exchange, &mut engine, deadline).await,
                Ok(())
            );
            Recovery::AfterCooldown
        }
        Err(failure) => panic!("recovered owner refused healthy storage: {failure:?}"),
    };
    let next = discovery_group_snapshot("after-reboot");
    assert_eq!(exchange.restored_now(), Some(next));
    let image = owner.journal.take().unwrap().release().into_image();
    drop(owner);
    for _ in 0..2 {
        assert_eq!(reboot(image).await, next);
    }
    recovery
}
